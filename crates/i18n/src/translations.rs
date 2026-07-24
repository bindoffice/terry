//! Generated UI string tables.
use std::collections::HashMap;
use std::sync::LazyLock;

pub static TRANSLATIONS: LazyLock<HashMap<&'static str, HashMap<&'static str, &'static str>>> =
    LazyLock::new(|| {
        let mut locales = HashMap::new();
        locales.insert("en", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminals");
            m.insert("new_terminal", "New Terminal");
            m.insert("new_center_terminal", "New Center Terminal");
            m.insert("new_ellipsis", "New…");
            m.insert("new_file", "New File");
            m.insert("open_file", "Open File");
            m.insert("search_project", "Search Project");
            m.insert("search_symbols", "Search Symbols");
            m.insert("rename", "Rename");
            m.insert("move_up", "Move Up");
            m.insert("move_down", "Move Down");
            m.insert("close", "Close");
            m.insert("terminal_list", "Terminal List");
            m.insert("group", "Group");
            m.insert("new_group", "New Group");
            m.insert("delete_group", "Delete Group");
            m.insert("files", "Files");
            m.insert("up_one_level", "Up One Level");
            m.insert("refresh", "Refresh");
            m.insert("file_list", "File List");
            m.insert("ui_language", "Language");
            m.insert(
                "ui_language_description",
                "Interface language. Defaults to the system language.",
            );
            m.insert("language_system", "System");
            m.insert("appearance", "Appearance");
            m.insert("select_theme", "Select Theme…");
            m.insert("custom_shortcuts", "Custom Shortcuts");
            m.insert("keymap_settings", "Keyboard Shortcuts");
            m.insert(
                "keymap_settings_description",
                "View and customize keyboard shortcuts.",
            );
            m.insert("keymap_search_placeholder", "Search shortcuts…");
            m.insert("keymap_bindings_count", "shortcuts");
            m.insert("open_keymap_file", "Open Keymap File");
            m.insert("vim_mode", "Vim Mode");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Settings");
            m.insert("recent_terminals", "Recent Terminals");
            m.insert("recent_folders", "Recent Folders");
            m.insert("open_recent_project", "Open Recent Project");
            m.insert("llm_providers", "LLM Providers");
            m.insert(
                "llm_providers_description",
                "Configure API keys and settings for your AI providers.",
            );
            m.insert("agent", "Agent");
            m.insert("split_pane", "Split Pane");
            m.insert("split_right", "Split Right");
            m.insert("split_left", "Split Left");
            m.insert("split_up", "Split Up");
            m.insert("split_down", "Split Down");
            m.insert("zoom_in", "Zoom In");
            m.insert("zoom_out", "Zoom Out");
            m.insert("about_terry", "About Terry");
            m.insert("about_terry_description", "A terminal-focused workspace.");
            m.insert("ok", "OK");
            m.insert("services", "Services");
            m.insert("hide_terry", "Hide Terry");
            m.insert("hide_others", "Hide Others");
            m.insert("show_all", "Show All");
            m.insert("quit_terry", "Quit Terry");
            m.insert("menu_edit", "Edit");
            m.insert("menu_view", "View");
            m.insert("menu_window", "Window");
            m.insert("undo", "Undo");
            m.insert("redo", "Redo");
            m.insert("cut", "Cut");
            m.insert("copy", "Copy");
            m.insert("paste", "Paste");
            m.insert("paste_text", "Paste Text");
            m.insert("select_all", "Select All");
            m.insert("clear", "Clear");
            m.insert("inline_assist", "Inline Assist");
            m.insert("add_to_agent_thread", "Add to Agent Thread");
            m.insert("close_terminal_tab", "Close Terminal Tab");
            m.insert("toggle_left_dock", "Toggle Left Dock");
            m.insert("toggle_right_dock", "Toggle Right Dock");
            m.insert("toggle_bottom_dock", "Toggle Bottom Dock");
            m.insert("toggle_all_docks", "Toggle All Docks");
            m.insert("terminal_panel", "Terminal Panel");
            m.insert("command_palette", "Command Palette…");
            m.insert("minimize", "Minimize");
            m.insert("zoom", "Zoom");
            m.insert("toggle_full_screen", "Toggle Full Screen");
            m.insert("visit_the", "Visit the");
            m.insert("provider_dashboard", "{provider} dashboard");
            m.insert("to_generate_api_key", "to generate an API key.");
            m.insert(
                "or_set_env_var",
                "Or set the {env_var} environment variable and restart for it to take effect.",
            );
            m.insert("save", "Save");
            m.insert("reset_key", "Reset Key");
            m.insert("api_key_set_in_env", "API key set in environment variable");
            m.insert("api_key_configured", "API key configured");
            m.insert(
                "reset_api_key_env_hint",
                "To reset your API key, unset the {env_var} environment variable.",
            );
            m.insert("paste_api_key", "Paste your API key…");
            m.insert("to_find_api_key", "To find an API key, visit the");
            m.insert("provider_dashboard_dot", "provider dashboard.");
            m.insert("get_started", "Get Started");
            m.insert("open_project", "Open Project");
            m.insert("open_command_palette", "Open Command Palette");
            m.insert("configure", "Configure");
            m.insert("open_settings", "Open Settings");
            m.insert("customize_keymaps", "Customize Keymaps");
            m.insert("explore_extensions", "Explore Extensions");
            m.insert("welcome_to_terry", "Welcome to Terry");
            m.insert("welcome_back_to_terry", "Welcome back to Terry");
            m.insert("terry_tagline", "The terminal workspace for what's next");
            m.insert("return_to_onboarding", "Return to Onboarding");
            m.insert("collaborate_with_agents", "Collaborate with Agents");
            m.insert("open_agent_panel", "Open Agent Panel");
            m.insert(
                "agent_card_description",
                "Run multiple threads at once, mix and match any ACP-compatible agent, and keep work conflict-free with worktrees.",
            );
            m
        });
        locales.insert("zh-CN", {
            let mut m = HashMap::new();
            m.insert("terminals", "终端");
            m.insert("new_terminal", "新建终端");
            m.insert("new_center_terminal", "新建中间终端");
            m.insert("new_ellipsis", "新建…");
            m.insert("new_file", "新建文件");
            m.insert("open_file", "打开文件");
            m.insert("search_project", "搜索项目");
            m.insert("search_symbols", "搜索符号");
            m.insert("rename", "重命名");
            m.insert("move_up", "上移");
            m.insert("move_down", "下移");
            m.insert("close", "关闭");
            m.insert("terminal_list", "终端列表");
            m.insert("group", "分组");
            m.insert("new_group", "新建分组");
            m.insert("delete_group", "删除分组");
            m.insert("files", "文件");
            m.insert("up_one_level", "上级目录");
            m.insert("refresh", "刷新");
            m.insert("file_list", "文件列表");
            m.insert("ui_language", "语言");
            m.insert("ui_language_description", "界面语言。默认跟随系统语言。");
            m.insert("language_system", "跟随系统");
            m.insert("appearance", "外观");
            m.insert("select_theme", "选择主题…");
            m.insert("custom_shortcuts", "自定义快捷键");
            m.insert("keymap_settings", "键盘快捷键");
            m.insert("keymap_settings_description", "查看和自定义键盘快捷键。");
            m.insert("keymap_search_placeholder", "搜索快捷键…");
            m.insert("keymap_bindings_count", "个快捷键");
            m.insert("open_keymap_file", "打开快捷键配置文件");
            m.insert("vim_mode", "Vim 模式");
                        m.insert("menu_file", "文件");
            m.insert("open", "打开…");
            m.insert("open_recent", "打开最近项目…");
            m.insert("add_folder_to_project", "将文件夹添加到项目…");
            m.insert("close_window", "关闭窗口");
            m.insert("settings", "设置");
            m.insert("recent_terminals", "最近终端");
            m.insert("recent_folders", "最近文件夹");
            m.insert("open_recent_project", "打开最近项目");
            m.insert("llm_providers", "大模型服务商");
            m.insert(
                "llm_providers_description",
                "配置 AI 服务商的 API Key 和相关设置。",
            );
            m.insert("agent", "Agent");
            m.insert("split_pane", "分屏");
            m.insert("split_right", "向右分屏");
            m.insert("split_left", "向左分屏");
            m.insert("split_up", "向上分屏");
            m.insert("split_down", "向下分屏");
            m.insert("zoom_in", "放大");
            m.insert("zoom_out", "缩小");
            m.insert("about_terry", "关于 Terry");
            m.insert("about_terry_description", "以终端为中心的工作区。");
            m.insert("ok", "好");
            m.insert("services", "服务");
            m.insert("hide_terry", "隐藏 Terry");
            m.insert("hide_others", "隐藏其他");
            m.insert("show_all", "全部显示");
            m.insert("quit_terry", "退出 Terry");
            m.insert("menu_edit", "编辑");
            m.insert("menu_view", "显示");
            m.insert("menu_window", "窗口");
            m.insert("undo", "撤销");
            m.insert("redo", "重做");
            m.insert("cut", "剪切");
            m.insert("copy", "拷贝");
            m.insert("paste", "粘贴");
            m.insert("paste_text", "粘贴纯文本");
            m.insert("select_all", "全选");
            m.insert("clear", "清屏");
            m.insert("inline_assist", "行内助手");
            m.insert("add_to_agent_thread", "添加到 Agent 会话");
            m.insert("close_terminal_tab", "关闭终端标签");
            m.insert("toggle_left_dock", "切换左侧停靠栏");
            m.insert("toggle_right_dock", "切换右侧停靠栏");
            m.insert("toggle_bottom_dock", "切换底部停靠栏");
            m.insert("toggle_all_docks", "切换全部停靠栏");
            m.insert("terminal_panel", "终端面板");
            m.insert("command_palette", "命令面板…");
            m.insert("minimize", "最小化");
            m.insert("zoom", "缩放");
            m.insert("toggle_full_screen", "切换全屏");
            m.insert("visit_the", "请访问");
            m.insert("provider_dashboard", "{provider} 控制台");
            m.insert("to_generate_api_key", "以生成 API Key。");
            m.insert(
                "or_set_env_var",
                "或设置环境变量 {env_var} 并重启后生效。",
            );
            m.insert("save", "保存");
            m.insert("reset_key", "重置密钥");
            m.insert("api_key_set_in_env", "API Key 已通过环境变量设置");
            m.insert("api_key_configured", "已配置 API Key");
            m.insert(
                "reset_api_key_env_hint",
                "要重置 API Key，请取消设置环境变量 {env_var}。",
            );
            m.insert("paste_api_key", "粘贴你的 API Key…");
            m.insert("to_find_api_key", "查找 API Key，请访问");
            m.insert("provider_dashboard_dot", "服务商控制台。");
            m.insert("get_started", "开始使用");
            m.insert("open_project", "打开项目");
            m.insert("open_command_palette", "打开命令面板");
            m.insert("configure", "配置");
            m.insert("open_settings", "打开设置");
            m.insert("customize_keymaps", "自定义快捷键");
            m.insert("explore_extensions", "浏览扩展");
            m.insert("welcome_to_terry", "欢迎使用 Terry");
            m.insert("welcome_back_to_terry", "欢迎回到 Terry");
            m.insert("terry_tagline", "面向下一步的终端工作区");
            m.insert("return_to_onboarding", "返回引导");
            m.insert("collaborate_with_agents", "与 Agent 协作");
            m.insert("open_agent_panel", "打开 Agent 面板");
            m.insert(
                "agent_card_description",
                "同时运行多个线程，自由组合 ACP 兼容 Agent，并用 worktree 避免冲突。",
            );
            m
        });
        locales.insert("zh-TW", {
            let mut m = HashMap::new();
            m.insert("terminals", "終端機");
            m.insert("new_terminal", "新增終端機");
            m.insert("new_center_terminal", "新增中間終端機");
            m.insert("new_ellipsis", "新增…");
            m.insert("new_file", "新增檔案");
            m.insert("open_file", "開啟檔案");
            m.insert("search_project", "搜尋專案");
            m.insert("search_symbols", "搜尋符號");
            m.insert("rename", "重新命名");
            m.insert("move_up", "上移");
            m.insert("move_down", "下移");
            m.insert("close", "關閉");
            m.insert("copy", "拷貝");
            m.insert("paste", "貼上");
            m.insert("paste_text", "貼上純文字");
            m.insert("select_all", "全選");
            m.insert("clear", "清除");
            m.insert("inline_assist", "行內助手");
            m.insert("add_to_agent_thread", "加入 Agent 工作階段");
            m.insert("close_terminal_tab", "關閉終端機標籤");
            m.insert("terminal_list", "終端機列表");
            m.insert("group", "分組");
            m.insert("new_group", "新增分組");
            m.insert("delete_group", "刪除分組");
            m.insert("files", "檔案");
            m.insert("up_one_level", "上一層");
            m.insert("refresh", "重新整理");
            m.insert("file_list", "檔案列表");
            m.insert("ui_language", "語言");
            m.insert("ui_language_description", "介面語言。預設跟隨系統語言。");
            m.insert("language_system", "跟隨系統");
            m.insert("appearance", "外觀");
            m.insert("select_theme", "選擇主題…");
                        m.insert("menu_file", "檔案");
            m.insert("open", "開啟…");
            m.insert("open_recent", "開啟最近專案…");
            m.insert("add_folder_to_project", "將資料夾加入專案…");
            m.insert("close_window", "關閉視窗");
            m.insert("settings", "設定");
            m.insert("open_recent_project", "開啟最近專案");
            m.insert("agent", "Agent");
            m.insert("split_pane", "分割窗格");
            m.insert("get_started", "開始使用");
            m.insert("open_project", "開啟專案");
            m.insert("configure", "設定");
            m.insert("welcome_to_terry", "歡迎使用 Terry");
            m.insert("welcome_back_to_terry", "歡迎回到 Terry");
            m.insert("terry_tagline", "面向下一步的終端機工作區");
            m.insert("recent_terminals", "最近終端機");
            m.insert("recent_folders", "最近資料夾");
            m
        });
        locales.insert("ja", {
            let mut m = HashMap::new();
            m.insert("terminals", "ターミナル");
            m.insert("new_terminal", "新しいターミナル");
            m.insert("new_center_terminal", "中央に新しいターミナル");
            m.insert("new_ellipsis", "新規…");
            m.insert("new_file", "新しいファイル");
            m.insert("open_file", "ファイルを開く");
            m.insert("search_project", "プロジェクトを検索");
            m.insert("search_symbols", "シンボルを検索");
            m.insert("rename", "名前を変更");
            m.insert("move_up", "上へ移動");
            m.insert("move_down", "下へ移動");
            m.insert("close", "閉じる");
            m.insert("copy", "コピー");
            m.insert("paste", "貼り付け");
            m.insert("paste_text", "テキストを貼り付け");
            m.insert("select_all", "すべて選択");
            m.insert("clear", "クリア");
            m.insert("inline_assist", "インラインアシスト");
            m.insert("add_to_agent_thread", "Agent スレッドに追加");
            m.insert("close_terminal_tab", "ターミナルタブを閉じる");
            m.insert("terminal_list", "ターミナル一覧");
            m.insert("group", "グループ");
            m.insert("new_group", "新しいグループ");
            m.insert("delete_group", "グループを削除");
            m.insert("files", "ファイル");
            m.insert("up_one_level", "上の階層へ");
            m.insert("refresh", "更新");
            m.insert("file_list", "ファイル一覧");
            m.insert("ui_language", "言語");
            m.insert(
                "ui_language_description",
                "インターフェースの言語。デフォルトはシステム言語です。",
            );
            m.insert("language_system", "システムに従う");
            m.insert("appearance", "外観");
            m.insert("select_theme", "テーマを選択…");
                        m.insert("menu_file", "ファイル");
            m.insert("open", "開く…");
            m.insert("open_recent", "最近のプロジェクトを開く…");
            m.insert("add_folder_to_project", "フォルダをプロジェクトに追加…");
            m.insert("close_window", "ウインドウを閉じる");
            m.insert("settings", "設定");
            m.insert("open_recent_project", "最近のプロジェクトを開く");
            m.insert("agent", "Agent");
            m.insert("split_pane", "ペインを分割");
            m.insert("get_started", "はじめに");
            m.insert("open_project", "プロジェクトを開く");
            m.insert("configure", "設定");
            m.insert("welcome_to_terry", "Terry へようこそ");
            m.insert("welcome_back_to_terry", "おかえりなさい");
            m.insert("terry_tagline", "次へ進むためのターミナルワークスペース");
            m.insert("recent_terminals", "最近のターミナル");
            m.insert("recent_folders", "最近のフォルダ");
            m
        });
        locales.insert("ko", {
            let mut m = HashMap::new();
            m.insert("terminals", "터미널");
            m.insert("new_terminal", "새 터미널");
            m.insert("new_center_terminal", "중앙에 새 터미널");
            m.insert("new_ellipsis", "새로 만들기…");
            m.insert("new_file", "새 파일");
            m.insert("open_file", "파일 열기");
            m.insert("search_project", "프로젝트 검색");
            m.insert("search_symbols", "심볼 검색");
            m.insert("rename", "이름 바꾸기");
            m.insert("move_up", "위로 이동");
            m.insert("move_down", "아래로 이동");
            m.insert("close", "닫기");
            m.insert("copy", "복사");
            m.insert("paste", "붙여넣기");
            m.insert("paste_text", "텍스트 붙여넣기");
            m.insert("select_all", "모두 선택");
            m.insert("clear", "지우기");
            m.insert("inline_assist", "인라인 어시스트");
            m.insert("add_to_agent_thread", "Agent 스레드에 추가");
            m.insert("close_terminal_tab", "터미널 탭 닫기");
            m.insert("terminal_list", "터미널 목록");
            m.insert("group", "그룹");
            m.insert("new_group", "새 그룹");
            m.insert("delete_group", "그룹 삭제");
            m.insert("files", "파일");
            m.insert("up_one_level", "상위 폴더");
            m.insert("refresh", "새로고침");
            m.insert("file_list", "파일 목록");
            m.insert("ui_language", "언어");
            m.insert(
                "ui_language_description",
                "인터페이스 언어입니다. 기본값은 시스템 언어입니다.",
            );
            m.insert("language_system", "시스템");
            m.insert("appearance", "모양");
            m.insert("select_theme", "테마 선택…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "설정");
            m.insert("open_recent_project", "최근 프로젝트 열기");
            m.insert("agent", "Agent");
            m.insert("split_pane", "창 분할");
            m.insert("get_started", "시작하기");
            m.insert("open_project", "프로젝트 열기");
            m.insert("configure", "구성");
            m.insert("welcome_to_terry", "Terry에 오신 것을 환영합니다");
            m.insert("welcome_back_to_terry", "다시 오신 것을 환영합니다");
            m.insert("terry_tagline", "다음을 위한 터미널 워크스페이스");
            m.insert("recent_terminals", "최근 터미널");
            m.insert("recent_folders", "최근 폴더");
            m
        });
        locales.insert("es", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminales");
            m.insert("new_terminal", "Nueva terminal");
            m.insert("terminal_list", "Lista de terminales");
            m.insert("group", "Grupo");
            m.insert("new_group", "Nuevo grupo");
            m.insert("files", "Archivos");
            m.insert("up_one_level", "Subir un nivel");
            m.insert("refresh", "Actualizar");
            m.insert("file_list", "Lista de archivos");
            m.insert("ui_language", "Idioma");
            m.insert(
                "ui_language_description",
                "Idioma de la interfaz. Por defecto usa el idioma del sistema.",
            );
            m.insert("language_system", "Sistema");
            m.insert("appearance", "Apariencia");
            m.insert("select_theme", "Seleccionar tema…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Ajustes");
            m.insert("open_recent_project", "Abrir proyecto reciente");
            m
        });
        locales.insert("fr", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminaux");
            m.insert("new_terminal", "Nouveau terminal");
            m.insert("terminal_list", "Liste des terminaux");
            m.insert("group", "Groupe");
            m.insert("new_group", "Nouveau groupe");
            m.insert("files", "Fichiers");
            m.insert("up_one_level", "Niveau supérieur");
            m.insert("refresh", "Actualiser");
            m.insert("file_list", "Liste des fichiers");
            m.insert("ui_language", "Langue");
            m.insert(
                "ui_language_description",
                "Langue de l'interface. Suit la langue du système par défaut.",
            );
            m.insert("language_system", "Système");
            m.insert("appearance", "Apparence");
            m.insert("select_theme", "Choisir un thème…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Réglages");
            m.insert("open_recent_project", "Ouvrir un projet récent");
            m
        });
        locales.insert("de", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminals");
            m.insert("new_terminal", "Neues Terminal");
            m.insert("terminal_list", "Terminal-Liste");
            m.insert("group", "Gruppe");
            m.insert("new_group", "Neue Gruppe");
            m.insert("files", "Dateien");
            m.insert("up_one_level", "Eine Ebene höher");
            m.insert("refresh", "Aktualisieren");
            m.insert("file_list", "Dateiliste");
            m.insert("ui_language", "Sprache");
            m.insert(
                "ui_language_description",
                "Oberflächensprache. Standardmäßig Systemsprache.",
            );
            m.insert("language_system", "System");
            m.insert("appearance", "Erscheinungsbild");
            m.insert("select_theme", "Design auswählen…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Einstellungen");
            m.insert("open_recent_project", "Zuletzt verwendetes Projekt öffnen");
            m
        });
        locales.insert("pt-BR", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminais");
            m.insert("new_terminal", "Novo terminal");
            m.insert("terminal_list", "Lista de terminais");
            m.insert("group", "Grupo");
            m.insert("new_group", "Novo grupo");
            m.insert("files", "Arquivos");
            m.insert("up_one_level", "Nível acima");
            m.insert("refresh", "Atualizar");
            m.insert("file_list", "Lista de arquivos");
            m.insert("ui_language", "Idioma");
            m.insert(
                "ui_language_description",
                "Idioma da interface. Por padrão, segue o idioma do sistema.",
            );
            m.insert("language_system", "Sistema");
            m.insert("appearance", "Aparência");
            m.insert("select_theme", "Selecionar tema…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Configurações");
            m.insert("open_recent_project", "Abrir projeto recente");
            m
        });
        locales.insert("ru", {
            let mut m = HashMap::new();
            m.insert("terminals", "Терминалы");
            m.insert("new_terminal", "Новый терминал");
            m.insert("terminal_list", "Список терминалов");
            m.insert("group", "Группа");
            m.insert("new_group", "Новая группа");
            m.insert("files", "Файлы");
            m.insert("up_one_level", "На уровень выше");
            m.insert("refresh", "Обновить");
            m.insert("file_list", "Список файлов");
            m.insert("ui_language", "Язык");
            m.insert(
                "ui_language_description",
                "Язык интерфейса. По умолчанию — язык системы.",
            );
            m.insert("language_system", "Системный");
            m.insert("appearance", "Оформление");
            m.insert("select_theme", "Выбрать тему…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Настройки");
            m.insert("open_recent_project", "Открыть недавний проект");
            m
        });
        locales.insert("ar", {
            let mut m = HashMap::new();
            m.insert("terminals", "الطرفيات");
            m.insert("new_terminal", "طرفية جديدة");
            m.insert("terminal_list", "قائمة الطرفيات");
            m.insert("group", "مجموعة");
            m.insert("new_group", "مجموعة جديدة");
            m.insert("files", "الملفات");
            m.insert("up_one_level", "المستوى الأعلى");
            m.insert("refresh", "تحديث");
            m.insert("file_list", "قائمة الملفات");
            m.insert("ui_language", "اللغة");
            m.insert(
                "ui_language_description",
                "لغة الواجهة. الافتراضي هو لغة النظام.",
            );
            m.insert("language_system", "النظام");
            m.insert("appearance", "المظهر");
            m.insert("select_theme", "اختر السمة…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "الإعدادات");
            m.insert("open_recent_project", "فتح مشروع حديث");
            m
        });
        locales.insert("hi", {
            let mut m = HashMap::new();
            m.insert("terminals", "टर्मिनल");
            m.insert("new_terminal", "नया टर्मिनल");
            m.insert("terminal_list", "टर्मिनल सूची");
            m.insert("group", "समूह");
            m.insert("new_group", "नया समूह");
            m.insert("files", "फ़ाइलें");
            m.insert("up_one_level", "एक स्तर ऊपर");
            m.insert("refresh", "रीफ़्रेश");
            m.insert("file_list", "फ़ाइल सूची");
            m.insert("ui_language", "भाषा");
            m.insert(
                "ui_language_description",
                "इंटरफ़ेस भाषा। डिफ़ॉल्ट रूप से सिस्टम भाषा।",
            );
            m.insert("language_system", "सिस्टम");
            m.insert("appearance", "दिखावट");
            m.insert("select_theme", "थीम चुनें…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "सेटिंग्स");
            m.insert("open_recent_project", "हालिया प्रोजेक्ट खोलें");
            m
        });
        locales.insert("it", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminali");
            m.insert("new_terminal", "Nuovo terminale");
            m.insert("terminal_list", "Elenco terminali");
            m.insert("group", "Gruppo");
            m.insert("new_group", "Nuovo gruppo");
            m.insert("files", "File");
            m.insert("up_one_level", "Livello superiore");
            m.insert("refresh", "Aggiorna");
            m.insert("file_list", "Elenco file");
            m.insert("ui_language", "Lingua");
            m.insert(
                "ui_language_description",
                "Lingua dell'interfaccia. Predefinita: lingua di sistema.",
            );
            m.insert("language_system", "Sistema");
            m.insert("appearance", "Aspetto");
            m.insert("select_theme", "Seleziona tema…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Impostazioni");
            m.insert("open_recent_project", "Apri progetto recente");
            m
        });
        locales.insert("nl", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminals");
            m.insert("new_terminal", "Nieuwe terminal");
            m.insert("terminal_list", "Terminallijst");
            m.insert("group", "Groep");
            m.insert("new_group", "Nieuwe groep");
            m.insert("files", "Bestanden");
            m.insert("up_one_level", "Eén niveau omhoog");
            m.insert("refresh", "Vernieuwen");
            m.insert("file_list", "Bestandenlijst");
            m.insert("ui_language", "Taal");
            m.insert(
                "ui_language_description",
                "Interfacetaal. Standaard volgt de systeemtaal.",
            );
            m.insert("language_system", "Systeem");
            m.insert("appearance", "Uiterlijk");
            m.insert("select_theme", "Thema kiezen…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Instellingen");
            m.insert("open_recent_project", "Recent project openen");
            m
        });
        locales.insert("tr", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminaller");
            m.insert("new_terminal", "Yeni terminal");
            m.insert("terminal_list", "Terminal listesi");
            m.insert("group", "Grup");
            m.insert("new_group", "Yeni grup");
            m.insert("files", "Dosyalar");
            m.insert("up_one_level", "Bir üst dizin");
            m.insert("refresh", "Yenile");
            m.insert("file_list", "Dosya listesi");
            m.insert("ui_language", "Dil");
            m.insert(
                "ui_language_description",
                "Arayüz dili. Varsayılan olarak sistem dilini kullanır.",
            );
            m.insert("language_system", "Sistem");
            m.insert("appearance", "Görünüm");
            m.insert("select_theme", "Tema seç…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Ayarlar");
            m.insert("open_recent_project", "Son projeyi aç");
            m
        });
        locales.insert("pl", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminale");
            m.insert("new_terminal", "Nowy terminal");
            m.insert("terminal_list", "Lista terminali");
            m.insert("group", "Grupa");
            m.insert("new_group", "Nowa grupa");
            m.insert("files", "Pliki");
            m.insert("up_one_level", "Poziom wyżej");
            m.insert("refresh", "Odśwież");
            m.insert("file_list", "Lista plików");
            m.insert("ui_language", "Język");
            m.insert(
                "ui_language_description",
                "Język interfejsu. Domyślnie język systemu.",
            );
            m.insert("language_system", "Systemowy");
            m.insert("appearance", "Wygląd");
            m.insert("select_theme", "Wybierz motyw…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Ustawienia");
            m.insert("open_recent_project", "Otwórz ostatni projekt");
            m
        });
        locales.insert("vi", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminal");
            m.insert("new_terminal", "Terminal mới");
            m.insert("terminal_list", "Danh sách terminal");
            m.insert("group", "Nhóm");
            m.insert("new_group", "Nhóm mới");
            m.insert("files", "Tệp");
            m.insert("up_one_level", "Lên một cấp");
            m.insert("refresh", "Làm mới");
            m.insert("file_list", "Danh sách tệp");
            m.insert("ui_language", "Ngôn ngữ");
            m.insert(
                "ui_language_description",
                "Ngôn ngữ giao diện. Mặc định theo ngôn ngữ hệ thống.",
            );
            m.insert("language_system", "Hệ thống");
            m.insert("appearance", "Giao diện");
            m.insert("select_theme", "Chọn chủ đề…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Cài đặt");
            m.insert("open_recent_project", "Mở dự án gần đây");
            m
        });
        locales.insert("th", {
            let mut m = HashMap::new();
            m.insert("terminals", "เทอร์มินัล");
            m.insert("new_terminal", "เทอร์มินัลใหม่");
            m.insert("terminal_list", "รายการเทอร์มินัล");
            m.insert("group", "กลุ่ม");
            m.insert("new_group", "กลุ่มใหม่");
            m.insert("files", "ไฟล์");
            m.insert("up_one_level", "ขึ้นหนึ่งระดับ");
            m.insert("refresh", "รีเฟรช");
            m.insert("file_list", "รายการไฟล์");
            m.insert("ui_language", "ภาษา");
            m.insert(
                "ui_language_description",
                "ภาษาของอินเทอร์เฟซ ค่าเริ่มต้นตามภาษาของระบบ",
            );
            m.insert("language_system", "ระบบ");
            m.insert("appearance", "ลักษณะ");
            m.insert("select_theme", "เลือกธีม…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "การตั้งค่า");
            m.insert("open_recent_project", "เปิดโปรเจกต์ล่าสุด");
            m
        });
        locales.insert("id", {
            let mut m = HashMap::new();
            m.insert("terminals", "Terminal");
            m.insert("new_terminal", "Terminal baru");
            m.insert("terminal_list", "Daftar terminal");
            m.insert("group", "Grup");
            m.insert("new_group", "Grup baru");
            m.insert("files", "File");
            m.insert("up_one_level", "Naik satu tingkat");
            m.insert("refresh", "Muat ulang");
            m.insert("file_list", "Daftar file");
            m.insert("ui_language", "Bahasa");
            m.insert(
                "ui_language_description",
                "Bahasa antarmuka. Default mengikuti bahasa sistem.",
            );
            m.insert("language_system", "Sistem");
            m.insert("appearance", "Tampilan");
            m.insert("select_theme", "Pilih tema…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Pengaturan");
            m.insert("open_recent_project", "Buka proyek terbaru");
            m
        });
        locales.insert("uk", {
            let mut m = HashMap::new();
            m.insert("terminals", "Термінали");
            m.insert("new_terminal", "Новий термінал");
            m.insert("terminal_list", "Список терміналів");
            m.insert("group", "Група");
            m.insert("new_group", "Нова група");
            m.insert("files", "Файли");
            m.insert("up_one_level", "На рівень вище");
            m.insert("refresh", "Оновити");
            m.insert("file_list", "Список файлів");
            m.insert("ui_language", "Мова");
            m.insert(
                "ui_language_description",
                "Мова інтерфейсу. За замовчуванням — мова системи.",
            );
            m.insert("language_system", "Системна");
            m.insert("appearance", "Вигляд");
            m.insert("select_theme", "Вибрати тему…");
                        m.insert("menu_file", "File");
            m.insert("open", "Open…");
            m.insert("open_recent", "Open Recent…");
            m.insert("add_folder_to_project", "Add Folder to Project…");
            m.insert("close_window", "Close Window");
            m.insert("settings", "Налаштування");
            m.insert("open_recent_project", "Відкрити недавній проєкт");
            m
        });
        locales
    });

pub fn language_native_name(code: &str) -> &'static str {
    match code {
        "en" => "English",
        "zh-CN" => "简体中文",
        "zh-TW" => "繁體中文",
        "ja" => "日本語",
        "ko" => "한국어",
        "es" => "Español",
        "fr" => "Français",
        "de" => "Deutsch",
        "pt-BR" => "Português (Brasil)",
        "ru" => "Русский",
        "ar" => "العربية",
        "hi" => "हिन्दी",
        "it" => "Italiano",
        "nl" => "Nederlands",
        "tr" => "Türkçe",
        "pl" => "Polski",
        "vi" => "Tiếng Việt",
        "th" => "ไทย",
        "id" => "Bahasa Indonesia",
        "uk" => "Українська",
        _ => "English",
    }
}
