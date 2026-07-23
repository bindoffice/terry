//! Application icon helpers for Terry.

use std::sync::Arc;

/// 512×512 PNG used for Linux/X11 window icons and macOS dock fallback.
pub const APP_ICON_PNG: &[u8] = include_bytes!("../resources/app-icon.png");

/// Decode the embedded app icon for platforms that accept an RGBA image.
pub fn app_icon_image() -> Option<Arc<image::RgbaImage>> {
    image::load_from_memory(APP_ICON_PNG)
        .ok()
        .map(|img| Arc::new(img.to_rgba8()))
}

/// Set the macOS Dock / app icon from the embedded PNG.
/// Must run on the main thread after `NSApplication` exists.
#[cfg(target_os = "macos")]
pub fn apply_dock_icon() {
    use cocoa::appkit::NSApp;
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSData;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let data: id = NSData::dataWithBytes_length_(
            nil,
            APP_ICON_PNG.as_ptr() as *const std::ffi::c_void,
            APP_ICON_PNG.len() as u64,
        );
        if data == nil {
            return;
        }

        let alloc: id = msg_send![class!(NSImage), alloc];
        let image: id = msg_send![alloc, initWithData: data];
        if image == nil {
            return;
        }
        let _: () = msg_send![NSApp(), setApplicationIconImage: image];
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_dock_icon() {}
