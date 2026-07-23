use std::collections::BTreeMap;

use editor::Editor;
use fs::Fs;
use gpui::{
    Action, App, AppContext as _, Context, Entity, FocusHandle, Focusable, Render, TitlebarOptions,
    Window, WindowBounds, WindowOptions, actions, div, px,
};
use settings::{KeybindSource, Settings, SettingsStore, update_settings_file};
use theme::ActiveTheme;
use ui::{
    Button, ButtonSize, ButtonStyle, Color, Label, LabelSize, SelectableButton, Toggleable,
    prelude::*,
};
use util::ResultExt;
use vim_mode_setting::VimModeSetting;

actions!(keymap_settings, [OpenKeymapSettings]);

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenKeymapSettings, cx| open_keymap_settings(cx));
}

fn open_keymap_settings(cx: &mut App) {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<KeymapSettingsWindow>())
    {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .log_err();
        return;
    }

    cx.defer(|cx| {
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some(format!("Terry — {}", i18n::t("keymap_settings")).into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                focus: true,
                show: true,
                is_movable: true,
                kind: gpui::WindowKind::Normal,
                window_background: cx.theme().window_background_appearance(),
                window_bounds: Some(WindowBounds::centered(gpui::size(px(720.), px(640.)), cx)),
                window_min_size: Some(gpui::Size {
                    width: px(480.),
                    height: px(400.),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| KeymapSettingsWindow::new(window, cx)),
        )
        .log_err();
    });
}

struct KeyBindingEntry {
    action_name: String,
    keystroke_text: String,
    source: KeybindSource,
}

pub struct KeymapSettingsWindow {
    focus_handle: FocusHandle,
    search_editor: Entity<Editor>,
}

impl KeymapSettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let search_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(i18n::t("keymap_search_placeholder").as_str(), window, cx);
            editor
        });

        cx.observe_global::<SettingsStore>(|_, cx| cx.notify())
            .detach();

        Self {
            focus_handle,
            search_editor,
        }
    }

    fn collect_bindings(cx: &App) -> Vec<KeyBindingEntry> {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let mut entries = Vec::new();

        for binding in keymap.bindings() {
            let action = binding.action();
            let action_name = action.name().to_string();

            // Skip internal/hidden actions
            if action_name.starts_with("zed_actions::OpenKeymap") || action_name.contains("::debug")
            {
                continue;
            }

            let keystroke_text = binding
                .keystrokes()
                .iter()
                .map(|ks| ks.to_string())
                .collect::<Vec<_>>()
                .join(" ");

            let source = binding
                .meta()
                .map(KeybindSource::from_meta)
                .unwrap_or(KeybindSource::Unknown);

            entries.push(KeyBindingEntry {
                action_name,
                keystroke_text,
                source,
            });
        }

        // Sort by action name for stable display
        entries.sort_by(|a, b| a.action_name.cmp(&b.action_name));

        // Deduplicate: keep only the last binding per action+keystroke combo
        // (later bindings override earlier ones)
        entries.dedup_by(|a, b| {
            a.action_name == b.action_name && a.keystroke_text == b.keystroke_text
        });

        entries
    }

    fn category_for_action(action_name: &str) -> String {
        // Extract the first meaningful segment as category
        let name = action_name
            .strip_prefix("zed_actions::")
            .or_else(|| action_name.strip_prefix("gpui::"))
            .unwrap_or(action_name);

        if let Some(idx) = name.find("::") {
            let prefix = &name[..idx];
            match prefix {
                "editor" => "Editor".to_string(),
                "workspace" => "Workspace".to_string(),
                "terminal_view" | "terminal" => "Terminal".to_string(),
                "vim" => "Vim".to_string(),
                "search" => "Search".to_string(),
                "menu" => "Menu".to_string(),
                "command_palette" => "Command Palette".to_string(),
                "go_to_line" => "Go to Line".to_string(),
                "outline" => "Outline".to_string(),
                "tab_switcher" => "Tab Switcher".to_string(),
                "markdown_preview" => "Markdown Preview".to_string(),
                "pane" => "Pane".to_string(),
                _ => capitalize_first(prefix),
            }
        } else {
            "General".to_string()
        }
    }
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let upper: String = first.to_uppercase().collect();
            upper + &chars.as_str().to_lowercase()
        }
    }
}

impl Focusable for KeymapSettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for KeymapSettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_editor.read(cx).text(cx).to_lowercase();
        let vim_mode = VimModeSetting::get_global(cx).0;

        let all_bindings = Self::collect_bindings(cx);
        let filtered: Vec<&KeyBindingEntry> = if query.is_empty() {
            all_bindings.iter().collect()
        } else {
            all_bindings
                .iter()
                .filter(|entry| {
                    entry.action_name.to_lowercase().contains(&query)
                        || entry.keystroke_text.to_lowercase().contains(&query)
                })
                .collect()
        };

        let groups = {
            let mut groups: BTreeMap<String, Vec<&&KeyBindingEntry>> = BTreeMap::new();
            for entry in &filtered {
                let category = KeymapSettingsWindow::category_for_action(&entry.action_name);
                groups.entry(category).or_default().push(entry);
            }
            groups
        };

        let total_count = filtered.len();

        div()
            .id("terry-keymap-settings")
            .key_context("KeymapSettings")
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            // Header
            .child(
                div()
                    .px_6()
                    .pt_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Label::new(i18n::t("keymap_settings")).size(LabelSize::Large))
                    .child(
                        Label::new(i18n::t("keymap_settings_description"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            // Toolbar: search + vim toggle + open keymap
            .child(
                div()
                    .px_6()
                    .pb_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .h_7()
                            .px_2()
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .child(self.search_editor.clone()),
                    )
                    .child(
                        Button::new("vim-mode-toggle", i18n::t("vim_mode"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .toggle_state(vim_mode)
                            .selected_style(ButtonStyle::Tinted(ui::TintColor::Accent))
                            .on_click(|_, window, cx| {
                                let new_value = !VimModeSetting::get_global(cx).0;
                                let fs = <dyn Fs>::global(cx);
                                update_settings_file(fs, cx, move |content, _| {
                                    content.vim_mode = Some(new_value);
                                });
                                let _ = window;
                            }),
                    )
                    .child(
                        Button::new("open-keymap-file", i18n::t("open_keymap_file"))
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Medium)
                            .on_click(|_, window, cx| {
                                window
                                    .dispatch_action(zed_actions::OpenKeymapFile.boxed_clone(), cx);
                            }),
                    ),
            )
            // Binding count
            .child(
                div().px_6().pb_1().child(
                    Label::new(format!(
                        "{} {}",
                        total_count,
                        i18n::t("keymap_bindings_count")
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
            )
            // Scrollable binding list
            .child(
                div()
                    .flex_1()
                    .id("keymap-binding-scroll")
                    .overflow_y_scroll()
                    .px_6()
                    .pb_10()
                    .child(v_flex().w_full().gap_3().children(
                        groups.into_iter().map(|(category, entries)| {
                            render_category_group(category, &entries, cx)
                        }),
                    )),
            )
    }
}

fn render_category_group(
    category: String,
    entries: &[&&KeyBindingEntry],
    cx: &mut Context<KeymapSettingsWindow>,
) -> AnyElement {
    v_flex()
        .w_full()
        .gap_1()
        .child(
            h_flex()
                .gap_1p5()
                .pb_1()
                .child(
                    Label::new(category.clone())
                        .size(LabelSize::Default)
                        .color(Color::Muted),
                )
                .child(div().flex_1().h_px().bg(cx.theme().colors().border_variant)),
        )
        .children(entries.iter().map(|entry| render_binding_row(entry, cx)))
        .into_any_element()
}

fn render_binding_row(
    entry: &&KeyBindingEntry,
    cx: &mut Context<KeymapSettingsWindow>,
) -> AnyElement {
    let entry = *entry;
    let is_user = entry.source == KeybindSource::User;
    let is_vim = entry.source == KeybindSource::Vim;

    // Clean up action name for display
    let display_name = entry
        .action_name
        .rsplit("::")
        .next()
        .unwrap_or(&entry.action_name);

    let source_badge = if is_user {
        Some(("User", Color::Accent))
    } else if is_vim {
        Some(("Vim", Color::Warning))
    } else {
        None
    };

    h_flex()
        .w_full()
        .py_0p5()
        .px_2()
        .rounded_md()
        .hover(|style| style.bg(cx.theme().colors().ghost_element_hover))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_1()
                .child(Label::new(display_name.to_string()).size(LabelSize::Small))
                .when_some(source_badge, |this, (badge_text, color)| {
                    this.child(Label::new(badge_text).size(LabelSize::XSmall).color(color))
                }),
        )
        .child(
            div().flex().items_center().gap_0p5().child(
                div()
                    .px_1p5()
                    .py_0p5()
                    .rounded_sm()
                    .bg(cx.theme().colors().element_background)
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(
                        Label::new(if entry.keystroke_text.is_empty() {
                            "—".to_string()
                        } else {
                            entry.keystroke_text.clone()
                        })
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    ),
            ),
        )
        .into_any_element()
}
