use std::{
    ffi::c_void,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::Duration,
};

use core_foundation::{
    base::TCFType,
    runloop::{kCFRunLoopCommonModes, CFRunLoop},
};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};

use super::keycodes;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapEnable(tap: *mut c_void, enable: bool);
}

#[derive(Default)]
struct Shared {
    /// The saved hold-to-talk key, if one is chosen.
    hotkey: Option<u16>,
    /// Whether the hold-to-talk key is currently held.
    down: bool,
    /// Set while the key picker waits for the next key press.
    pick_tx: Option<mpsc::Sender<u16>>,
}

/// Global press-and-hold key capture, built on a macOS event tap.
///
/// The tap is an active one, so a chosen key that would otherwise type or
/// trigger something is swallowed while held. Modifier keys are passed through
/// untouched, because dropping them can leave other apps with a stuck modifier.
/// Needs the Accessibility and Input Monitoring permissions.
#[derive(Clone)]
pub struct Hotkey {
    shared: Arc<Mutex<Shared>>,
}

impl Hotkey {
    /// Installs the tap on its own thread. `on_hold` is called with `true` when
    /// the talk key goes down and `false` when it comes up. It runs inside the
    /// event tap callback, so it must return quickly. Returns an error when
    /// macOS refuses the tap, which normally means a permission is missing.
    pub fn start(on_hold: impl Fn(bool) + Send + Sync + 'static) -> Result<Hotkey, String> {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (ready_tx, ready_rx) = mpsc::channel::<bool>();
        let callback_shared = shared.clone();

        thread::Builder::new()
            .name("chirp-hotkey".into())
            .spawn(move || {
                let port = Arc::new(AtomicUsize::new(0));
                let callback_port = port.clone();
                let tap = CGEventTap::new(
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    CGEventTapOptions::Default,
                    vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged],
                    move |_proxy, event_type, event| {
                        handle(&callback_shared, &on_hold, &callback_port, event_type, event)
                    },
                );
                let Ok(tap) = tap else {
                    let _ = ready_tx.send(false);
                    return;
                };
                port.store(tap.mach_port().as_concrete_TypeRef() as usize, Ordering::SeqCst);
                let source = tap
                    .mach_port()
                    .create_runloop_source(0)
                    .expect("event tap run loop source");
                CFRunLoop::get_current().add_source(&source, unsafe { kCFRunLoopCommonModes });
                tap.enable();
                let _ = ready_tx.send(true);
                CFRunLoop::run_current();
            })
            .map_err(|e| format!("could not start the key thread: {e}"))?;

        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(true) => Ok(Hotkey { shared }),
            Ok(false) => Err("macOS refused the keyboard tap".into()),
            Err(_) => Err("the key thread did not report back".into()),
        }
    }

    /// Sets (or clears) the hold-to-talk key.
    pub fn set_key(&self, keycode: Option<u16>) {
        let mut shared = self.shared.lock().unwrap();
        shared.hotkey = keycode;
        shared.down = false;
    }

    /// Waits for the next key press and returns its key code, or `None` on
    /// timeout. The pressed key is swallowed so it does not reach other apps.
    pub fn pick(&self, timeout: Duration) -> Option<u16> {
        let (tx, rx) = mpsc::channel();
        self.shared.lock().unwrap().pick_tx = Some(tx);
        let code = rx.recv_timeout(timeout).ok();
        self.shared.lock().unwrap().pick_tx = None;
        code
    }
}

fn handle(
    shared: &Mutex<Shared>,
    on_hold: &(dyn Fn(bool) + Send + Sync),
    port: &AtomicUsize,
    event_type: CGEventType,
    event: &CGEvent,
) -> CallbackResult {
    if matches!(
        event_type,
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
    ) {
        // macOS switches a tap off when a callback is slow. Switch it back on.
        let tap = port.load(Ordering::SeqCst) as *mut c_void;
        if !tap.is_null() {
            unsafe { CGEventTapEnable(tap, true) };
        }
        return CallbackResult::Keep;
    }

    let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
    // Modifier keys only ever send "flags changed"; the device-dependent flag
    // bit says whether this particular modifier is down.
    let pressed = match event_type {
        CGEventType::KeyDown => true,
        CGEventType::KeyUp => false,
        CGEventType::FlagsChanged => match keycodes::modifier_mask(code) {
            Some(mask) => event.get_flags().bits() & mask != 0,
            None => return CallbackResult::Keep,
        },
        _ => return CallbackResult::Keep,
    };
    let autorepeat = matches!(event_type, CGEventType::KeyDown)
        && event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
    let is_modifier = matches!(event_type, CGEventType::FlagsChanged);
    let result = if is_modifier { CallbackResult::Keep } else { CallbackResult::Drop };

    let mut state = shared.lock().unwrap();

    if pressed && !autorepeat {
        if let Some(tx) = state.pick_tx.take() {
            let _ = tx.send(code);
            return result;
        }
    }

    if state.hotkey != Some(code) {
        return CallbackResult::Keep;
    }
    if autorepeat {
        return result;
    }
    let changed = pressed != state.down;
    state.down = pressed;
    drop(state);
    if changed {
        on_hold(pressed);
    }
    result
}
