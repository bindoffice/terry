# Terry

[English](./README.md)

**Terry** 把终端变成真正的工作区。  
分组管理 Shell、就近浏览文件，AI Agent 随时在侧边待命 —— 为天天泡在命令行的人而生。

## 预览

<p align="center">
  <img src="assets/images/1.jpg" alt="Terry 预览 1" width="800" />
</p>
<p align="center">
  <img src="assets/images/2.png" alt="Terry 预览 2" width="800" />
</p>
<p align="center">
  <img src="assets/images/3.png" alt="Terry 预览 3" width="800" />
</p>

## 功能特性

- **终端工作区** — 分组管理多个终端，按正确工作目录新建会话；分组支持自动平铺布局（手动 / tall / grid / stack，类似 kitty）；在分组右键菜单开启**广播输入**，一次键入同时发送到组内所有终端（`ctrl-alt-b`）
- **Quake 终端** — 全局热键 `ctrl+\`` 随处呼出的下拉终端，贴顶显示（macOS）
- **命令行** — 从脚本和编辑器驱动运行中的 Terry：`terry new-tab --cwd ~/src`、`terry send-text "cargo test"`
- **终端图像** — 支持 kitty graphics 协议与 sixel，终端内直接显示图片，GIF 可动画（`chafa` / `viu` / `icat` 可用）
- **终端协议** — kitty 键盘协议（CSI u）与 OSC 52 剪贴板读写
- **Shell 集成** — zsh / fish 开箱即用：提示符标记（OSC 133 / VS Code OSC 633）与工作目录跟踪（OSC 7）；`ctrl-shift-up` 跳转到上一条命令的提示符
- **链接跳转** — `Ctrl+Shift+E` 列出当前可见的所有链接（URL / 路径 / OSC 8），键盘选择打开
- **AI Agent** — 侧边栏与大模型对话，通过可配置的 Profile 调用命令与工具
- **MCP** — 接入 Model Context Protocol 服务器，扩展 Agent 能力
- **文件面板** — 在终端旁浏览项目目录树
- **编辑器** — 从文件面板，或通过「新建文件 / 打开文件」打开文本，与终端共用标签页和分屏。默认开启 Vim 模式，支持文件内搜索和跳转到行；Markdown 可预览，图片可直接查看
- **设置与主题** — 自定义 Shell、外观、Agent 模型等
- **国际化** — 界面支持多语言（含中英文）

## Shell 集成

Terry 解析 **OSC 133** 提示符标记（含 VS Code 的 **OSC 633** 变体）与 **OSC 7** 工作目录上报。集成生效后，`ctrl-shift-up` 可跳转到上一条命令的提示符。

- **zsh 与 fish** 开箱即用。Terry 会把集成脚本写入数据目录（macOS 上为 `~/Library/Application Support/Terry/shell-integration`）并自动注入，无需修改任何 dotfile。在设置的 `terminal` 段中设置 `"shell_integration": false` 可关闭。
- **bash** 需手动启用 —— 在 `~/.bashrc` 中加一行：

  ```bash
  [ -n "$TERRY_INTEGRATION_DIR" ] && . "$TERRY_INTEGRATION_DIR/bash/terry.bash"
  ```

  （需要 bash ≥ 4，Linux 发行版默认满足。）

以 `ssh` 启动的远程 shell 会被跳过；任务运行（task）不会注入集成。

## 命令行

运行中的 Terry 实例会监听本地 IPC 套接字（带 token 保护，连接信息在数据目录的 `terry/ipc.json`）。`terry` 可执行文件同时充当客户端：

```sh
terry new-tab --cwd ~/src          # 在当前窗口新建标签页
terry send-text "cargo test"       # 向活动终端粘贴文本
```

`send-text` 只粘贴不执行 —— 永远不会自动附加换行。

## 命令输出

Shell 集成生效后，Terry 会跟踪每条命令的边界与退出码（`OSC 133;C/D`）：

- `cmd-alt-u`（macOS）/ `ctrl-alt-u`（Linux、Windows）把上一条命令的输入与输出复制到剪贴板
- 状态栏显示最近一条失败命令的退出码，如 `分组 · 标题 · exit 1`
- 终端搜索（`cmd-f`）支持大小写敏感开关（macOS 为 `alt-cmd-c`，其他平台为 `alt-c`）

## 支持平台

| 平台 | 状态 |
|------|------|
| macOS（Apple Silicon / Intel） | 支持 |
| Linux（x86_64） | 支持 |
| Windows（x86_64） | 支持 |

发布包由 GitHub Actions 构建（`.github/workflows/release.yml`）。

## 快速开始

### 环境要求

- Rust **1.95.0**（见 `rust-toolchain.toml`）
- 平台构建依赖：CMake、C/C++ 工具链；Linux 还需常见的 X11/Wayland/fontconfig 等库

### 编译与运行

```bash
cargo run --release
```

配置与数据目录使用应用名 **Terry**（例如 Linux 上为 `~/.config/terry/`，macOS 上为 `~/Library/Application Support/Terry`）。

### 本地打包

```bash
# macOS
script/package-macos.sh

# Linux
script/package-linux.sh

# Windows（PowerShell）
.\script\package-windows.ps1
```

产物输出在 `target/release/`。

## 目录结构

```
src/                 # 应用入口、终端/文件面板、菜单
crates/              # 共享库（GPUI、终端、Agent、设置等）
agent_ui / crates/agent_ui
assets/              # 图标、默认设置、主题
script/              # 打包脚本
resources/           # 应用图标与桌面元数据
```

## 许可证

- 应用包：**GPL-3.0-or-later**（见 `LICENSE-GPL`）
- 许多 crate 保留上游许可证（含 Apache-2.0；见 `LICENSE-APACHE` 及各 crate 元数据）

## 参与贡献

欢迎提 Issue 与 Pull Request。请保持改动聚焦，遵循现有代码风格，并在你改动的目标平台上验证。
