# 资源包作者指南

资源包把 **skills / prompts / themes / extensions** 打成一个可安装单元，用户不需要
Rust 工具链。它是 `tom install`（预编译 cdylib 扩展）的补充，不是替代。

- 要分发 **Rust 动态库扩展** → `tom install` + `tom-extension.json`
- 要分发 **资源和配置** → 资源包（本文）
- 两者都要 → 在一个资源包里放 `extensions/`，扩展本身仍按 `tom install` 的流程发布

## 最小结构

```
my-pack/
├── tom-package.json
├── skills/
│   └── review/SKILL.md
├── prompts/
│   └── summarize.md
├── themes/
│   └── ocean.json
└── extensions/
    └── libmy_extension.so
```

不写清单时，这四个目录会被自动发现。推荐仍然写清单：它让包名、版本和资源位置明确，
也能把资源放到任意子目录。

## 清单

`tom-package.json`：

```json
{
  "name": "my-pack",
  "version": "1.0.0",
  "skills": ["./resources/skills"],
  "prompts": ["prompts/*.md"],
  "themes": ["./themes"],
  "extensions": ["./lib"]
}
```

也接受嵌套写法 `{ "tom": { "skills": [...] } }`，与 `package.json` 的形状一致。

`package.json`（npm 包）：

```json
{
  "name": "my-pack",
  "version": "1.0.0",
  "tom": { "skills": ["./resources/skills"] }
}
```

键名优先级：`tom` > `pi`。同时提供两者时只用 `tom`——一个包对目标宿主的表态应当
唯一。只有 `pi` 的现成包可以直接安装，无需改写。

数组元素支持：

- 目录或文件路径（相对包根，`./` 可省）
- 通配：`prompts/*.md`、`resources/*/skills`

声明了但不存在路径不会让安装失败（那是打包错误，不是运行错误），但
`tom package add` 会提示包没有贡献任何资源——通常意味着清单路径写错了。

## 发布到 npm

```bash
npm publish            # name 与 version 取 package.json
tom package add npm:my-pack
tom package add npm:my-pack@1.0.0    # 固定版本
```

`tom` 会：

1. 从 registry 取包文档（`TOM_NPM_REGISTRY` 可指向镜像）
2. 本地解析版本范围，而不是直接信 `latest`
3. 校验 `dist.integrity`（sha512/sha256）或 `dist.shasum`，不匹配就拒绝解包
4. 解包到临时目录、写 `.tom-managed.json`、再改名到位

不要依赖 `postinstall` 之类的生命周期脚本：`tom` 不解包也不执行它们，只取 tarball
里的文件。

## 发布到 git

```bash
tom package add git:github.com/me/my-pack#v1.0.0
```

浅克隆后记录解析出的 commit。`#v1.0.0` 这类 tag/分支可以浅取；40 位或 7 位以上的
commit 哈希会在克隆后 checkout。`.git` 与 `node_modules` 不会进入安装目录。

## 与手写目录的边界

安装目录里始终有 `.tom-managed.json`，记录来源、版本与校验值。因此：

- `tom package remove` 只删带标记、且来源匹配的目录
- 覆盖一个没有标记的目录会被拒绝，除非 `--force`
- 手工维护的目录用 `tom package adopt <dir>` 接管

这条规则的存在是为了让「误删用户手写内容」不可能发生，代价是接管多一步。

## 优先级

资源发现顺序（同名 first-registration-wins）：

1. 项目包（`./.tom/packages`）
2. 项目 `.tom/<kind>`
3. 用户包（`<agent>/packages`）
4. 用户 `<agent>/<kind>`

即：**项目覆盖全局，显式包覆盖同名散装文件**。

## 检查清单

- [ ] `tom-package.json` 的 `name` 与可执行目录名一致（否则用 `--name`）
- [ ] 声明的路径都存在，且 `tom package add` 打印出资源条数
- [ ] npm 包已发布，`integrity` 字段由 registry 生成（不要手写）
- [ ] 不在包里放密钥：安装后的资源会被模型读取
- [ ] 扩展（`extensions/`）仍需自行按 `tom install` 发布，你要说明前置条件
