//! Speaks what Chirp is doing, in order, without making the work wait for the
//! speech. Lines go to one thread that says them one by one; pressing the talk
//! key calls `cancel`, which drops everything still queued.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{channel, Sender},
        Arc, Mutex,
    },
    thread,
};

use crate::voice::speak::{say, say_uncached, stop};

/// What the agent needs from speech, so tests can listen instead of speak.
pub trait Narrate: Send + Sync {
    /// A fixed phrase of the app's own, cached on disk after the first time.
    fn fixed(&self, line: &str);
    /// A line that may contain the user's words or screen text. Never cached.
    fn dynamic(&self, line: &str);
    /// Drops queued lines and cuts off the one being spoken.
    fn cancel(&self);
}

enum Kind {
    Fixed,
    Dynamic,
}

pub struct Narrator {
    tx: Mutex<Sender<(u64, Kind, String)>>,
    generation: Arc<AtomicU64>,
}

impl Narrator {
    pub fn start() -> Arc<Self> {
        let (tx, rx) = channel::<(u64, Kind, String)>();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        thread::Builder::new()
            .name("chirp-narrator".into())
            .spawn(move || {
                while let Ok((generation, kind, line)) = rx.recv() {
                    if generation != current.load(Ordering::SeqCst) {
                        continue; // cancelled while it waited
                    }
                    match kind {
                        Kind::Fixed => say(&line),
                        Kind::Dynamic => say_uncached(&line),
                    }
                }
            })
            .expect("could not start the narrator thread");
        Arc::new(Narrator { tx: Mutex::new(tx), generation })
    }

    fn send(&self, kind: Kind, line: &str) {
        // A line that is one of Chirp's fixed phrases may be cached and may have a
        // recording, even if the caller sent it as ordinary text.
        let kind = if crate::voice::phrases::is_fixed(line) { Kind::Fixed } else { kind };
        let generation = self.generation.load(Ordering::SeqCst);
        let _ = self.tx.lock().unwrap().send((generation, kind, line.to_string()));
    }
}

impl Narrate for Narrator {
    fn fixed(&self, line: &str) {
        self.send(Kind::Fixed, line);
    }

    fn dynamic(&self, line: &str) {
        self.send(Kind::Dynamic, line);
    }

    fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        stop();
    }
}
