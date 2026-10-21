//! Runs one spoken turn from key press to transcript and drives the overlay
//! through it: Listening while the key is held, Thinking while the transcript
//! comes back, then Done or Error.

use std::{
    thread,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{channel, Sender},
        Arc, Mutex,
    },
};

use tauri::{menu::MenuItem, AppHandle, Wry};

use super::{
    agent::{Agent, Confirmation, Hooks, Outcome as TaskOutcome, Report},
    memory::Conversation,
    narrator::{Narrate, Narrator},
    overlay::apply_state,
    state::{State, StateHub},
};
use crate::{
    platform::macos::adapter::MacAdapter,
    safety::confirm::await_answer,
    voice::{
        listen::{ListenError, Listener, Outcome, Utterance},
        speak::{say, say_readback},
        stt::SttError,
        transcript,
    },
};

/// How long Chirp waits for a spoken yes or no before treating silence as a no.
const CONFIRM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// How long the Done or Error look stays up after a task, so it can be seen and heard.
const RESULT_HOLD: std::time::Duration = std::time::Duration::from_millis(900);

pub struct Session {
    app: AppHandle,
    hub: StateHub,
    listener: Listener,
    /// Dev aid: shows the last transcript in the menu. Held in memory only.
    transcript_item: MenuItem<Wry>,
    current: Mutex<Option<Utterance>>,
    /// Bumped on every key press so a slow result cannot overwrite a newer turn.
    generation: AtomicU64,
    said_no_key: AtomicBool,
    agent: Arc<Agent<MacAdapter>>,
    narrator: Arc<Narrator>,
    /// One task runs at a time; a new one waits for the old to notice it was interrupted.
    task_lock: Mutex<()>,
    /// What was said recently, so follow-ups make sense. In memory only.
    memory: Mutex<Conversation>,
    /// Set while Chirp waits for a spoken yes or no; the next transcript goes here
    /// instead of starting a new task.
    confirm_tx: Mutex<Option<Sender<String>>>,
}

impl Session {
    pub fn new(
        app: AppHandle,
        hub: StateHub,
        listener: Listener,
        transcript_item: MenuItem<Wry>,
        agent: Arc<Agent<MacAdapter>>,
        narrator: Arc<Narrator>,
    ) -> Arc<Self> {
        Arc::new(Session {
            app,
            hub,
            listener,
            transcript_item,
            current: Mutex::new(None),
            generation: AtomicU64::new(0),
            said_no_key: AtomicBool::new(false),
            agent,
            narrator,
            task_lock: Mutex::new(()),
            memory: Mutex::new(Conversation::new()),
            confirm_tx: Mutex::new(None),
        })
    }

    /// Called when the talk key goes down (`true`) or up (`false`).
    pub fn on_hold(self: &Arc<Self>, held: bool) {
        if held {
            // While Chirp waits for a yes or no, the key means "I am answering": the
            // question stops and the task keeps waiting. Otherwise it interrupts.
            if self.confirm_tx.lock().unwrap().is_some() {
                self.narrator.cancel();
                apply_state(&self.app, &self.hub, State::Listening);
                *self.current.lock().unwrap() = Some(self.listener.begin());
                return;
            }
            // Pressing the talk key always interrupts Chirp: speech stops, queued
            // lines are dropped, and a running task stops before its next step.
            self.narrator.cancel();
            self.generation.fetch_add(1, Ordering::SeqCst);
            apply_state(&self.app, &self.hub, State::Listening);
            *self.current.lock().unwrap() = Some(self.listener.begin());
            return;
        }
        let Some(utterance) = self.current.lock().unwrap().take() else {
            apply_state(&self.app, &self.hub, State::Idle);
            return;
        };
        let generation = self.generation.load(Ordering::SeqCst);
        apply_state(&self.app, &self.hub, State::Thinking);
        let session = self.clone();
        utterance.end(move |outcome| session.finish(generation, outcome));
    }

    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    fn set_state(&self, generation: u64, state: State) {
        if self.is_current(generation) {
            apply_state(&self.app, &self.hub, state);
        }
    }

    fn finish(self: &Arc<Self>, generation: u64, outcome: Outcome) {
        eprintln!("[chirp] timings: {}", outcome.timing_line());

        // Waiting for a yes or no: this utterance is the answer, whatever it says.
        if let Some(tx) = self.confirm_tx.lock().unwrap().clone() {
            let heard = if outcome.error.is_none() { transcript::clean(&outcome.text).text } else { String::new() };
            eprintln!("[chirp] answer to the question: {heard:?}");
            let _ = tx.send(heard);
            return;
        }

        match &outcome.error {
            None if !outcome.text.is_empty() => {
                let language = outcome.language.as_deref().unwrap_or("?");
                let segments = outcome.stt.as_ref().map(|s| s.segments).unwrap_or(0);
                eprintln!(
                    "[chirp] transcript [{} ms, {language}, {segments} segment(s), {:.1} s of audio]: {}",
                    outcome.timings.stt_final_ms.unwrap_or(0),
                    outcome.audio_seconds,
                    outcome.text
                );
                // The command is what remains once the user's address to Chirp is removed.
                let cleaned = transcript::clean(&outcome.text);
                if cleaned.addressed {
                    eprintln!("[chirp] command after removing the name: {:?}", cleaned.text);
                }
                let shown = if cleaned.text.is_empty() { &outcome.text } else { &cleaned.text };
                let _ = self.transcript_item.set_text(menu_text(shown));
                if cleaned.text.is_empty() {
                    // Only the name was said.
                    self.set_state(generation, State::Done);
                    if self.is_current(generation) {
                        say("I'm here. What should I do?");
                    }
                } else if self.is_current(generation) {
                    // The command goes to the agent, which runs it and narrates.
                    say_readback(&format!("You said: {}", cleaned.text));
                    self.start_task(generation, cleaned.text.clone(), outcome.timings.stt_final_ms);
                    return;
                }
            }
            None => self.fail(generation, "Sorry, I didn't catch that."),
            Some(ListenError::TooShort) => {}
            Some(ListenError::NoKey) => {
                if !self.said_no_key.swap(true, Ordering::SeqCst) {
                    self.fail(generation, "I can't hear yet. Add a Sarvam key.");
                }
            }
            Some(ListenError::BudgetReached) => self.fail(generation, "My speech credit is used up."),
            Some(ListenError::NoMicrophone(reason)) => {
                eprintln!("[chirp] microphone: {reason}");
                self.fail(generation, "I can't reach the microphone.");
            }
            Some(ListenError::Stt(error)) => {
                eprintln!("[chirp] speech service: {error}");
                let line = match error {
                    SttError::Server { .. } => "The speech service refused that.",
                    _ => "I couldn't reach the speech service.",
                };
                self.fail(generation, line);
            }
        }
        self.set_state(generation, State::Idle);
    }

    /// Runs a request on its own thread so the key listener and the overlay stay responsive.
    fn start_task(self: &Arc<Self>, generation: u64, text: String, stt_ms: Option<u64>) {
        let session = self.clone();
        thread::Builder::new()
            .name("chirp-task".into())
            .spawn(move || {
                let _one_at_a_time = session.task_lock.lock().unwrap();
                if !session.is_current(generation) {
                    return;
                }
                let context = session.memory.lock().unwrap().prompt();
                let report = session.agent.run(
                    &text,
                    &context,
                    &Hooks {
                        cancelled: &|| !session.is_current(generation),
                        on_state: &|state| session.set_state(generation, state),
                        confirm: &|question| session.confirm(generation, question),
                    },
                );
                session.conclude(generation, &text, report, stt_ms);
            })
            .expect("could not start the task thread");
    }

    /// Asks a yes or no question aloud and waits for the spoken answer (the user
    /// holds the talk key to give it). Silence, or an answer that is not clearly a
    /// yes, is a no: an irreversible step never runs on a guess.
    fn confirm(&self, generation: u64, question: &str) -> Confirmation {
        let (tx, rx) = channel::<String>();
        *self.confirm_tx.lock().unwrap() = Some(tx);
        self.set_state(generation, State::Confirming);
        self.narrator.dynamic(question);
        let answer = match await_answer(&rx, CONFIRM_TIMEOUT, || self.is_current(generation), || {
            self.set_state(generation, State::Confirming);
            self.narrator.fixed("Sorry, was that a yes or a no?");
        }) {
            Some(true) => Confirmation::Yes,
            Some(false) => Confirmation::No,
            None => Confirmation::Cancelled,
        };
        *self.confirm_tx.lock().unwrap() = None;
        eprintln!("[chirp] confirmation: {answer:?}");
        answer
    }

    /// Reports how a task ended: the overlay state, the spoken result and the timing log.
    fn conclude(&self, generation: u64, said: &str, report: Report, stt_ms: Option<u64>) {
        eprintln!(
            "[chirp] task {:?}: {} step(s), {} model call(s), {}+{} tokens; intent {} ms, first action {} ms, first speech {} ms, total {} ms (speech-to-text {} ms){}",
            report.outcome,
            report.log.len(),
            report.llm_calls,
            report.input_tokens,
            report.output_tokens,
            report.timings.intent_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            report.timings.first_action_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            report.timings.first_speech_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            report.timings.total_ms,
            stt_ms.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
            if report.fast_path { " [fast path]" } else { "" },
        );
        for entry in &report.log {
            eprintln!(
                "[chirp]   step {} [{}] {} ({:?}{}) -> {}{} ({} ms)",
                entry.step,
                entry.subtask,
                entry.action,
                entry.risk,
                match entry.confirmed {
                    Some(true) => ", you said yes",
                    Some(false) => ", you said no",
                    None => "",
                },
                if entry.ok { "ok: " } else { "FAILED: " },
                entry.result,
                entry.ms
            );
        }
        if !self.is_current(generation) {
            return;
        }
        // What Chirp said or did, for the conversation memory.
        let chirp_side = match &report.outcome {
            TaskOutcome::Done(text) | TaskOutcome::Reply(text) | TaskOutcome::Failed(text) | TaskOutcome::Refused(text) if !text.is_empty() => text.clone(),
            TaskOutcome::Ask(question) => question.clone(),
            TaskOutcome::NoModel => "I needed a language model key for that.".into(),
            _ => "Done.".into(),
        };
        self.memory.lock().unwrap().push(said, &chirp_side);
        match report.outcome {
            TaskOutcome::Done(_) | TaskOutcome::Reply(_) => self.set_state(generation, State::Done),
            TaskOutcome::Ask(question) => {
                self.set_state(generation, State::Confirming);
                self.narrator.dynamic(&question);
            }
            TaskOutcome::Failed(line) | TaskOutcome::Refused(line) => {
                self.set_state(generation, State::Error);
                self.narrator.dynamic(&line);
            }
            TaskOutcome::NoModel => {
                self.set_state(generation, State::Error);
                self.narrator.fixed("I need a language model key for that.");
            }
            TaskOutcome::Cancelled => {}
        }
        thread::sleep(RESULT_HOLD);
        self.set_state(generation, State::Idle);
    }

    fn fail(&self, generation: u64, line: &str) {
        self.set_state(generation, State::Error);
        if self.is_current(generation) {
            say(line);
        }
    }
}

/// `Heard: "…"`, shortened so a long sentence does not stretch the menu.
fn menu_text(text: &str) -> String {
    let short: String = text.chars().take(40).collect();
    if text.chars().count() > 40 {
        format!("Heard: \"{short}…\"")
    } else {
        format!("Heard: \"{short}\"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_shown_whole() {
        assert_eq!(menu_text("open notes"), "Heard: \"open notes\"");
    }

    #[test]
    fn long_text_is_cut_on_character_boundaries() {
        let hindi = "नमस्ते ".repeat(20);
        let shown = menu_text(&hindi);
        assert!(shown.ends_with("…\""));
        assert_eq!(shown.chars().filter(|c| *c == '…').count(), 1);
    }
}
