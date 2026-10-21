use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use tauri::{menu::MenuItem, AppHandle, Wry};

use super::{
    config::{Config, HotkeyConfig},
    overlay::apply_state,
    session::Session,
    state::{State, StateHub},
};
use crate::{
    platform::macos::{
        hotkey::Hotkey,
        keycodes,
        permissions::{self, Permission},
    },
    voice::speak::say,
};

const PERMISSION_WAIT: Duration = Duration::from_secs(120);
const PICK_WAIT: Duration = Duration::from_secs(30);
const PICK_ATTEMPTS: usize = 3;
/// Offered in the spoken prompt: a right-side modifier is rarely used in
/// shortcuts and does not type anything.
pub const SUGGESTED_KEY: u16 = 61; // Right Option

/// First-run setup and the menu actions that re-run parts of it: the spoken
/// permissions flow, the key picker and the saved config. Everything here
/// runs on a worker thread, one flow at a time.
#[derive(Clone)]
pub struct Setup {
    app: AppHandle,
    hub: StateHub,
    config_dir: PathBuf,
    config: Arc<Mutex<Config>>,
    hotkey: Arc<Mutex<Option<Hotkey>>>,
    key_item: MenuItem<Wry>,
    hold_tx: mpsc::Sender<bool>,
    busy: Arc<AtomicBool>,
}

impl Setup {
    pub fn new(
        app: AppHandle,
        hub: StateHub,
        config_dir: PathBuf,
        key_item: MenuItem<Wry>,
        session: Arc<Session>,
    ) -> Self {
        let config = Config::load(&config_dir);

        // The key callback must return fast, so each press and release is
        // handed to a worker thread that runs the spoken turn.
        let (hold_tx, hold_rx) = mpsc::channel::<bool>();
        thread::spawn(move || {
            for held in hold_rx {
                session.on_hold(held);
            }
        });

        let setup = Setup {
            app,
            hub,
            config_dir,
            config: Arc::new(Mutex::new(config)),
            hotkey: Arc::new(Mutex::new(None)),
            key_item,
            hold_tx,
            busy: Arc::new(AtomicBool::new(false)),
        };
        setup.show_key_label();
        setup
    }

    /// On launch: first-run welcome, any missing permissions, then the key.
    pub fn start(&self) {
        self.spawn(|s| {
            let first_run = !s.config.lock().unwrap().setup_complete;
            if first_run {
                say("Hi, I am Chirp. Let's set up a few permissions.");
            }
            if !s.ready() {
                return;
            }
            let saved = s.config.lock().unwrap().hotkey.clone();
            match saved {
                Some(key) => s.use_key(key.keycode),
                None => {
                    if s.pick_key() && first_run {
                        say("You are all set.");
                    }
                }
            }
        });
    }

    /// Menu: choose a different talk key.
    pub fn change_key(&self) {
        self.spawn(|s| {
            if s.ready() {
                s.pick_key();
            }
        });
    }

    /// Menu: re-check permissions and fix any that are missing.
    pub fn check_permissions(&self) {
        self.spawn(|s| {
            let all_on = Permission::ALL.iter().all(|p| permissions::is_granted(*p));
            if all_on && s.hotkey.lock().unwrap().is_some() {
                say("All permissions are on.");
                return;
            }
            if s.ready() {
                let saved = s.config.lock().unwrap().hotkey.clone();
                if let Some(key) = saved {
                    s.use_key(key.keycode);
                }
                say("All permissions are on.");
            }
        });
    }

    /// Runs `flow` on a worker thread unless another flow is already running.
    fn spawn(&self, flow: impl FnOnce(&Setup) + Send + 'static) {
        if self.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        let setup = self.clone();
        thread::spawn(move || {
            flow(&setup);
            setup.set_state(State::Idle);
            setup.busy.store(false, Ordering::SeqCst);
        });
    }

    fn set_state(&self, state: State) {
        apply_state(&self.app, &self.hub, state);
    }

    fn flash(&self, state: State, millis: u64) {
        self.set_state(state);
        thread::sleep(Duration::from_millis(millis));
        self.set_state(State::Idle);
    }

    fn show_key_label(&self) {
        let text = match &self.config.lock().unwrap().hotkey {
            Some(key) => format!("Talk key: {}", key.label),
            None => "Talk key: not set".to_string(),
        };
        let _ = self.key_item.set_text(text);
    }

    /// Permissions, then the keyboard tap. Both must be in place for the talk key.
    fn ready(&self) -> bool {
        self.ensure_permissions() && self.ensure_tap()
    }

    /// Asks for each missing permission, one at a time, with a spoken reason.
    fn ensure_permissions(&self) -> bool {
        for permission in Permission::ALL {
            if permissions::is_granted(permission) {
                continue;
            }
            self.set_state(State::Confirming);
            say(permission.reason());
            permissions::request(permission);
            if !self.wait_for(permission) {
                say("Still waiting. Choose Check permissions in the Chirp menu.");
                return false;
            }
            say(&format!("{} is on.", permission.label()));
        }
        self.set_state(State::Idle);
        true
    }

    fn wait_for(&self, permission: Permission) -> bool {
        let deadline = Instant::now() + PERMISSION_WAIT;
        while Instant::now() < deadline {
            if permissions::is_granted(permission) {
                return true;
            }
            thread::sleep(Duration::from_millis(500));
        }
        false
    }

    /// Installs the global key tap once permissions allow it.
    fn ensure_tap(&self) -> bool {
        if self.hotkey.lock().unwrap().is_some() {
            return true;
        }
        for attempt in 0..2 {
            let tx = self.hold_tx.clone();
            match Hotkey::start(move |held| {
                let _ = tx.send(held);
            }) {
                Ok(hotkey) => {
                    *self.hotkey.lock().unwrap() = Some(hotkey);
                    return true;
                }
                Err(reason) => {
                    eprintln!("[chirp] key tap failed (attempt {}): {reason}", attempt + 1);
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
        say("Please restart Chirp to finish setup.");
        false
    }

    fn use_key(&self, keycode: u16) {
        if let Some(hotkey) = self.hotkey.lock().unwrap().as_ref() {
            hotkey.set_key(Some(keycode));
        }
    }

    /// "Press the key you want to use." Refuses keys that would break typing.
    fn pick_key(&self) -> bool {
        let Some(hotkey) = self.hotkey.lock().unwrap().clone() else {
            return false;
        };
        for _ in 0..PICK_ATTEMPTS {
            self.set_state(State::Confirming);
            say(&crate::voice::phrases::suggest_key_line(&keycodes::label(SUGGESTED_KEY)));
            let Some(code) = hotkey.pick(PICK_WAIT) else {
                say("I didn't hear a key. Choose Change talk key in the menu.");
                return false;
            };
            match self.validate(code) {
                Err(reason) => {
                    self.flash(State::Error, 500);
                    say(reason);
                }
                Ok(()) => {
                    let label = keycodes::label(code);
                    hotkey.set_key(Some(code));
                    self.save_key(code, &label);
                    self.set_state(State::Done);
                    say(&format!("Got it. Hold {label} to talk."));
                    return true;
                }
            }
        }
        say("Let's try again later. Choose Change talk key in the menu.");
        false
    }

    fn validate(&self, code: u16) -> Result<(), &'static str> {
        keycodes::check(code)?;
        if matches!(code, keycodes::FN | keycodes::GLOBE) && !keycodes::fn_is_free() {
            return Err("Fn opens other features on this Mac. Pick another key.");
        }
        Ok(())
    }

    fn save_key(&self, keycode: u16, label: &str) {
        let mut config = self.config.lock().unwrap();
        config.hotkey = Some(HotkeyConfig { keycode, label: label.to_string() });
        config.setup_complete = true;
        if let Err(error) = config.save(&self.config_dir) {
            eprintln!("[chirp] could not save config: {error}");
        }
        drop(config);
        self.show_key_label();
    }
}
