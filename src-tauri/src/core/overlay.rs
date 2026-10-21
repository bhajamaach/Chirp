use std::{thread, time::Duration};

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::state::{State, StateHub};

const OVERLAY_LABEL: &str = "overlay";
const OVERLAY_SIZE: f64 = 140.0;
/// Offset from the cursor so the mascot does not sit under the pointer.
const CURSOR_OFFSET: f64 = 20.0;

pub fn build(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let window = WebviewWindowBuilder::new(app, OVERLAY_LABEL, WebviewUrl::App("overlay.html".into()))
        .title("Chirp overlay")
        .inner_size(OVERLAY_SIZE, OVERLAY_SIZE)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()?;
    window.set_ignore_cursor_events(true)?;
    #[cfg(target_os = "macos")]
    crate::platform::macos::overlay::configure(&window)?;
    Ok(window)
}

/// Keeps the overlay next to the cursor while Chirp is not Idle. Sleeps on a
/// condvar while Idle so idle CPU use stays near zero.
pub fn spawn_cursor_follower(app: AppHandle, hub: StateHub) {
    thread::spawn(move || loop {
        hub.wait_until_active();
        if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
            if let Ok(pos) = app.cursor_position() {
                let scale = window.scale_factor().unwrap_or(1.0);
                let offset = (CURSOR_OFFSET * scale) as i32;
                let _ = window.set_position(PhysicalPosition::new(
                    pos.x as i32 + offset,
                    pos.y as i32 + offset,
                ));
            }
        }
        thread::sleep(Duration::from_millis(16));
    });
}

/// Moves the state machine and shows or hides the overlay to match.
pub fn apply_state(app: &AppHandle, hub: &StateHub, state: State) {
    hub.set(app, state);
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = if state == State::Idle { window.hide() } else { window.show() };
    }
}
