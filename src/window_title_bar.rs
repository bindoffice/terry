use gpui::{AppContext as _, Context, Decorations, Entity, SharedString, Window};
use platform_title_bar::PlatformTitleBar;
use ui::{Color, Label, LabelSize, prelude::*};

/// Creates a platform title bar for a window that may fall back to
/// client-side decorations.
///
/// On Linux, compositors without server-side decoration support (such as GNOME
/// on Wayland) leave it up to the application to draw the title bar and its
/// window controls.
pub fn new_window_title_bar<T: 'static>(
    id: &'static str,
    cx: &mut Context<T>,
) -> Option<Entity<PlatformTitleBar>> {
    if cfg!(any(target_os = "linux", target_os = "freebsd")) {
        Some(cx.new(|cx| PlatformTitleBar::new(id, cx)))
    } else {
        None
    }
}

/// Renders the title bar for the given window if it is client-decorated.
///
/// Returns `None` when the platform draws the decorations itself, in which case
/// the window keeps its native title bar.
pub fn render_window_title_bar<T: 'static>(
    title_bar: &Option<Entity<PlatformTitleBar>>,
    title: impl Into<SharedString>,
    window: &Window,
    cx: &mut Context<T>,
) -> Option<Entity<PlatformTitleBar>> {
    let title_bar = title_bar.as_ref()?;
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return None;
    }

    let title = title.into();
    title_bar.update(cx, |title_bar, _| {
        title_bar.set_children([Label::new(title)
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element()]);
    });

    Some(title_bar.clone())
}
