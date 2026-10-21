mod core;
mod platform;

use std::{thread, time::Duration};

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    ActivationPolicy, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::core::state::{State, StateHub};

const OVERLAY_LABEL: &str = "overlay";
const OVERLAY_SIZE: f64 = 140.0;
/// Offset from the cursor so the mascot does not sit under the pointer.
const CURSOR_OFFSET: f64 = 20.0;

const DEV_STATES: [(&str, State); 7] = [
    ("Idle", State::Idle),
    ("Listening", State::Listening),
    ("Thinking", State::Thinking),
    ("Acting", State::Acting),
    ("Confirming", State::Confirming),
    ("Done", State::Done),
    ("Error", State::Error),
];

fn build_overlay(app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
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
    platform::macos::overlay::configure(&window)?;
    Ok(window)
}

/// Keeps the overlay next to the cursor while Chirp is active. Sleeps on a
/// condvar while Idle so idle CPU use stays near zero.
fn spawn_cursor_follower(app: tauri::AppHandle, hub: StateHub) {
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

fn apply_state(app: &tauri::AppHandle, hub: &StateHub, state: State) {
    hub.set(app, state);
    if let Some(window) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = if state == State::Idle { window.hide() } else { window.show() };
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let hub = StateHub::new();
            app.manage(hub.clone());

            build_overlay(app.handle())?;
            spawn_cursor_follower(app.handle().clone(), hub.clone());

            // Dev aid: `CHIRP_DEBUG_STATE=listening` applies a state shortly
            // after launch so the overlay can be checked without the tray.
            if let Ok(name) = std::env::var("CHIRP_DEBUG_STATE") {
                let handle = app.handle().clone();
                let hub = hub.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(3));
                    if let Some((_, state)) =
                        DEV_STATES.iter().find(|(l, _)| l.eq_ignore_ascii_case(&name))
                    {
                        apply_state(&handle, &hub, *state);
                    }
                });
            }

            // Dev-only state picker so every overlay state can be seen
            // before the voice loop exists.
            let state_items = DEV_STATES
                .iter()
                .map(|(label, _)| MenuItem::with_id(app, format!("state:{label}"), *label, true, None::<&str>))
                .collect::<tauri::Result<Vec<_>>>()?;
            let state_refs: Vec<&dyn tauri::menu::IsMenuItem<_>> =
                state_items.iter().map(|i| i as &dyn tauri::menu::IsMenuItem<_>).collect();
            let states_menu = Submenu::with_items(app, "Show state", true, &state_refs)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Chirp", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&states_menu, &PredefinedMenuItem::separator(app)?, &quit],
            )?;

            TrayIconBuilder::new()
                // Monochrome silhouette; macOS tints it for light and dark menu bars.
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .tooltip("Chirp")
                .menu(&menu)
                .on_menu_event(move |app, event| {
                    let id = event.id().as_ref();
                    if id == "quit" {
                        app.exit(0);
                    } else if let Some(label) = id.strip_prefix("state:") {
                        if let Some((_, state)) = DEV_STATES.iter().find(|(l, _)| *l == label) {
                            let hub = app.state::<StateHub>();
                            apply_state(app, &hub, *state);
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
