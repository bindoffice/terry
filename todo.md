下面是一个完整的开发提示词，你可以直接给开发团队或用于 AI 编程助手（如 Claude、Cursor），来驱动注意力系统、会话持久化、MCP Server 这三项功能的实现。

---

## 项目：Ink 三件套功能实现提示词

### 背景
Ink 是一个基于 GPUI + Alacritty 终端的跨平台（macOS / Linux / Windows）终端工作台，现有分组侧栏 + 中心 tabs 结构。  
我们要一次性实现三个核心特性，让 Ink 从「多终端编辑器」进化为「可信赖的多终端中枢 + AI agent 可操控平台」。

### 一、总体目标
1. **注意力系统**：未读标记、通知、一键跳转到需要关注的终端  
2. **会话持久化**：重启后恢复分组、标签、工作目录、滚动回溯  
3. **MCP Server**：独立的 `ink-mcp-server` 子进程，通过 HTTP localhost 与 Ink 主进程 IPC，对外暴露标准 MCP 协议，使任何 AI 客户端（Claude Desktop、Zed AI 等）都能操控 Ink 终端

### 二、实现约束
- **跨平台**：macOS / Linux / Windows 必须完全可用，不能使用 Unix socket 等非可移植 IPC
- **现有技术栈**：Rust 语言，GPUI 框架，Alacritty 终端核心
- **MCP Server 独立性**：独立子进程，可独立升级 MCP 协议版本，不与编辑器主进程强耦合
- **最小化依赖**：尽量使用标准库和已有库（如 `serde`, `tokio`, `rmcp` 或自行实现简易 MCP）

---

## 功能详细规格

### 1. 注意力系统 (Attention System)

**UI 表现：**
- 侧栏中每个分组/标签显示未读标记（圆点 / 数字 badge）
- 当后台终端产生新输出或满足特定条件时，对应 tab/分组高亮
- 桌面通知：当有 OSC 777 序列（或用户自定义触发器）时，弹出系统通知
- 全局快捷键（如 `Ctrl+Shift+U` / `Cmd+Shift+U`）跳转到最新未读会话

**技术实现：**
- 终端输出监控：利用 Alacritty 的 `Term` 事件或 `Handler`，在每次输出时通知前端
- 通知触发条件（最小闭环）：
  - 任何后台终端产生输出（可配置关闭）
  - OSC 99 / OSC 777 序列：`\x1b]99;urgency=1\x1b\\` 或 `\x1b]777;notify;message\x1b\\`
  - 可扩展：通过 MCP 工具 `ink_notify` 注入
- 侧栏组件需持有每个会话的未读计数/标志，并在渲染时显示
- 桌面通知使用跨平台库（如 `notify-rust`），保证三个平台行为一致
- 跳转功能：查找带未读标记的最高优先级会话，调用现有 `focus` 接口

**注意事项：**
- 当用户聚焦到会话时，清除未读标记
- 通知消息需可被系统级通知设置控制（避免骚扰）

---

### 2. 会话持久化 (Session Persistence)

**要持久化的内容：**
- 窗口位置、大小（可选，主要交给系统恢复）
- 所有分组 (groups) 和内部的标签 (tabs) 结构
- 每个终端标签的：
  - 工作目录 (cwd)
  - 环境变量快照 (可选)
  - 滚动回溯缓冲区 (scrollback，限制最大行数，如 10,000 行)
  - 当前运行的命令？不存，只恢复 cwd 和空 shell
- 会话元数据：git branch（若有）、端口信息？可由后续重启时重新探测，但存储 cwd 即可

**序列化格式：**
- 推荐使用 JSON + 可选压缩，保存到平台特定的数据目录（例如 `$XDG_DATA_HOME/ink/sessions/` 或 `%APPDATA%\ink\sessions\`）
- 保存时机：正常退出时、手动触发“保存会话”、定期自动保存（如每分钟）

**恢复逻辑：**
- 启动时检测持久化文件，若无异常，恢复分组和标签结构
- 为每个标签创建新的 PTY，设置 cwd
- 将保存的 scrollback 文本重新灌入终端显示（只读，不能回放命令）
- 窗口位置和大小可延迟实现（非紧急）

**跨平台要点：**
- 路径处理：使用 Rust 标准库 `dirs` crate 获取数据目录
- 文件权限：Windows 无需特别考虑，Unix 上设 `0o700`

---

### 3. MCP Server（独立进程 + HTTP localhost IPC）

#### 3.1 架构

- **Ink 主进程**：在内部启动一个 **HTTP server**，只监听 `127.0.0.1` 上的随机端口。提供 REST/JSON API 来执行命令和查询。每个请求需携带简单的 token 或随机口令，避免被本地其他进程滥用。
- **独立 `ink-mcp-server` 进程**：这是一个标准的 MCP server，使用 **stdio 传输**（也可以支持 HTTP+SSE，但初期用 stdio）。它把 MCP 请求转换成对 Ink 主进程 HTTP API 的调用，并将响应打包回 MCP 格式。
- **启动流程**：
  1. 用户启动 Ink，Ink 启动内部 HTTP server，生成随机 token，打印端口和 token 到 stdout 或写入临时文件（如 `~/.ink/ipc.json`）
  2. 用户（或 AI 客户端）启动 `ink-mcp-server`，该进程通过读取临时文件获得连接信息，然后连接主进程开始 MCP 服务
- **为什么这样设计**：
  - 主进程不直接理解 MCP，解耦
  - `ink-mcp-server` 可独立更新协议版本
  - 跨平台 HTTP 最稳妥，且便于调试

#### 3.2 Ink 主进程内部 HTTP API 设计

所有端点返回 JSON。请求体使用 JSON。

- `POST /api/list_sessions`  
  返回：分组树结构，每个终端的 `id`, `title`, `cwd`, `is_active`, `has_unread`

- `POST /api/focus`  
  参数：`{ "session_id": "uuid" }`  
  使 Ink 主窗口聚焦到该终端标签

- `POST /api/send_keys`  
  参数：`{ "session_id": "uuid", "text": "ls -la\n" }`  
  向目标终端发送按键（支持 ANSI 控制序列，实际写入 PTY）

- `POST /api/read_screen`  
  参数：`{ "session_id": "uuid", "max_lines": 200 }`  
  返回当前终端可视区域文本（带 ANSI 或纯文本，由参数决定）  
  实现：通过 Alacritty 的 `Grid` 提取可见行内容

- `POST /api/get_context`  
  参数：`{ "session_id": "uuid" }` 或省略返回当前活跃会话  
  返回：`cwd`, `git_branch`（若可探测），最近 N 行输出（如 20 行）

- `POST /api/notify`  
  参数：`{ "session_id": "uuid"? , "message": "编译完成" }`  
  触发注意力系统：侧栏 badge + 桌面通知 + 未读标记

- `POST /api/health`  
  返回服务状态

认证方式：每个请求携带 `Authorization: Bearer <token>`

#### 3.3 MCP Server 进程规范

**工具定义（初期七个，足以提供核心能力）：**

| 工具名 | 描述 | 参数 |
|--------|------|------|
| `list_sessions` | 列出所有终端会话 | 无 |
| `focus_session` | 聚焦到指定会话 | `session_id: string` |
| `send_keys` | 向会话发送按键 | `session_id: string`, `keys: string` |
| `read_screen` | 读取屏幕文本 | `session_id: string`, `raw_ansi: boolean?` |
| `get_context` | 获取会话上下文 | `session_id: string?` |
| `notify` | 发送通知 | `message: string`, `session_id?: string` |
| `create_new_terminal` | 创建新终端标签 | `cwd?: string`, `title?: string` |

**MCP 初始化消息举例（来自官方 spec）：**
```
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/list",
  "params": {}
}
```
处理逻辑：将请求转为 HTTP 调用 `/api/list_sessions`，或直接处理本进程内逻辑（如工具列表静态定义）。

**附加要求：**
- `list_sessions` 返回的每个对象中最好包含 `session_id` 和简短描述（标题、cwd）
- `read_screen` 去除 ANSI 颜色后返回纯文本（或可配置），方便 AI 阅读
- `send_keys` 需要能处理特殊键（如 Enter，Ctrl+C），可使用 ANSI 转义表示，如 `\x03` 代表 Ctrl+C，`\x0d` 代表 Enter，或提供便捷映射

#### 3.4 启动配置示例（给用户）

```json
// claude_desktop_config.json
{
  "mcpServers": {
    "ink": {
      "command": "ink-mcp-server",
      "args": ["--connect", "~/.ink/ipc.json"]
    }
  }
}
```
`ink-mcp-server` 读取 IPC 文件获取 `port` 和 `token`，然后启动 stdio 模式 MCP 服务。

---

## 实现顺序建议

1. **会话持久化**（基础设施，为后续恢复提供结构）  
   - 定义 Session/Group 数据结构  
   - 实现序列化与反序列化  
   - 修改启动逻辑加载，关闭时保存  
   - 确保跨平台路径

2. **注意力系统 UI + 逻辑**  
   - 在侧栏模型中加入未读计数和标记字段  
   - 终端输出回调中更新未读  
   - 实现桌面通知（notify-rust）  
   - 添加快捷键跳转  
   - OSC 序列解析（简单实现）

3. **Ink 内部 HTTP server**  
   - 使用 `hyper` 或 `actix-web` 轻量启动，监听 127.0.0.1 随机端口  
   - 实现所有 API 端点  
   - 认证 token 机制  
   - 向 stdout 打印连接信息（`INK_IPC_PORT=12345 INK_IPC_TOKEN=xyz`）或写文件

4. **独立 ink-mcp-server**  
   - 新建二进制 crate，使用 `tokio` + `rmcp`（或手动实现 MCP JSON-RPC）  
   - 加载连接信息，连接主 HTTP API  
   - 实现工具列表与调用转发  
   - 测试与 Claude Desktop 的集成

---

## 关键审查点
- 跨平台：HTTP IPC 在任何平台都工作，路径使用 `dirs`，终端操作不依赖系统特性
- 性能：读取屏幕（`/read_screen`）应是轻量操作，避免阻塞主渲染循环
- 安全：HTTP 必须仅绑定 127.0.0.1，token 随机强，避免 CSRF（无浏览器场景还好）
- 错误处理：API 返回标准错误码，MCP 转换器需优雅处理超时或主进程未启动

---

**请开始实现，优先保证跨平台可用性和最小可用产品（MVP），遇到架构疑问随时反馈。**
