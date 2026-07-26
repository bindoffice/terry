use gpui::{App, actions};
use zed_actions::OpenKeymap;

actions!(keymap_settings, [OpenKeymapSettings]);

pub fn init(cx: &mut App) {
    // Route to the full keymap editor so bindings can be recorded and edited.
    cx.on_action(|_: &OpenKeymapSettings, cx| {
        cx.dispatch_action(&OpenKeymap);
    });
}
