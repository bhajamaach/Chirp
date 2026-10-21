mod core;
mod llm;
mod platform;
mod safety;
mod voice;

use std::{
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use tauri::{
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
    ActivationPolicy, Manager,
};

use crate::{
    core::{
        agent::{Agent, Limits},
        barge_in, cues,
        narrator::Narrator,
        overlay::{self, apply_state},
        session::Session,
        setup::Setup,
        state::{State, StateHub},
        usage::Ledger,
    },
    llm::{LlmClient, Router},
    platform::macos::adapter::MacAdapter,
    safety::log::ActionLog,
    voice::{
        listen::Listener, playback::Player, speaker::Speaker, stt::sarvam::SarvamConfig,
        tts::sarvam::TtsConfig,
    },
};

/// Estimated speech spend, in rupees, at which speech calls stop. Leaves a
/// reserve of the Rs 100 free credit. Override with `SARVAM_BUDGET_RUPEES`.
const DEFAULT_BUDGET_RUPEES: f64 = 90.0;

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
    // Keys come from the environment or a git-ignored .env, never from code.
    let _ = dotenvy::dotenv();

    // `chirp --transcribe-file clip.wav` tests speech-to-text without the app.
    let args: Vec<String> = std::env::args().collect();
    if let Some(position) = args.iter().position(|a| a == "--transcribe-file") {
        let code = match args.get(position + 1) {
            Some(path) => voice::cli::transcribe_file(path),
            None => {
                eprintln!("Usage: chirp --transcribe-file <clip.wav>");
                2
            }
        };
        std::process::exit(code);
    }

    // `chirp --ax <command>` drives the accessibility adapter from typed commands.
    #[cfg(target_os = "macos")]
    if let Some(position) = args.iter().position(|a| a == "--ax") {
        std::process::exit(platform::macos::cli::run(&args[position + 1..]));
    }

    // `chirp --plan "request"` shows what the model would do first, touching nothing.
    if let Some(position) = args.iter().position(|a| a == "--plan") {
        let code = match args.get(position + 1) {
            Some(request) => core::cli::plan_only(request, args.iter().any(|a| a == "--screen")),
            None => {
                eprintln!("Usage: chirp --plan \"request\"");
                2
            }
        };
        std::process::exit(code);
    }

    // `chirp --run "request" [--speak]` runs a typed request through the agent for real.
    if let Some(position) = args.iter().position(|a| a == "--run") {
        let code = match args.get(position + 1) {
            Some(request) => core::cli::run_request(request, args.iter().any(|a| a == "--speak")),
            None => {
                eprintln!("Usage: chirp --run \"request\" [--speak]");
                2
            }
        };
        std::process::exit(code);
    }

    // `chirp --voice-lines list|status|import <folder>` manages recorded voice lines.
    if let Some(position) = args.iter().position(|a| a == "--voice-lines") {
        std::process::exit(voice::cli::voice_lines(&args[position + 1..]));
    }

    // `chirp --barge-test` checks the speak-over-Chirp interrupt with the real mic and speakers.
    if args.iter().any(|a| a == "--barge-test") {
        std::process::exit(voice::cli::barge_test());
    }

    // `chirp --cue-test` checks the sound cues reach the speakers and how loud they are.
    if args.iter().any(|a| a == "--cue-test") {
        std::process::exit(voice::cli::cue_test());
    }

    // `chirp --stop-test` measures how fast speech can be cut off.
    if args.iter().any(|a| a == "--stop-test") {
        std::process::exit(voice::cli::stop_test());
    }

    // `chirp --speak "text"` tests spoken output without the app.
    if let Some(position) = args.iter().position(|a| a == "--speak") {
        let code = match args.get(position + 1) {
            Some(text) => voice::cli::speak_text(text),
            None => {
                eprintln!("Usage: chirp --speak \"text\"");
                2
            }
        };
        std::process::exit(code);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let hub = StateHub::new();
            app.manage(hub.clone());

            let config_dir = app.path().app_config_dir()?;
            let budget = std::env::var("SARVAM_BUDGET_RUPEES")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(DEFAULT_BUDGET_RUPEES);
            let ledger = Arc::new(Mutex::new(Ledger::load(&config_dir, budget)));

            // Audio output for sound cues and spoken replies. With no output
            // device the speaker falls back to the system voice.
            let player = match Player::init_global() {
                Ok(player) => {
                    eprintln!("[chirp] audio output ready at {} Hz", player.device_rate());
                    Some(player)
                }
                Err(error) => {
                    eprintln!("[chirp] no audio output: {error}");
                    None
                }
            };
            let speaker =
                Speaker::init(player.clone(), TtsConfig::from_env(), ledger.clone(), app.path().app_cache_dir()?);
            speaker.use_recordings(config_dir.join("voice-lines"));
            cues::start(&hub);
            // Speaking over Chirp stops it. Needs the microphone and an output.
            if let (true, Some(player)) = (barge_in::enabled(), player) {
                barge_in::start(speaker, player);
            }

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
            let transcript_item = MenuItem::with_id(app, "last_heard", "Heard: nothing yet", false, None::<&str>)?;
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
                    &transcript_item,
                    &change_key,
                    &check_permissions,
                    &PredefinedMenuItem::separator(app)?,
                    &states_menu,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?;

            let ledger_for_llm = ledger.clone();
            let config_dir_for_log = config_dir.clone();
            let listener = Listener::new(SarvamConfig::from_env(), ledger);
            // The agent: simple commands run by rules; the rest go to a language model.
            let narrator = Narrator::start();
            let router = Router::from_env(Some(ledger_for_llm));
            let model: Option<Arc<dyn LlmClient>> = if router.has_provider() {
                eprintln!("[chirp] language model: {}", router.describe());
                Some(Arc::new(router))
            } else {
                eprintln!("[chirp] no language-model key: only simple commands (open an app, scroll, back) will work");
                None
            };
            let action_log = ActionLog::open(&config_dir_for_log);
            let agent = Arc::new(Agent::new(MacAdapter::new(), model, narrator.clone(), action_log, Limits::default()));
            let session = Session::new(app.handle().clone(), hub.clone(), listener, transcript_item, agent, narrator);
            let setup = Setup::new(app.handle().clone(), hub.clone(), config_dir, key_item, session);
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
