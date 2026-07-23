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
            m.insert("terminal_list", "Terminal List");
            m.insert("group", "Group");
            m.insert("new_group", "New Group");
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
            m.insert("settings", "Settings");
            m.insert("recent_terminals", "Recent Terminals");
            m.insert("recent_folders", "Recent Folders");
            m.insert("open_recent_project", "Open Recent Project");
            m.insert("llm_providers", "LLM Providers");
            m.insert(
                "llm_providers_description",
                "Configure API keys and settings for your AI providers.",
            );
            m
        });
        locales.insert("zh-CN", {
            let mut m = HashMap::new();
            m.insert("terminals", "终端");
            m.insert("new_terminal", "新建终端");
            m.insert("terminal_list", "终端列表");
            m.insert("group", "分组");
            m.insert("new_group", "新建分组");
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
            m.insert("settings", "设置");
            m.insert("recent_terminals", "最近终端");
            m.insert("recent_folders", "最近文件夹");
            m.insert("open_recent_project", "打开最近项目");
            m.insert("llm_providers", "大模型服务商");
            m.insert(
                "llm_providers_description",
                "配置 AI 服务商的 API Key 和相关设置。",
            );
            m
        });
        locales.insert("zh-TW", {
            let mut m = HashMap::new();
            m.insert("terminals", "終端機");
            m.insert("new_terminal", "新增終端機");
            m.insert("terminal_list", "終端機列表");
            m.insert("group", "分組");
            m.insert("new_group", "新增分組");
            m.insert("files", "檔案");
            m.insert("up_one_level", "上一層");
            m.insert("refresh", "重新整理");
            m.insert("file_list", "檔案列表");
            m.insert("ui_language", "語言");
            m.insert("ui_language_description", "介面語言。預設跟隨系統語言。");
            m.insert("language_system", "跟隨系統");
            m.insert("appearance", "外觀");
            m.insert("select_theme", "選擇主題…");
            m.insert("settings", "設定");
            m.insert("open_recent_project", "開啟最近專案");
            m
        });
        locales.insert("ja", {
            let mut m = HashMap::new();
            m.insert("terminals", "ターミナル");
            m.insert("new_terminal", "新しいターミナル");
            m.insert("terminal_list", "ターミナル一覧");
            m.insert("group", "グループ");
            m.insert("new_group", "新しいグループ");
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
            m.insert("settings", "設定");
            m.insert("open_recent_project", "最近のプロジェクトを開く");
            m
        });
        locales.insert("ko", {
            let mut m = HashMap::new();
            m.insert("terminals", "터미널");
            m.insert("new_terminal", "새 터미널");
            m.insert("terminal_list", "터미널 목록");
            m.insert("group", "그룹");
            m.insert("new_group", "새 그룹");
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
            m.insert("settings", "설정");
            m.insert("open_recent_project", "최근 프로젝트 열기");
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
