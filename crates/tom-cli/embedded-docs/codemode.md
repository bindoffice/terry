# codemode：在沙箱里写脚本调用工具

`codemode` 让模型写一段 JavaScript，由宿主在 QuickJS 沙箱里执行；脚本唯一的
能力是调用注入的工具。脚本里的工具调用与结果**不进入模型上下文**，只有脚本自己
的输出回去。

它解决的问题是：当一步流程本来就是「批量、链式、过滤、分支」时，逐次工具调用
既慢又贵——每次调用都要把整段结果带回上下文。脚本把同一件事写成一次调用。

```js
// 一次读出所有 README，只把「哪些缺 LICENSE」给模型
const files = ['a/README.md', 'b/README.md', 'c/README.md'];
const missing = [];
for (const path of files) {
  try { await tools.read({ path }); }
  catch (error) { missing.push(path); }
}
text(missing.length + ' missing: ' + missing.join(', '));
```

## 什么时候用

- **批量**：对一列文件/仓库/PR 做同一件事。`Promise.all` 会让这些调用真正并发。
- **链式**：前一步的结果决定下一步的参数，模型不需要看见中间结果。
- **过滤**：结果很大而只需要其中一小部分时，先在脚本里筛，再输出摘要。
- **分支**：按退出码、字段值决定下一步，而不是让模型每步重新决定。

不适合：单次调用、需要模型判断中间结果、探索性任务（模型需要看到每一步）。

## 脚本环境

脚本是 **async 函数的函数体**：顶层 `await` 与 `return` 都可用。

| 名字 | 作用 |
| --- | --- |
| `tools.<name>(args)` | 调用工具，返回 Promise。工具返回文本时得到字符串，声明了结构化输出时得到对象；失败时 reject |
| `text(value)` | 追加一段输出。非字符串会 JSON 序列化 |
| `return value` | 返回值同样作为输出追加 |
| `console.log/info/warn/error/debug` | 追加 console 行，结果里汇集成一个 `<console_output>` 块 |
| `image(dataUrlOrBlock)` | 追加一张图片。只接受 base64 `data:` URL 或图片块；远程 URL 被拒绝 |
| `exit()` | 立即结束脚本，保留已有输出与 `store()` 写入 |
| `store(key, value)` / `load(key)` | 跨脚本调用保存 JSON 值（`store(key, undefined)` 删除） |
| `ALL_TOOLS` | 所有可调用工具的 `{ name, description }` |
| `searchTools(query, { limit?, namespace? })` | 按 BM25 检索工具 |
| `describeTool(name)` / `describeNamespace(name)` | 取工具的完整声明 / 命名空间说明 |

工具名会规范成合法标识符：`mcp__fs__read_file` 可直接写
`tools.mcp__fs__read_file`，原名 `tools["mcp__fs__read_file"]` 也可用。写到不存在
的成员时错误会提示最接近的名字。

### 首行选项

```js
// @options: {"max_output_tokens": 2000, "timeout_ms": 30000}
```

- `max_output_tokens`：输出预算（默认 10000）。超出时保留首尾并注明截断了多少。
- `timeout_ms`：整个脚本的硬期限，含其中的工具调用。
- 该行必须是第一行；写在别处会报错而不是被静默忽略（否则「预算没生效」无从发现）。

## 工具调用走的是同一条管线

脚本里的工具调用与模型直接发起的调用经过**同一套**流程：参数校验、
`before_tool_call` / `after_tool_call` 钩子、扩展的权限门。这是刻意的——否则
一个拦得住模型的权限门，用一次 `codemode` 就绕过了。

```jsonc
// 例：拦截 bash 的扩展钩子，对脚本同样生效
// 脚本里 try/catch 会拿到 "bash is not allowed here"
```

可调用集合是**已声明的工具 + 所有延迟工具**。延迟（`exposure: "deferred"`）的
含义是「模型还没拿到它」，不是「不可用」——脚本按名字调用它，正是延迟声明换来的
交互方式。脚本不能调用 `codemode` 自己（脚本不得启动脚本）。

## 沙箱边界

沙箱里**没有**：`fetch`、`require`、`process`、`setTimeout`、`WebAssembly`、
模块系统、文件系统、网络。宿主侧的限制：

| 限制 | 值 |
| --- | --- |
| 堆 | 256 MiB（超出时脚本内抛 `out of memory`） |
| 输出 | 16 Mi 字符 / 10 万项 |
| 单个 `store()` 值 | 256 Ki 字符 JSON |
| `store()` 总量 | 1 Mi 字符 JSON |
| 默认期限 | 10 分钟 |

内建对象在脚本运行前被冻结，所以改写 `JSON.stringify` 之类无法让宿主收到错误的
结果。脚本结束时仍在运行的调用会被取消。

## 结果格式

```
Script completed
Wall time 1.2 seconds
Output:
==> text 1/2 <==
第一段输出
==> text 2/2 <==
第二段输出
<console_output>
debug line
</console_output>
```

只有一段文本时不加 `==> text N/M <==` 前缀。脚本失败时结果是**错误结果**（和
`bash` 的非零退出一样），但仍保留失败前的输出，并列出已执行的调用：

```
Script error:
Error: boom
    at <anonymous> (codemode.js:3:7)

Tool calls made before the failure (they are not undone): read (ok), bash (error)
```

## `store()` 的生命周期

`store()` 写入落在**当前分支**上，作为 `codemode-store` 自定义条目；`load()` 按
分支重放这些条目。因此在 `/tree` 切分支、续接会话、fork 之后，每个分支看到的都是
它自己那条路径上写过的值，而不是进程内存里残留的值。

## 设置

```json
{
  "codemode": {
    "mode": "on",
    "inlineBudget": 3000
  }
}
```

- `mode`：`on`（默认）保留直连工具的声明，`codemode` 描述里只列脚本可见但模型
  看不到的工具（通常是 `deferred` 的 MCP 工具）；`only` 在 `codemode` 描述里列出
  所有脚本可调用的工具。`on` 下已声明的工具不进描述——重复一遍模型已经有的声明只
  花 token，不增加信息；它们在脚本里照样可调，`ALL_TOOLS` 也照旧列出。
- `inlineBudget`：描述里工具声明的估算 token 预算（字符数 / 4）。放不下的工具不
  列出，脚本用 `searchTools()` 找它们。

工具本身受常规工具策略控制：`--tools codemode,read`、`--exclude-tools codemode`、
`defaultTools` 都照常生效。

## 与假定不同的几处

- **不持久化脚本本身。** 脚本是内存里的 VM 状态；跨调用要保留的是 `store()` 里
  的小数据。持久化一次执行需要日志与重放，那是另一个量级的复杂度。
- **没有 `models.*`。** pi 的脚本还能调用分类器与图像模型；tom 的模型目录没有
  「分类器」这一类的概念。
- **不递归。** 脚本不能启动脚本——那会让「一次调用的成本」变成不可预测。
