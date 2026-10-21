mod core;
mod platform;
mod voice;

use std::{thread, time::Duration};

use tauri::{
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    ActivationPolicy, Manager,
};

use crate::core::{
    overlay::{self, apply_state},
    setup::Setup,
    state::{State, StateHub},
};

const DEV_STATES: [(&str, State); 7] = [
    ("Idle", State::Idle),
    ("Listening", State::Listening),
    ("Thinking", State::Thinking),
    ("Acting", State::Acting),
    ("Confirming", State::Confirming),
    ("Done", State::Done),
    ("Error", State::Error),
];

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let hub = StateHub::new();
            app.manage(hub.clone());

            overlay::build(app.handle())?;
            overlay::spawn_cursor_follower(app.handle().clone(), hub.clone());

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

            // Menu-bar menu: the talk key, the setup actions, and a dev-only
            // state picker so every overlay state can be previewed.
            let key_item = MenuItem::with_id(app, "key_label", "Talk key: not set", false, None::<&str>)?;
            let change_key = MenuItem::with_id(app, "change_key", "Change talk key…", true, None::<&str>)?;
            let check_permissions =
                MenuItem::with_id(app, "check_permissions", "Check permissions…", true, None::<&str>)?;
            let state_items = DEV_STATES
                .iter()
                .map(|(label, _)| MenuItem::with_id(app, format!("state:{label}"), *label, true, None::<&str>))
                .collect::<tauri::Result<Vec<_>>>()?;
            let state_refs: Vec<&dyn IsMenuItem<_>> =
                state_items.iter().map(|i| i as &dyn IsMenuItem<_>).collect();
            let states_menu = Submenu::with_items(app, "Show state", true, &state_refs)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Chirp", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &key_item,
                    &change_key,
                    &check_permissions,
                    &PredefinedMenuItem::separator(app)?,
                    &states_menu,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            let config_dir = app.path().app_config_dir()?;
            let setup = Setup::new(app.handle().clone(), hub.clone(), config_dir, key_item);
            setup.start();

            TrayIconBuilder::new()
                // Monochrome silhouette; macOS tints it for light and dark menu bars.
                .icon(tauri::include_image!("icons/tray.png"))
                .icon_as_template(true)
                .tooltip("Chirp")
                .menu(&menu)
                .on_menu_event(move |app, event| {
                    let id = event.id().as_ref();
                    match id {
                        "quit" => app.exit(0),
                        "change_key" => setup.change_key(),
                        "check_permissions" => setup.check_permissions(),
                        _ => {
                            if let Some(label) = id.strip_prefix("state:") {
                                if let Some((_, state)) = DEV_STATES.iter().find(|(l, _)| *l == label) {
                                    let hub = app.state::<StateHub>();
                                    apply_state(app, &hub, *state);
                                }
                            }
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
