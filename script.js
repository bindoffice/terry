(() => {
  const translations = {
    zh: {
      htmlLang: "zh-CN",
      title: "Terry — 终端优先的 AI 工作区",
      description:
        "Terry 是以终端为中心的桌面工作区，内置 AI Agent。分组终端、文件面板与可配置 Agent，专注命令行与智能体工作流。",
      homeAria: "Terry 首页",
      navAria: "主导航",
      langAria: "语言",
      navWorkspace: "工作区",
      navNaming: "命名",
      navSplit: "分屏",
      navPerformance: "性能",
      navThemes: "主题",
      navAgent: "Agent",
      navDownload: "下载",
      navBindoffice: "官网",
      brandAside: "和 Tom & Jerry 无关",
      heroTitle: "终端在前，Agent 在侧。",
      heroLede:
        "以终端为中心的桌面工作区。分组会话、项目文件与可配置 AI Agent，专注命令行与智能体工作流——不是又一个完整 IDE。",
      ctaGet: "获取 Terry",
      ctaSource: "查看源码",
      heroBubble: "检查 release 产物，确认 macOS / Linux / Windows 均已打包。",
      workspaceEyebrow: "工作区",
      workspaceTitle: "终端分组，按目录开会话。",
      workspaceBody:
        "管理多终端标签与分组，在正确工作目录新建会话；旁侧文件面板让项目树始终在手边。",
      namingEyebrow: "命名",
      namingTitle: "支持修改分组名、终端名。",
      namingBody:
        "为分组和终端自定义名称，会话列表更清晰；改名结果会随工作区一起保存，下次打开仍在。",
      namingItem1Label: "分组",
      namingItem2Label: "终端",
      namingItem3Label: "持久化",
      namingItem3Value: "随工作区保存",
      splitEyebrow: "分屏",
      splitTitle: "上下左右分屏，多终端同屏协作。",
      splitBody:
        "将当前窗格向右、向左、向上或向下拆分，并排查看日志与命令、前后端会话或远程与本地终端。布局会随终端分组一并保存。",
      perfEyebrow: "性能",
      perfTitle: "原生性能，开箱即快。",
      perfBody:
        "基于 Rust 与 GPUI 构建，GPU 加速界面与高效终端渲染，启动快、滚动顺、多会话也不拖泥带水。",
      perfItem1Label: "技术栈",
      perfItem1Value: "Rust · GPUI",
      perfItem2Label: "渲染",
      perfItem2Value: "GPU 加速界面",
      perfItem3Label: "手感",
      perfItem3Value: "顺滑滚动 · 轻量占用",
      themesEyebrow: "主题",
      themesTitle: "多种主题，随心切换。",
      themesBody:
        "内置多套外观主题，覆盖明暗风格与配色偏好；界面与终端外观可一起调整，找到最适合长时间工作的视觉节奏。",
      agentEyebrow: "Agent",
      agentTitle: "侧栏对话，直接驱动命令与工具。",
      agentBody:
        "用可配置 Profile 连接大模型；通过 MCP 扩展能力，让 Agent 在终端上下文里读写与执行。",
      downloadEyebrow: "下载",
      downloadTitle: "macOS · Linux · Windows",
      downloadBody:
        "发布包由 GitHub Actions 构建。从 Releases 下载对应平台，或自行从源码编译。",
      ctaReleases: "打开 Releases",
      footerNote: "GPL-3.0-or-later · 基于 GPUI 技术栈",
    },
    en: {
      htmlLang: "en",
      title: "Terry — Terminal-first AI workspace",
      description:
        "Terry is a terminal-centered desktop workspace with a built-in AI agent. Grouped terminals, a files panel, and configurable agents — focused on shell and agent workflows.",
      homeAria: "Terry home",
      navAria: "Primary",
      langAria: "Language",
      navWorkspace: "Workspace",
      navNaming: "Naming",
      navSplit: "Split",
      navPerformance: "Performance",
      navThemes: "Themes",
      navAgent: "Agent",
      navDownload: "Download",
      navBindoffice: "Website",
      brandAside: "Not related to Tom & Jerry",
      heroTitle: "Terminal first. Agent beside.",
      heroLede:
        "A terminal-centered desktop workspace. Grouped sessions, project files, and a configurable AI agent — built for shell and agent workflows, not another full IDE.",
      ctaGet: "Get Terry",
      ctaSource: "View source",
      heroBubble: "Check release artifacts for macOS, Linux, and Windows.",
      workspaceEyebrow: "Workspace",
      workspaceTitle: "Group terminals. Open sessions in the right directory.",
      workspaceBody:
        "Manage tabs and groups, spawn sessions with the correct working directory, and keep the project tree beside your shell.",
      namingEyebrow: "Naming",
      namingTitle: "Rename groups and terminals.",
      namingBody:
        "Give groups and terminals custom names so the session list stays clear. Renames persist with the workspace and come back on next launch.",
      namingItem1Label: "group",
      namingItem2Label: "terminal",
      namingItem3Label: "persist",
      namingItem3Value: "saved with workspace",
      splitEyebrow: "Split",
      splitTitle: "Split panes in any direction.",
      splitBody:
        "Split the active pane right, left, up, or down — watch logs next to commands, pair frontend and backend shells, or keep remote and local sessions side by side. Layouts persist with each terminal group.",
      perfEyebrow: "Performance",
      perfTitle: "Native speed. Ready out of the box.",
      perfBody:
        "Built with Rust and GPUI for a GPU-accelerated UI and efficient terminal rendering — fast startup, smooth scrolling, and many sessions without the lag.",
      perfItem1Label: "stack",
      perfItem1Value: "Rust · GPUI",
      perfItem2Label: "render",
      perfItem2Value: "GPU-accelerated UI",
      perfItem3Label: "feel",
      perfItem3Value: "snappy scroll · light memory",
      themesEyebrow: "Themes",
      themesTitle: "Many themes. Switch anytime.",
      themesBody:
        "Choose from built-in light and dark themes. Tune UI and terminal appearance together until the look fits long coding sessions.",
      agentEyebrow: "Agent",
      agentTitle: "Chat in the sidebar. Drive commands and tools.",
      agentBody:
        "Connect models with configurable profiles. Extend capabilities through MCP so the agent can read, write, and run in your terminal context.",
      downloadEyebrow: "Download",
      downloadTitle: "macOS · Linux · Windows",
      downloadBody:
        "Release packages are built by GitHub Actions. Download for your platform from Releases, or build from source.",
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
