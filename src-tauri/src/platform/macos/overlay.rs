use objc2_app_kit::{NSScreenSaverWindowLevel, NSWindow, NSWindowCollectionBehavior};
use tauri::WebviewWindow;

/// Native NSWindow tweaks Tauri's window API does not cover: sit above
/// full-screen apps, join every space, stay out of window cycling, and never
/// receive mouse events. Must be called on the main thread.
pub fn configure(window: &WebviewWindow) -> tauri::Result<()> {
    let ptr = window.ns_window()?;
    // SAFETY: Tauri guarantees `ns_window()` is a live NSWindow for as long as
    // the WebviewWindow exists, and we only use it on the main thread.
    let ns: &NSWindow = unsafe { &*(ptr as *const NSWindow) };

    ns.setLevel(NSScreenSaverWindowLevel);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    ns.setIgnoresMouseEvents(true);
    ns.setHidesOnDeactivate(false);
    ns.setCanHide(false);
    Ok(())
}
