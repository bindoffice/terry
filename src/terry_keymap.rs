//! Filters keymap bindings and command palette entries to Terry-relevant actions.

use collections::HashSet;
use command_palette_hooks::CommandPaletteFilter;
use gpui::{App, KeyBinding};

/// Action namespaces that Terry actually uses in the UI / panels / menus.
/// Must stay sorted for binary search.
const TERRY_ACTION_NAMESPACES: &[&str] = &[
    "agent",
    "app_title_bar",
    "buffer_search",
    "command_palette",
    "copilot",
    "edit_prediction",
    "editor",
    "encoding_selector",
    "file_list_panel",
    "go_to_line",
    "gpui",
    "image_viewer",
    "keymap_editor",
    "keymap_settings",
    "keystroke_input",
    "language_selector",
    "line_ending_selector",
    "llm_provider_settings",
    "markdown",
    "markdown_preview",
    "menu",
    "multi_workspace",
    "outline",
    "pane",
    "picker",
    "project_search",
    "projects",
    "recent_projects",
    "search",
    "settings_window",
    "tab_switcher",
    "terminal",
    "terminal_list_panel",
    "terminal_panel",
    "terminal_view",
    "theme",
    "theme_selector",
    "toast",
    "vim",
    "workspace",
    "zed",
];

fn action_namespace(action_name: &str) -> &str {
    action_name.split("::").next().unwrap_or(action_name)
}

fn is_terry_namespace(namespace: &str) -> bool {
    TERRY_ACTION_NAMESPACES.binary_search(&namespace).is_ok()
}

pub fn is_terry_action_name(action_name: &str) -> bool {
    is_terry_namespace(action_namespace(action_name))
}

pub fn filter_terry_key_bindings(bindings: Vec<KeyBinding>) -> Vec<KeyBinding> {
    bindings
        .into_iter()
        .filter(|binding| is_terry_action_name(binding.action().name()))
        .collect()
}

/// Hide command-palette / keymap-editor entries outside Terry's action set.
pub fn apply_terry_action_filter(cx: &mut App) {
    CommandPaletteFilter::update_global(cx, |filter, cx| {
        let mut namespaces = HashSet::default();
        for &name in cx.all_action_names() {
            namespaces.insert(action_namespace(name));
        }
        for namespace in namespaces {
            if !is_terry_namespace(namespace) {
                filter.hide_namespace(namespace);
            }
        }
    });
}
