(() => {
  const translations = {
    zh: {
      htmlLang: "zh-CN",
      title: "Terry — 终端工作区与 AI Agent",
      description:
        "Terry 把终端变成工作区：分组与平铺终端、SSH/tmux、Shell 集成、远程控制、配色库与可配置 AI Agent。",
      homeAria: "Terry 首页",
      navAria: "主导航",
      langAria: "语言",
      navWorkspace: "工作区",
      navSessions: "会话",
      navTerminal: "终端",
      navAgent: "Agent",
      navCli: "CLI",
      navDownload: "下载",
      navBindoffice: "官网",
      brandAside: "和 Tom & Jerry 无关",
      heroTitle: "终端就是工作区。",
      heroLede:
        "分组终端、自动平铺、SSH 与 tmux、脚本化控制，再加上一个能显示工作状态的 AI Agent——为长期生活在命令行里的人打造。",
      ctaGet: "获取 Terry",
      ctaSource: "查看源码",
      heroBubble: "Codex 正在等待输入。点一下即可跳到对应终端。",
      heroAgentMeta: "3 working · 1 blocked · MCP on",
      workspaceEyebrow: "工作区",
      workspaceTitle: "终端分组，自动平铺。",
      workspaceBody:
        "把相关终端放进同一个组，用 ctrl-alt-shift-up / down 切换；新会话在正确的工作目录打开。kitty 风格的自动布局可随时切换，右键开启广播输入，一次键入所有终端。",
      workspaceChip1: "分组切换",
      workspaceChip2: "自动平铺布局",
      workspaceChip3: "广播输入 ctrl-alt-b",
      workspaceChip4: "按目录开会话",
      sessionsEyebrow: "会话",
      sessionsTitle: "关掉的标签，随时回来。",
      sessionsBody:
        "cmd-shift-t / ctrl-shift-t 恢复最近关闭的终端，cwd、profile、SSH/tmux、配色与字体缩放原样带回；Move Tab to New Window 把运行中的进程整体搬到新窗口。分组、布局、目录与标题随重启恢复；开启持久会话后，终端由后台守护进程持有，退出 Terry 也不中断。",
      sessionsChip1: "重命名分组与终端",
      sessionsChip2: "终端 Profile",
      sessionsChip3: "SSH hosts · 连接复用 60s",
      sessionsChip4: "tmux attach / native",
      sessionsChip5: "Quake 下拉终端 ctrl+`",
      shellEyebrow: "Shell 集成",
      shellTitle: "命令边界、输出与提示，全都看得见。",
      shellBody:
        "zsh 与 fish 开箱即用，bash 一行接入。Terry 解析 OSC 133 / 633 提示符标记与 OSC 7 工作目录，ctrl-shift-up / down 在提示符间跳转；复制、选中或重跑上一条命令的输出。Hints 用 Ctrl+Shift+E 列出 URL、路径、git hash、UUID、IP 和自定义正则。",
      shellChip1: "zsh · fish · bash",
      shellChip2: "OSC 133 / 633 · OSC 7",
      shellChip3: "复制命令输出",
      shellChip4: "失败退出码",
      terminalEyebrow: "终端",
      terminalTitle: "现代终端该有的协议与图像。",
      terminalBody:
        "kitty graphics 与 sixel 直接渲染图片和动图（chafa / viu / icat 可用）；kitty 键盘协议、OSC 52 剪贴板、OSC 99 通知、OSC 22 指针形状、OSC 66 文字尺寸与 Unicode 字素聚类一应俱全。多行粘贴默认确认，搜索支持大小写、整词与正则。",
      terminalChip1: "kitty graphics · sixel",
      terminalChip2: "CSI u",
      terminalChip3: "OSC 52 / 99 / 22 / 66",
      terminalChip4: "Unicode 字素",
      terminalChip5: "粘贴安全",
      terminalChip6: "正则搜索",
      themesEyebrow: "配色",
      themesTitle: "配色库，按终端切换。",
      themesBody:
        "Dracula、Nord、Solarized、Gruvbox、One Dark、Tokyo Night、Catppuccin、Rosé Pine……只改终端颜色。可按终端、按 Profile 或设置 terminal.color_scheme；从 Terminal 菜单导入 Alacritty .toml、iTerm2 .itermcolors 或 Terry JSON。",
      themesChip1: "内置配色库",
      themesChip2: "按终端 / Profile 应用",
      themesChip3: "导入 Alacritty / iTerm2",
      themesChip4: "设置 · 外观 · Agent 模型",
      themesChip5: "多语言界面",
      agentEyebrow: "Agent",
      agentTitle: "Agent 在侧，状态一目了然。",
      agentBody:
        "侧栏与 LLM 对话，用可配置 Profile 运行命令与工具；Terry 从终端屏幕底部识别 Claude、Codex、Gemini、OpenCode、Cursor、Qwen、Copilot 等编码 Agent 的工作 / 等待输入 / 空闲状态。通过 MCP 连接服务器扩展能力；文件面板与编辑器共享标签与分屏，Vim 模式、Markdown 预览与图片查看一并就绪。",
      agentChip1: "Agent 状态检测",
      agentChip2: "MCP",
      agentChip3: "文件面板",
      agentChip4: "编辑器 · Vim",
      agentChip5: "Markdown 预览",
      cliEyebrow: "CLI 与自动化",
      cliTitle: "用脚本驱动正在运行的 Terry。",
      cliBody:
        "本地 IPC 受 token 保护，terry 二进制同时是客户端：new-tab、send-text、run、session save/load、agents、wait、events、kitten。同样的能力也开放为 JSON 自动化 API 和插件（kittens）契约；手机或平板可通过 Tailscale 打开只绑私网的远程控制。",
      cliChip1: "本地 IPC · token",
      cliChip2: "JSON API",
      cliChip3: "SSE 事件",
      cliChip4: "kittens 插件",
      cliChip5: "远程控制 · Tailscale",
      downloadEyebrow: "下载",
      downloadTitle: "macOS · Linux · Windows",
      downloadBody:
        "发布包由 GitHub Actions 构建，覆盖 Apple Silicon / Intel、Linux x86_64 与 Windows x86_64。从 Releases 下载对应平台，或从源码编译（Rust 1.95）。",
      downloadChip1: "Rust 1.95",
      ctaReleases: "打开 Releases",
      footerNote: "GPL-3.0-or-later · 基于 GPUI 技术栈",
    },
    en: {
      htmlLang: "en",
      title: "Terry — Terminal workspace with an AI agent",
      description:
        "Terry turns your terminal into a workspace: grouped and tiled terminals, SSH/tmux, shell integration, remote control, color schemes, and a configurable AI agent.",
      homeAria: "Terry home",
      navAria: "Primary",
      langAria: "Language",
      navWorkspace: "Workspace",
      navSessions: "Sessions",
      navTerminal: "Terminal",
      navAgent: "Agent",
      navCli: "CLI",
      navDownload: "Download",
      navBindoffice: "Website",
      brandAside: "Not related to Tom & Jerry",
      heroTitle: "Your terminal, as a workspace.",
      heroLede:
        "Grouped terminals, automatic tiling, SSH and tmux, scriptable control, and an AI agent that shows what it's doing — built for people who live in the command line.",
      ctaGet: "Get Terry",
      ctaSource: "View source",
      heroBubble: "Codex is waiting for input. Click through to its terminal.",
      heroAgentMeta: "3 working · 1 blocked · MCP on",
      workspaceEyebrow: "Workspace",
      workspaceTitle: "Group terminals. Tile them automatically.",
      workspaceBody:
        "Put related terminals in a group and switch with ctrl-alt-shift-up / down; new sessions open in the right working directory. Kitty-style automatic layouts switch anytime, and broadcast input types into every terminal in the group at once.",
      workspaceChip1: "Group switching",
      workspaceChip2: "Automatic tiling",
      workspaceChip3: "Broadcast input · ctrl-alt-b",
      workspaceChip4: "Sessions in the right cwd",
      sessionsEyebrow: "Sessions",
      sessionsTitle: "Closed tabs come back.",
      sessionsBody:
        "cmd-shift-t / ctrl-shift-t reopens the most recently closed terminal with its cwd, profile, SSH/tmux launch, colors, and font zoom. Move Tab to New Window takes a running process with it. Groups, layouts, directories, and titles restore on restart; with persistent sessions, a background daemon owns your terminals so they survive quitting Terry.",
      sessionsChip1: "Rename groups and terminals",
      sessionsChip2: "Terminal profiles",
      sessionsChip3: "SSH hosts · 60s connection reuse",
      sessionsChip4: "tmux attach / native",
      sessionsChip5: "Quake drop-down · ctrl+`",
      shellEyebrow: "Shell integration",
      shellTitle: "Command boundaries, output, and hints in view.",
      shellBody:
        "zsh and fish work out of the box; bash takes one line. Terry parses OSC 133 / 633 prompt marks and OSC 7 working directories, so ctrl-shift-up / down jump between prompts and you can copy, select, or re-run the previous command's output. Hints (Ctrl+Shift+E) list URLs, paths, git hashes, UUIDs, IPs, and custom regexes.",
      shellChip1: "zsh · fish · bash",
      shellChip2: "OSC 133 / 633 · OSC 7",
      shellChip3: "Copy command output",
      shellChip4: "Failed exit codes",
      terminalEyebrow: "Terminal",
      terminalTitle: "The protocols and images a modern terminal needs.",
      terminalBody:
        "Kitty graphics and sixel render images and animated GIFs right in the terminal (chafa / viu / icat work). The kitty keyboard protocol, OSC 52 clipboard, OSC 99 notifications, OSC 22 pointer shapes, OSC 66 text sizing, and Unicode grapheme clustering are all here. Multi-line pastes ask for confirmation, and search supports case-sensitive, whole-word, and regex modes.",
      terminalChip1: "kitty graphics · sixel",
      terminalChip2: "CSI u",
      terminalChip3: "OSC 52 / 99 / 22 / 66",
      terminalChip4: "Unicode graphemes",
      terminalChip5: "Paste safety",
      terminalChip6: "Regex search",
      themesEyebrow: "Color schemes",
      themesTitle: "A scheme library, per terminal.",
      themesBody:
        "Dracula, Nord, Solarized, Gruvbox, One Dark, Tokyo Night, Catppuccin, Rosé Pine, and more — terminal colors only. Pick one per terminal, per profile, or with terminal.color_scheme; import Alacritty .toml, iTerm2 .itermcolors, or Terry JSON from the Terminal menu.",
      themesChip1: "Bundled library",
      themesChip2: "Per terminal / profile",
      themesChip3: "Import Alacritty / iTerm2",
      themesChip4: "Settings · appearance · models",
      themesChip5: "Localized UI",
      agentEyebrow: "Agent",
      agentTitle: "An agent beside you, with visible state.",
      agentBody:
        "Chat with LLMs in the sidebar and run commands and tools with configurable profiles. Terry detects Claude, Codex, Gemini, OpenCode, Cursor, Qwen, Copilot, and other coding agents as working, waiting for input, or idle from the bottom of the terminal. Connect MCP servers to extend it; the files panel and editor share tabs and splits, with Vim mode, Markdown preview, and an image viewer.",
      agentChip1: "Agent state detection",
      agentChip2: "MCP",
      agentChip3: "Files panel",
      agentChip4: "Editor · Vim",
      agentChip5: "Markdown preview",
      cliEyebrow: "CLI & automation",
      cliTitle: "Script a running Terry.",
      cliBody:
        "A token-protected local IPC socket makes the terry binary a client: new-tab, send-text, run, session save/load, agents, wait, events, kitten. The same abilities are exposed as a JSON automation API and a plugin (kittens) contract; phones and tablets can drive an instance over a private-only remote control listener via Tailscale.",
      cliChip1: "Local IPC · token",
      cliChip2: "JSON API",
      cliChip3: "SSE events",
      cliChip4: "kittens plugins",
      cliChip5: "Remote control · Tailscale",
      downloadEyebrow: "Download",
      downloadTitle: "macOS · Linux · Windows",
      downloadBody:
        "Release packages are built by GitHub Actions for Apple Silicon / Intel, Linux x86_64, and Windows x86_64. Download for your platform from Releases, or build from source with Rust 1.95.",
      downloadChip1: "Rust 1.95",
      ctaReleases: "Open Releases",
      footerNote: "GPL-3.0-or-later · Built on the GPUI stack",
    },
  };

  const storageKey = "terry-website-lang";

  function detectLang() {
    const saved = localStorage.getItem(storageKey);
    if (saved === "zh" || saved === "en") return saved;
    const nav = (navigator.language || "en").toLowerCase();
    return nav.startsWith("zh") ? "zh" : "en";
  }

  function applyLang(lang) {
    const dict = translations[lang] || translations.en;
    document.documentElement.lang = dict.htmlLang;
    document.title = dict.title;

    const meta = document.querySelector('meta[name="description"]');
    if (meta) meta.setAttribute("content", dict.description);

    document.querySelectorAll("[data-i18n]").forEach((el) => {
      const key = el.getAttribute("data-i18n");
      if (key && dict[key] != null) el.textContent = dict[key];
    });

    document.querySelectorAll("[data-i18n-aria]").forEach((el) => {
      const key = el.getAttribute("data-i18n-aria");
      if (key && dict[key] != null) el.setAttribute("aria-label", dict[key]);
    });

    document.querySelectorAll(".lang-btn").forEach((btn) => {
      const active = btn.getAttribute("data-lang") === lang;
      btn.setAttribute("aria-pressed", active ? "true" : "false");
      btn.classList.toggle("is-active", active);
    });

    localStorage.setItem(storageKey, lang);
  }

  const initialLang = detectLang();
  applyLang(initialLang);

  document.querySelectorAll(".lang-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const lang = btn.getAttribute("data-lang");
      if (lang === "zh" || lang === "en") applyLang(lang);
    });
  });

  const prefersReduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  const reveals = document.querySelectorAll(".reveal");
  if (!prefersReduced && "IntersectionObserver" in window) {
    const io = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            entry.target.classList.add("is-visible");
            io.unobserve(entry.target);
          }
        }
      },
      { threshold: 0.18, rootMargin: "0px 0px -8% 0px" }
    );
    reveals.forEach((el) => io.observe(el));
  } else {
    reveals.forEach((el) => el.classList.add("is-visible"));
  }

  const stage = document.querySelector(".hero-stage");
  if (stage && !prefersReduced) {
    const shell = stage.querySelector(".product-shell");
    let raf = 0;
    const onMove = (event) => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const rect = stage.getBoundingClientRect();
        const x = (event.clientX - rect.left) / rect.width - 0.5;
        const y = (event.clientY - rect.top) / rect.height - 0.5;
        shell.style.transform = `perspective(1200px) rotateY(${x * 4}deg) rotateX(${-y * 3}deg) translateY(${y * -6}px)`;
      });
    };
    const reset = () => {
      shell.style.transform = "";
    };
    stage.addEventListener("pointermove", onMove);
    stage.addEventListener("pointerleave", reset);
  }
})();
