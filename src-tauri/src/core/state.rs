use std::sync::{mpsc, Arc, Condvar, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

pub const STATE_EVENT: &str = "chirp://state";

/// The single source of truth for what Chirp is doing. The overlay and the
/// sound cues are driven only by this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Idle,
    Listening,
    Thinking,
    Acting,
    Confirming,
    Done,
    Error,
}

/// Shared state plus a condvar so background threads can sleep while Idle.
#[derive(Clone)]
pub struct StateHub {
    inner: Arc<(Mutex<State>, Condvar)>,
    subscribers: Arc<Mutex<Vec<mpsc::Sender<State>>>>,
}

impl StateHub {
    pub fn new() -> Self {
        Self {
            inner: Arc::new((Mutex::new(State::Idle), Condvar::new())),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn set(&self, app: &AppHandle, state: State) {
        *self.inner.0.lock().unwrap() = state;
        self.inner.1.notify_all();
        let _ = app.emit(STATE_EVENT, state);
        // Tell subscribers such as the sound cues, dropping any that have gone.
        self.subscribers.lock().unwrap().retain(|tx| tx.send(state).is_ok());
    }

    /// A stream of every state change from now on.
    pub fn subscribe(&self) -> mpsc::Receiver<State> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Blocks while the state is Idle, then returns the current state.
    pub fn wait_until_active(&self) -> State {
        let (lock, cv) = &*self.inner;
        let guard = cv
            .wait_while(lock.lock().unwrap(), |s| *s == State::Idle)
            .unwrap();
        *guard
    }
}
