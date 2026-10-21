//! Plays a sound cue on each state change, so every state can be heard as well
//! as seen. Driven only by the state machine.

use std::thread;

use super::state::{State, StateHub};
use crate::voice::{earcons, playback::Player};

/// Cues are on unless `CHIRP_CUES=0`.
pub fn start(hub: &StateHub) {
    if std::env::var("CHIRP_CUES").map(|v| v == "0").unwrap_or(false) {
        return;
    }
    let states = hub.subscribe();
    thread::Builder::new()
        .name("chirp-cues".into())
        .spawn(move || {
            for state in states {
                let cue = match state {
                    State::Listening => earcons::listening(),
                    State::Thinking => earcons::thinking(),
                    State::Done => earcons::success(),
                    State::Error => earcons::error(),
                    // Idle is silent, and Acting and Confirming are spoken instead.
                    State::Idle | State::Acting | State::Confirming => continue,
                };
                match Player::global() {
                    Some(player) => {
                        eprintln!("[chirp] cue for {state:?} ({} ms)", cue.len() as u64 * 1000 / earcons::RATE as u64);
                        player.play(&cue, earcons::RATE);
                    }
                    None => eprintln!("[chirp] cue for {state:?} skipped: no audio output"),
                }
            }
        })
        .expect("cue thread");
}
