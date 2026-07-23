# Terry

[English](./README.md)

**Terry** 是一款以终端为中心的桌面工作区，内置 AI Agent。  
基于 [Zed](https://zed.dev) 的 GPUI 技术栈，但聚焦于**终端、文件与 Agent 工作流**，而不是完整的 IDE。

## 功能特性

- **终端工作区** — 分组管理多个终端，按正确工作目录新建会话
- **AI Agent** — 侧边栏与大模型对话，通过可配置的 Profile 调用命令与工具
- **MCP** — 接入 Model Context Protocol 服务器，扩展 Agent 能力
- **文件面板** — 在终端旁浏览项目目录树
- **设置与主题** — 自定义 Shell、外观、Agent 模型等
- **国际化** — 界面支持多语言（含中英文）

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
- 平台构建依赖（与 Zed 同类）：CMake、C/C++ 工具链；Linux 还需常见的 X11/Wayland/fontconfig 等库

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

## 与 Zed 的关系

Terry 大量复用了 [Zed](https://github.com/zed-industries/zed) 编辑器的代码（GPUI、工作区、终端、Agent 基础设施等）。  
产品定位不同：面向 **轻量级终端 + Agent 工作区**，而非通用代码编辑器。

## 许可证

- 应用包：**GPL-3.0-or-later**（见 `LICENSE-GPL`）
- 许多 crate 保留上游许可证（含 Apache-2.0；见 `LICENSE-APACHE` 及各 crate 元数据）

## 参与贡献

欢迎提 Issue 与 Pull Request。请保持改动聚焦，遵循现有代码风格，并在你改动的目标平台上验证。
