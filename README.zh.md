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

- **终端工作区** — 分组管理多个终端，`ctrl-alt-shift-up` / `ctrl-alt-shift-down` 切换分组；按正确工作目录新建会话；分组支持自动平铺布局（manual / splits / tall / fat / grid / stack / horizontal / vertical，类似 kitty）；在分组右键菜单开启**广播输入**，一次键入同时发送到组内所有终端（`ctrl-alt-b`）
- **标签管理** — `cmd-shift-t`（`ctrl-shift-t`）重新打开最近关闭的终端，并恢复工作目录、Profile/SSH/tmux 启动方式、配色和字号；“将标签移到新窗口”会把运行中的终端（进程保留）移到新窗口
- **Quake 终端** — 全局热键 `ctrl+\`` 随处呼出的下拉终端，贴顶显示（macOS）
- **命令行** — 从脚本和编辑器驱动运行中的 Terry：`terry new-tab --cwd ~/src`、`terry send-text "cargo test"`、`terry run "cargo test"`，以及 `terry session save/load` 保存可复用工作流
- **Kittens / 插件** — 从 `kittens/` 目录发现可执行插件，用 `terry kitten list` / `terry kitten run NAME` 启动；插件会拿到 `TERRY_*` 上下文和自动化 API。注意这是 Terry 的插件协议，不是 kitty Python kitten API 的源码级兼容层
- **终端图像** — 支持 kitty graphics 协议与 sixel，终端内直接显示图片，GIF 可动画（`chafa` / `viu` / `icat` 可用）
- **终端协议** — kitty 键盘协议（CSI u，支持按下/重复/释放事件、备用键与关联文本）、OSC 52 剪贴板写入（读取需显式开启 `"terminal": { "clipboard_read": true }`）、OSC 99 桌面通知、OSC 22 鼠标指针形状、OSC 66 文本缩放、CSI 8 t 窗口尺寸调整，以及 Unicode grapheme 聚类（emoji ZWJ 序列、旗帜、keycap、组合字符）
- **粘贴安全** — 多行粘贴默认先确认；在弹窗中选择「粘贴，不再询问」即可关闭，也可用 `"terminal": { "confirm_multiline_paste": false }` 关闭
- **Shell 集成** — zsh / fish 开箱即用：提示符标记（OSC 133 / VS Code OSC 633）与工作目录跟踪（OSC 7）；`ctrl-shift-up` / `ctrl-shift-down` 在各条命令的提示符之间跳转
- **终端配置档** — 命名 Profile 可覆盖 Shell、环境变量与工作目录，并在 Shell 启动后执行命令（例如 `tmux` 或激活项目虚拟环境）；可在分组右键菜单中选择，或用 `default_profile` 设为默认
- **SSH 主机** — 从 `~/.ssh/config` 与 `terminal.ssh_hosts` 生成主机选择器；连接会复用 60 秒（`ControlMaster`），同一主机的第二个终端瞬间打开。「通过 tmux 连接」会在远端 attach 或新建 tmux 会话
- **tmux 会话** — 选择器可直接 attach 或新建本地 tmux 会话；远端等价于 `ssh … tmux new -A -s <名称>`。选择「原生分屏」后会通过 tmux **control mode**（`tmux -CC`）把每个 tmux window 映射成 Terry 分组、每个 pane 映射成原生分屏，输入、布局变化、窗口重命名与断线重连都会同步；不选原生分屏时会话仍运行在单个终端里
- **终端配色方案** — 内置配色库（Dracula、Nord、Solarized、Gruvbox、One Dark、Tokyo Night、Catppuccin、Rosé Pine 等），只改变终端颜色；可按终端、按 Profile 或用 `terminal.color_scheme` 选择。可从「终端」菜单导入 Alacritty `.toml`、iTerm2 `.itermcolors` 或 Terry JSON
- **会话恢复** — 重启后恢复分组、分屏布局、工作目录、标题、Profile 配色、每终端字号缩放，以及 Profile / SSH / tmux 启动方式
- **持久会话（可选）** — 开启 `terminal.persistent_sessions` 后，终端由后台 daemon 持有：退出 Terry 或关闭窗口后进程继续运行，重新打开时自动重连并回放 scrollback。关闭单个标签页会分离（首次会提示），可用「重新打开已关闭的终端」恢复，或运行 `terry daemon kill` 结束。目前仅支持 Unix
- **Hints** — `Ctrl+Shift+E` 列出可见的 URL、路径、git hash、UUID、IP 与自定义正则匹配；可键盘选择打开、复制、粘贴或选中。自定义规则写在 `terminal.hints`，例如 `{ "regex": "#\\d+", "action": "copy" }`
- **AI Agent** — 侧边栏与大模型对话，通过可配置的 Profile 调用命令与工具；侧边栏还会标记运行中的 coding agent 是「工作中 / 等待输入 / 空闲」（基于终端底部屏幕内容识别，支持 Claude、Codex、Gemini、OpenCode、Cursor、Qwen、Copilot、Grok、Kimi、Amp、Hermes、Qoder、Antigravity 等）
- **MCP** — 接入 Model Context Protocol 服务器，扩展 Agent 能力
- **文件面板** — 在终端旁浏览项目目录树
- **编辑器** — 从文件面板，或通过「新建文件 / 打开文件」打开文本，与终端共用标签页和分屏。默认开启 Vim 模式，支持文件内搜索和跳转到行；Markdown 可预览，图片可直接查看
- **设置与主题** — 自定义 Shell、外观、Agent 模型等
- **国际化** — 界面支持多语言（含中英文）

## Shell 集成

Terry 解析 **OSC 133** 提示符标记（含 VS Code 的 **OSC 633** 变体）与 **OSC 7** 工作目录上报。集成生效后，`ctrl-shift-up` / `ctrl-shift-down` 可在各条命令的提示符之间前后跳转；Terry 还会记录命令文本，用于选中输出或重新执行。

- **zsh 与 fish** 开箱即用。Terry 会把集成脚本写入数据目录（macOS 上为 `~/Library/Application Support/Terry/shell-integration`）并自动注入，无需修改任何 dotfile。在设置的 `terminal` 段中设置 `"shell_integration": false` 可关闭。
- **bash** 需手动启用 —— 在 `~/.bashrc` 中加一行：

  ```bash
  [ -n "$TERRY_INTEGRATION_DIR" ] && . "$TERRY_INTEGRATION_DIR/bash/terry.bash"
  ```

  （macOS 自带的 bash 即可；如果已加载 [bash-preexec](https://github.com/rcaloras/bash-preexec)，Terry 会自动接入它的钩子。）

以 `ssh` 启动的远程 shell 会被跳过；任务运行（task）不会注入集成。

## 命令行

运行中的 Terry 实例会监听本地 IPC 套接字（带 token 保护，连接信息在数据目录的 `ipc.json`）。`terry` 可执行文件同时充当客户端：

```sh
terry new-tab --cwd ~/src          # 在当前窗口新建标签页
terry send-text "cargo test"       # 向活动终端粘贴文本
terry send-text --terminal 7 "hi"  # 向指定终端粘贴文本（不执行）
terry run "cargo test"             # 粘贴文本并回车执行
terry session save "Work"          # 把当前分组保存为 session 模板
terry session load "Work"          # 从模板创建一个新分组
terry session list                 # 列出已保存模板
terry session delete "Work"        # 删除模板
terry agents                       # 列出 agent 终端及其状态（working/blocked/idle）
terry wait --state blocked         # 阻塞直到某个 agent 需要输入
terry wait --match-output "FAILED" # 阻塞直到终端最近的输出包含 FAILED
terry events                       # 订阅 agent 状态变化（SSE，Ctrl-C 退出）
terry kitten list                  # 列出已安装的 kitten/插件
terry kitten run hello --arg       # 运行 kitten（也可直接 terry kitten hello --arg）
terry kitten path                  # 显示插件搜索目录
terry install-cli                  # 把 terry/terry-daemon 安装到 ~/.local/bin
terry uninstall-cli                # 移除上述 shim
```

`send-text` 只粘贴不执行 —— 永远不会自动附加换行；`run` 会粘贴并提交。session 模板会保存分组内的终端、工作目录、profile/启动方式、平铺布局和广播设置。

在 Terry 打开的终端里，`terry` 与 `terry-daemon` 始终在 `PATH` 中：两者都指向当前运行的构建，且 `terry-daemon ...` 等价于 `terry daemon ...`。即使 Terry 从不在登录 shell `PATH` 里的应用包启动，也能直接使用。

要在 Terry 之外的终端里使用 CLI，运行 `terry install-cli`。它会把两个 shim 写入 `~/.local/bin`（可用 `--dir DIR` 或 `TERRY_CLI_DIR` 覆盖），并在需要时提示如何把该目录加入 `PATH`。如果目标位置已存在不是 Terry 创建的同名文件，它不会覆盖。

Kitten 是放在 `$TERRY_KITTEN_PATH`、Terry 配置目录下 `kittens/` 或数据目录下 `kittens/` 中的可执行文件（Unix 下需要 shebang 和可执行位）。插件继承 stdio，并获得 `TERRY_KITTEN_NAME`、`TERRY_KITTEN_DIR`、`TERRY_KITTEN_PATH`、`TERRY_PID`、`TERRY_IPC_URL`、`TERRY_IPC_TOKEN`、`TERRY_CONFIG_DIR`、`TERRY_DATA_DIR`，因此可以调用自动化 API 或 `terry` CLI。Kitten 是受信任的本地可执行文件，会拿到 IPC token，请只安装可信插件。这里不提供 kitty 的 Python kitten API；现有 kitten 需要按这个协议移植。

已有实例在运行时再次启动 `terry` 会聚焦现有窗口，而不是启动第二个进程，并在 stderr 打印它把启动转交给了哪个进程（pid）。如果要在旧实例仍打开时运行新编译的二进制，使用 `terry --new-instance` 强制启动新实例。

### 自动化 API

同一个带 bearer token 的本地服务器也接受 JSON 请求：

- `/api/list_sessions` — 分组、分组 id、布局、终端 id、标题、cwd 与当前命令
- `/api/focus_session` — 通过 `terminal_id` / `session_id` 聚焦终端
- `/api/focus_group` — 通过 `group_id` 切换分组
- `/api/send_keys` — 发送文本或 `ctrl-c` 等按键
- `/api/send_text` — 向 `terminal_id`（默认活动终端）粘贴文本，不执行
- `/api/run` — 粘贴文本并可选回车执行（`"enter": false` 关闭回车）
- `/api/read_screen` — 以纯文本读取终端内容（`lines` 限制为最近 N 行非空内容）
- `/api/screen` — 以带样式的单元格读取可见屏幕：颜色、粗体/斜体/下划线等属性、光标位置与形状、视口尺寸
- `/api/get_context` — 活动终端的标题、cwd、分组与命令，以及检测到的 agent 状态
- `/api/close_session` — 不弹确认直接关闭终端（不传 id 时关闭活动终端）
- `/api/detach_session`、`/api/reattach_session`、`/api/list_detached` — 将终端分离出分组、保持运行并重新挂载
- `/api/rename_session` — 设置或清除终端标题（`"title": null` 清除）
- `/api/new_group`、`/api/delete_group`（会终止组内终端）、`/api/rename_group` — 管理分组
- `/api/move_session` — 把终端移动到其他分组
- `/api/set_group_layout` — `manual`/`splits`、`tall`、`fat`、`grid`、`stack`、`horizontal` 或 `vertical`
- `/api/list_session_templates`、`/api/save_session_template`、`/api/load_session_template`、`/api/delete_session_template` — 可复用工作流模板
- `/api/set_broadcast` — 开启/关闭分组的广播输入
- `/api/scroll` — `top`、`bottom`、`page_up`、`page_down`、`line_up` 或 `line_down`
- `/api/set_font_size`、`/api/set_color_scheme` — 单终端显示覆盖（`null` 清除）
- `/api/notify` — 发送系统通知
- `/api/list_agent_states` — 正在运行的 agent 终端及其状态（`working`/`blocked`/`idle`）、标题、cwd 与是否退出
- `/api/wait_agent_state` — 阻塞直到 agent 进入指定状态（`state` 或 `states`、可选 `timeout_ms`；超时返回 408）
- `/api/wait_for_output` — 阻塞直到终端最近的输出匹配 `match`（子串）或 `regex`（正则）；`tail_lines` 控制搜索范围，`0` 为整个缓冲区
- `/api/events` — SSE 事件流：`snapshot`、`agent_state`、`terminal_added`、`terminal_removed`
- `/api/new_tab`、`/api/send_text`、`/api/focus`、`/api/new_window`

### 远程控制（手机 / 平板）

上面的自动化 API 只监听回环地址，令牌每次启动都会更换，所以本机之外访问不到。要让手机或平板控制这个实例，开启第二个监听器：

```json
{
  "terminal": {
    "remote_control": {
      "enabled": true,
      "bind": "0.0.0.0",
      "port": 47823
    }
  }
}
```

「设置 → 终端 → 远程控制」会列出可用的地址和访问令牌，把它们填进客户端即可。令牌保存在数据目录的 `remote-token`（权限 `0600`，首次使用时自动生成），重启后不变，因此只需配对一次；点「重新生成令牌」会让所有已配对设备立即失效。

这个端点是**明文 HTTP + Bearer 令牌**，等价于远程命令执行，请只通过私有网络访问：

- **Tailscale / WireGuard**（推荐）：手机加入同一个 tailnet，然后直接访问 `http://<这台机器的 tailnet IP>:47823`。设置页会列出探测到的地址。
- **只走本机 + `tailscale serve`**：把 `bind` 保持为 `127.0.0.1`，再运行 `tailscale serve --bg http://127.0.0.1:47823`，由 Tailscale 提供 TLS 和 `*.ts.net` 域名。
- 不要把端口直接映射到公网。

自检（把 `<地址>` 和 `<令牌>` 换成设置页显示的值）：

```sh
curl -s -X POST http://<地址>:47823/api/list_sessions \
  -H 'Authorization: Bearer <令牌>' -d '{}'
```

`/api/screen` 返回一帧可直接重绘的视口：

```json
{
  "terminal_id": 3, "rows": 24, "cols": 80,
  "alternate_screen": false, "display_offset": 0,
  "default_fg": "#d4d4d4", "default_bg": "#1e1e1e",
  "cursor": { "row": 4, "col": 10, "shape": "block", "visible": true },
  "lines": [
    [ { "col": 0, "text": "❯ ls", "fg": "#50fa7b", "attrs": ["bold"] } ]
  ]
}
```

每个 `lines` 元素对应一行，里面是按起始列（`col`）定位的样式片段；空白且无样式的单元格会省略，`fg`/`bg` 缺省表示使用 `default_fg`/`default_bg`，反显（inverse）已在服务端交换完成。宽字符（CJK/emoji）占两列，尾随的空位单元格会被跳过。

原本的回环监听器不受影响，`terry` CLI、kitten 与 MCP server 的行为保持不变。

## 命令输出

Shell 集成生效后，Terry 会跟踪每条命令的边界与退出码（`OSC 133;C/D`）：

- `cmd-alt-u`（macOS）/ `ctrl-alt-u`（Linux、Windows）把上一条命令的输出复制到剪贴板
- `cmd-alt-shift-u` / `ctrl-alt-shift-u` 选中上一条命令的输出
- `cmd-alt-shift-r` / `ctrl-alt-shift-r` 重新执行上一条命令
- 状态栏显示最近一条失败命令的退出码，如 `分组 · 标题 · exit 1`
- 可在「设置 → 终端」开启命令结束通知；OSC 9 进度会显示在标签页标题中
- 终端搜索（`cmd-f`）支持大小写敏感、整词匹配与正则模式

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
