//! Lightweight UI internationalization for Terry.
//!
//! - [`UiLanguage`] setting (system or an explicit locale)
//! - [`t`] / [`t_str`] lookup helpers
//! - Defaults to the OS language via `sys-locale`

mod translations;

use std::sync::RwLock;

use gpui::{App, Global};
use settings::{RegisterSetting, Settings, SettingsContent, SettingsStore, UiLanguage};
use translations::{TRANSLATIONS, language_native_name};

static ACTIVE_LOCALE: RwLock<&'static str> = RwLock::new("en");

/// Runtime setting wrapper so UI can observe language changes.
#[derive(Clone, Copy, RegisterSetting)]
pub struct UiLanguageSetting(pub UiLanguage);

impl Settings for UiLanguageSetting {
    fn from_settings(content: &SettingsContent) -> Self {
        Self(content.ui_language.unwrap())
    }
}

#[allow(dead_code)]
struct ActiveLocaleGlobal(&'static str);

impl Global for ActiveLocaleGlobal {}

/// Initialize i18n: resolve locale from settings / system and refresh on change.
pub fn init(cx: &mut App) {
    UiLanguageSetting::register(cx);
    apply_from_settings(cx);

    cx.observe_global::<SettingsStore>(|cx| {
        apply_from_settings(cx);
    })
    .detach();
}

fn apply_from_settings(cx: &mut App) {
    let setting = UiLanguageSetting::get_global(cx).0;
    let locale = resolve_locale(setting);
    if locale == active_locale() && cx.try_global::<ActiveLocaleGlobal>().is_some() {
        return;
    }
    set_active_locale(locale);
    cx.set_global(ActiveLocaleGlobal(locale));
    cx.refresh_windows();
}

/// Map a [`UiLanguage`] setting to a concrete BCP-47-ish locale code.
pub fn resolve_locale(setting: UiLanguage) -> &'static str {
    match setting {
        UiLanguage::System => detect_system_locale(),
        UiLanguage::English => "en",
        UiLanguage::ChineseSimplified => "zh-CN",
        UiLanguage::ChineseTraditional => "zh-TW",
        UiLanguage::Japanese => "ja",
        UiLanguage::Korean => "ko",
        UiLanguage::Spanish => "es",
        UiLanguage::French => "fr",
        UiLanguage::German => "de",
        UiLanguage::PortugueseBrazil => "pt-BR",
        UiLanguage::Russian => "ru",
        UiLanguage::Arabic => "ar",
        UiLanguage::Hindi => "hi",
        UiLanguage::Italian => "it",
        UiLanguage::Dutch => "nl",
        UiLanguage::Turkish => "tr",
        UiLanguage::Polish => "pl",
        UiLanguage::Vietnamese => "vi",
        UiLanguage::Thai => "th",
        UiLanguage::Indonesian => "id",
        UiLanguage::Ukrainian => "uk",
    }
}

/// Detect the system UI language and map it onto a supported locale.
pub fn detect_system_locale() -> &'static str {
    let Some(raw) = sys_locale::get_locale() else {
        return "en";
    };
    map_system_locale(&raw)
}

fn map_system_locale(raw: &str) -> &'static str {
    let normalized = raw.replace('_', "-");
    let lower = normalized.to_ascii_lowercase();

    // Exact / prefix matches for supported locales.
    const SUPPORTED: &[&str] = &[
        "en", "zh-CN", "zh-TW", "ja", "ko", "es", "fr", "de", "pt-BR", "ru", "ar", "hi", "it",
        "nl", "tr", "pl", "vi", "th", "id", "uk",
    ];
    for code in SUPPORTED {
        if lower.eq_ignore_ascii_case(code) {
            return *code;
        }
    }

    // Language-only fallbacks.
    let lang = lower.split('-').next().unwrap_or("en");
    match lang {
        "zh" => {
            // zh-HK / zh-MO / zh-TW → Traditional; otherwise Simplified.
            if lower.contains("tw") || lower.contains("hk") || lower.contains("mo") || lower.contains("hant")
            {
                "zh-TW"
            } else {
                "zh-CN"
            }
        }
        "pt" => "pt-BR",
        "en" => "en",
        "ja" => "ja",
        "ko" => "ko",
        "es" => "es",
        "fr" => "fr",
        "de" => "de",
        "ru" => "ru",
        "ar" => "ar",
        "hi" => "hi",
        "it" => "it",
        "nl" => "nl",
        "tr" => "tr",
        "pl" => "pl",
        "vi" => "vi",
        "th" => "th",
        "id" | "in" => "id",
        "uk" => "uk",
        _ => "en",
    }
}

fn set_active_locale(locale: &'static str) {
    if let Ok(mut guard) = ACTIVE_LOCALE.write() {
        *guard = locale;
    }
}

/// Currently active locale code (e.g. `"zh-CN"`).
pub fn active_locale() -> &'static str {
    ACTIVE_LOCALE
        .read()
        .map(|g| *g)
        .unwrap_or("en")
}

/// Translate `key` for the active locale, falling back to English then the key.
pub fn t(key: &str) -> String {
    t_str(key).to_string()
}

/// Translate `key` returning a borrowed string when possible.
pub fn t_str(key: &str) -> &'static str {
    let locale = active_locale();
    TRANSLATIONS
        .get(locale)
        .and_then(|m| m.get(key).copied())
        .or_else(|| TRANSLATIONS.get("en").and_then(|m| m.get(key).copied()))
        .unwrap_or("???")
}

/// Native display name for a locale code (e.g. `"ja"` → `"日本語"`).
pub fn native_name_for_code(code: &str) -> &'static str {
    language_native_name(code)
}

/// Native display name for a [`UiLanguage`] setting value.
pub fn native_name_for_setting(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::System => t_str("language_system"),
        other => native_name_for_code(resolve_locale(other)),
    }
}
