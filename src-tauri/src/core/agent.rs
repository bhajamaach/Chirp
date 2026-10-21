//! The agent: turns one request into finished work.
//!
//! 1. Simple commands (open an app, scroll, back) run from rules, with no model.
//! 2. Anything else: one call reads the request into an `Intent`; then a loop
//!    breaks the work into subtasks and does them one action at a time. Every
//!    turn the model sees the plan, what has been done and an outline of the
//!    screen, picks the next action, and Chirp checks it, runs it and looks again.
//!
//! The loop only talks to `PlatformAdapter`, `LlmClient` and `Narrate`, so it is
//! tested end to end with mocks, without touching a real app.

use std::{
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use super::{
    fastpath::{self, Direction, Fast},
    narrator::Narrate,
    state::State,
};
use crate::{
    llm::{
        schema::{self, Action, BadStep, Risk, Step},
        LlmClient, LlmError, LlmReply,
    },
    platform::{AdapterError, ElementQuery, PlatformAdapter},
    safety::{self, log::ActionLog, Verdict},
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
pub struct Limits {
    /// Most actions in one task before Chirp gives up and hands control back.
    pub max_steps: usize,
    /// Failed actions in a row before giving up.
    pub max_failures: usize,
    /// Most step narrations spoken in one task (the final line is always spoken).
    pub max_narrations: usize,
    /// Characters of screen outline sent to the model each turn.
    pub outline_chars: usize,
    pub tree_depth: usize,
    /// Pause after an action so the screen can catch up before it is read.
    pub settle: Duration,
    /// Scales `wait` actions; tests set it to zero.
    pub wait_scale: f32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_steps: 12,
            max_failures: 3,
            max_narrations: 3,
            outline_chars: 2500,
            tree_depth: 8,
            settle: Duration::from_millis(250),
            wait_scale: 1.0,
        }
    }
}

/// How a task ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Finished; the text is the result or answer.
    Done(String),
    /// A conversational answer, with nothing done on the computer.
    Reply(String),
    /// Needs one fact from the user; the text is the question.
    Ask(String),
    /// Could not be done; the text says why. Control goes back to the user.
    Failed(String),
    /// Safety refused to run a step.
    Refused(String),
    /// The user interrupted.
    Cancelled,
    /// The request needs a language model and none has a key.
    NoModel,
}

/// One action that was tried. Actions, not content: typed text and scripts are
/// described by their size only.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub step: usize,
    pub subtask: String,
    pub action: String,
    pub risk: Risk,
    /// `Some(true)` the user said yes to it, `Some(false)` no, `None` not asked.
    pub confirmed: Option<bool>,
    pub ok: bool,
    pub result: String,
    pub ms: u128,
}

#[derive(Clone, Debug, Default)]
pub struct Timings {
    /// Request received to the intent being ready.
    pub intent_ms: Option<u128>,
    /// Request received to the first action starting.
    pub first_action_ms: Option<u128>,
    /// Request received to the first spoken line being queued.
    pub first_speech_ms: Option<u128>,
    pub total_ms: u128,
}

#[derive(Clone, Debug)]
pub struct Report {
    pub outcome: Outcome,
    pub log: Vec<LogEntry>,
    pub plan: Vec<String>,
    pub timings: Timings,
    pub llm_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub fast_path: bool,
}

/// The user's answer to "Is that okay?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Confirmation {
    Yes,
    No,
    /// The user interrupted instead of answering.
    Cancelled,
}

/// What the caller wants to know and control while a task runs.
pub struct Hooks<'a> {
    /// True once the user has interrupted. Checked before every step.
    pub cancelled: &'a dyn Fn() -> bool,
    pub on_state: &'a dyn Fn(State),
    /// Speaks the question, listens, and returns the answer. Only an explicit
    /// `Yes` lets an irreversible step run.
    pub confirm: &'a dyn Fn(&str) -> Confirmation,
}

pub struct Agent<A: PlatformAdapter> {
    adapter: A,
    llm: Option<Arc<dyn LlmClient>>,
    narrator: Arc<dyn Narrate>,
    log: Arc<ActionLog>,
    limits: Limits,
    persona: schema::Persona,
    /// Rotates the spoken acknowledgements so Chirp does not say the same thing every time.
    acknowledged: AtomicUsize,
}

struct Run<'a> {
    started: Instant,
    task: u64,
    timings: Timings,
    log: Vec<LogEntry>,
    plan: Vec<String>,
    llm_calls: usize,
    input_tokens: u64,
    output_tokens: u64,
    narrations: usize,
    /// The last line spoken, so the same words are never said twice running.
    last_line: String,
    hooks: &'a Hooks<'a>,
}

impl Run<'_> {
    fn note_speech(&mut self) {
        self.timings.first_speech_ms.get_or_insert(self.started.elapsed().as_millis());
    }

    fn count(&mut self, reply: &LlmReply) {
        self.llm_calls += 1;
        self.input_tokens += reply.input_tokens;
        self.output_tokens += reply.output_tokens;
    }

    fn finish(mut self, outcome: Outcome, fast_path: bool) -> Report {
        self.timings.total_ms = self.started.elapsed().as_millis();
        Report {
            outcome,
            log: self.log,
            plan: self.plan,
            timings: self.timings,
            llm_calls: self.llm_calls,
            input_tokens: self.input_tokens,
            output_tokens: self.output_tokens,
            fast_path,
        }
    }
}

impl<A: PlatformAdapter> Agent<A> {
    pub fn new(
        adapter: A,
        llm: Option<Arc<dyn LlmClient>>,
        narrator: Arc<dyn Narrate>,
        log: Arc<ActionLog>,
        limits: Limits,
    ) -> Self {
        Agent { adapter, llm, narrator, log, limits, persona: schema::Persona::from_env(), acknowledged: AtomicUsize::new(0) }
    }

    /// Runs one request to the end. Blocks; call it from a worker thread.
    /// `context` is the recent conversation (may be empty) so follow-ups make sense.
    pub fn run(&self, request: &str, context: &str, hooks: &Hooks) -> Report {
        let mut run = Run {
            started: Instant::now(),
            task: self.log.new_task_id(),
            timings: Timings::default(),
            log: Vec::new(),
            plan: Vec::new(),
            llm_calls: 0,
            input_tokens: 0,
            output_tokens: 0,
            narrations: 0,
            last_line: String::new(),
            hooks,
        };
        if (hooks.cancelled)() {
            return run.finish(Outcome::Cancelled, false);
        }
        if let Some(fast) = fastpath::parse(request) {
            let outcome = self.run_fast(fast, &mut run);
            return run.finish(outcome, true);
        }
        let Some(llm) = self.llm.clone() else {
            return run.finish(Outcome::NoModel, false);
        };
        let outcome = self.run_planned(request, context, llm.as_ref(), &mut run);
        run.finish(outcome, false)
    }

    // ---- fast path -------------------------------------------------------------

    fn run_fast(&self, fast: Fast, run: &mut Run) -> Outcome {
        (run.hooks.on_state)(State::Acting);
        run.timings.first_action_ms = Some(run.started.elapsed().as_millis());
        let (action, result): (String, Result<Outcome, AdapterError>) = match fast {
            Fast::Stop => return Outcome::Cancelled,
            Fast::Open(app) => {
                let result = self.adapter.focus_app(&app).map(|found| Outcome::Done(format!("Opened {}.", found.name)));
                (format!("open {app}"), result)
            }
            Fast::Scroll(direction) => {
                let key = if direction == Direction::Down { "pagedown" } else { "pageup" };
                (format!("scroll {direction:?}"), self.adapter.press_keys(key).map(|()| Outcome::Done(String::new())))
            }
            Fast::Back => ("go back".into(), self.adapter.press_keys("cmd+[").map(|()| Outcome::Done(String::new()))),
            Fast::WhichApp => (
                "read the front app".into(),
                self.adapter.frontmost_app().map(|app| Outcome::Done(format!("You're in {}.", app.name))),
            ),
            // Reading the log must not become the "last task" itself.
            Fast::WhatDidYouDo => {
                let summary = self.log.spoken_summary();
                self.speak(run, &summary);
                return Outcome::Done(summary);
            }
        };
        let ok = result.is_ok();
        run.log.push(LogEntry {
            step: 1,
            subtask: "fast path".into(),
            action: action.clone(),
            risk: Risk::Reversible,
            confirmed: None,
            ok,
            result: result.as_ref().map(|_| "ok".to_string()).unwrap_or_else(|e| e.to_string()),
            ms: run.started.elapsed().as_millis(),
        });
        if !matches!(fast_kind(&action), FastKind::ReadOnly) {
            self.log.record(run.task, 1, &action, &past_of(&action), Risk::Reversible, None, ok);
        }
        match result {
            Ok(outcome) => {
                if let Outcome::Done(text) = &outcome {
                    self.speak(run, text);
                }
                outcome
            }
            Err(error) => Outcome::Failed(failure_line(&error)),
        }
    }

    // ---- planner loop ----------------------------------------------------------

    fn run_planned(&self, request: &str, context: &str, llm: &dyn LlmClient, run: &mut Run) -> Outcome {
        // The quick acknowledgement is a fixed phrase, cached on disk, so it starts almost at
        // once. Conversation does not get one: an answer follows, and "On it" would be odd.
        if !looks_like_chat(request) {
            run.note_speech();
            let n = self.acknowledged.fetch_add(1, Ordering::SeqCst);
            self.narrator.fixed(acknowledgement_for(self.persona, request, n));
        }
        (run.hooks.on_state)(State::Thinking);

        let with_context = schema::with_context(context, request);
        let intent = match self.ask(llm, run, &schema::intent_system(self.persona), &with_context, "intent", &schema::intent_schema(), schema::parse_intent) {
            Ok(intent) => intent,
            Err(outcome) => return outcome,
        };
        run.timings.intent_ms = Some(run.started.elapsed().as_millis());
        if intent.needs_clarification {
            return Outcome::Ask(intent.question.clone().unwrap_or_default());
        }
        if let Some(reply) = &intent.reply {
            self.speak(run, reply);
            return Outcome::Reply(reply.clone());
        }

        let mut history: Vec<String> = Vec::new();
        let mut expected_app: Option<String> = None;
        let mut consecutive_failures = 0;
        let mut recent: Vec<(String, bool)> = Vec::new();
        let mut executed: Vec<String> = Vec::new();

        for number in 1..=self.limits.max_steps {
            if (run.hooks.cancelled)() {
                return Outcome::Cancelled;
            }
            (run.hooks.on_state)(State::Thinking);
            let (app, screen) = match self.observe() {
                Ok(seen) => seen,
                Err(error) => return Outcome::Failed(failure_line(&error)),
            };
            let prompt = schema::step_prompt(&with_context, &intent, &run.plan, &history, &app, &screen);
            let step = match self.ask(llm, run, &schema::step_system(self.persona), &prompt, "step", &schema::step_schema(), schema::parse_step) {
                Ok(step) => step,
                Err(outcome) => return outcome,
            };
            if (run.hooks.cancelled)() {
                return Outcome::Cancelled;
            }
            if !step.plan.is_empty() {
                run.plan = step.plan.clone();
            }
            let risk = safety::worst(step.risk, safety::classify(&step.action));
            let verdict = safety::check(&step.action, step.risk);
            // A step that needs a yes gets the question instead of a chatty narration.
            if let Some(line) = &step.say {
                if verdict != Verdict::Confirm && run.narrations < self.limits.max_narrations && !step.action.is_terminal() {
                    run.narrations += 1;
                    self.speak(run, line);
                }
            }
            let mut confirmed = None;
            match verdict {
                Verdict::Refuse(reason) => {
                    self.log_step(run, number, &step, risk, None, false, format!("refused: {reason}"), Instant::now());
                    self.log.record(run.task, number, &step.action.describe(), &step.action.past(), risk, None, false);
                    return Outcome::Refused(reason.into());
                }
                Verdict::Confirm => {
                    (run.hooks.on_state)(State::Confirming);
                    let question = safety::confirmation_question(&step.action, &app, step.confirm.as_deref());
                    run.note_speech();
                    match (run.hooks.confirm)(&question) {
                        Confirmation::Yes => confirmed = Some(true),
                        Confirmation::No => {
                            self.log_step(run, number, &step, risk, Some(false), false, "the user said no".into(), Instant::now());
                            self.log.record(run.task, number, &step.action.describe(), &step.action.past(), risk, Some(false), false);
                            let line = "Okay, I'll leave it.".to_string();
                            self.speak(run, &line);
                            return Outcome::Done(line);
                        }
                        Confirmation::Cancelled => return Outcome::Cancelled,
                    }
                }
                Verdict::Allow => {}
            }
            match &step.action {
                Action::Done { message } => {
                    self.log_step(run, number, &step, risk, None, true, "finished".into(), Instant::now());
                    self.speak(run, message);
                    return Outcome::Done(message.clone());
                }
                Action::Ask { question } => {
                    self.log_step(run, number, &step, risk, None, true, "asked".into(), Instant::now());
                    return Outcome::Ask(question.clone());
                }
                Action::Fail { message } => {
                    self.log_step(run, number, &step, risk, None, false, "gave up".into(), Instant::now());
                    return Outcome::Failed(message.clone());
                }
                _ => {}
            }

            (run.hooks.on_state)(State::Acting);
            run.timings.first_action_ms.get_or_insert(run.started.elapsed().as_millis());
            let began = Instant::now();
            let result = self.execute(&step.action, &mut expected_app);
            let (ok, text) = match &result {
                Ok(text) => (true, text.clone()),
                Err(error) => (false, error.to_string()),
            };
            self.log_step(run, number, &step, risk, confirmed, ok, text.clone(), began);
            self.log.record(run.task, number, &step.action.describe(), &step.action.past(), risk, confirmed, ok);
            history.push(format!("{number}. [{}] {} -> {}", step.subtask, step.action.describe(), if ok { text } else { format!("FAILED: {text}") }));
            if history.len() > 8 {
                history.remove(0);
            }

            consecutive_failures = if ok { 0 } else { consecutive_failures + 1 };
            if consecutive_failures >= self.limits.max_failures {
                return Outcome::Failed("I couldn't get that to work.".into());
            }
            let described = step.action.describe();
            executed.push(described.clone());
            let last_five = &executed[executed.len().saturating_sub(5)..];
            if !matches!(step.action, Action::Wait { .. }) && last_five.iter().filter(|d| **d == described).count() >= 3 {
                return Outcome::Failed("I seem to be going in circles, so I'll stop here.".into());
            }
            recent.push((described, ok));
            if recent.len() >= 3 && recent[recent.len() - 3..].iter().all(|r| *r == recent[recent.len() - 1]) && !ok {
                return Outcome::Failed("I'm going in circles, so I'll stop.".into());
            }
            thread::sleep(self.limits.settle);
        }
        Outcome::Failed("That took too many steps, so I stopped.".into())
    }

    /// One model call whose reply must parse. A reply that does not gets one retry
    /// that says what was wrong, then the task ends with a spoken fallback.
    fn ask<T>(
        &self,
        llm: &dyn LlmClient,
        run: &mut Run,
        system: &str,
        user: &str,
        name: &str,
        schema: &serde_json::Value,
        parse: fn(&serde_json::Value) -> Result<T, BadStep>,
    ) -> Result<T, Outcome> {
        let mut message = user.to_string();
        for attempt in 0..2 {
            if (run.hooks.cancelled)() {
                return Err(Outcome::Cancelled);
            }
            let reply = llm.complete(system, &message, name, schema).map_err(|e| match e {
                LlmError::NoProvider => Outcome::NoModel,
                LlmError::BudgetReached => Outcome::Failed("My credit is used up.".into()),
                LlmError::Failed(why) => {
                    eprintln!("[chirp] the language model failed: {why}");
                    Outcome::Failed("I couldn't reach my thinking service.".into())
                }
            })?;
            run.count(&reply);
            match parse(&reply.value) {
                Ok(parsed) => return Ok(parsed),
                Err(bad) if attempt == 0 => {
                    eprintln!("[chirp] model reply rejected, retrying: {bad}");
                    message = format!("{user}\n\nYour last reply was invalid ({bad}). Reply again with a valid JSON object.");
                }
                Err(bad) => {
                    eprintln!("[chirp] model reply rejected twice: {bad}");
                    return Err(Outcome::Failed("I didn't understand how to do that.".into()));
                }
            }
        }
        Err(Outcome::Failed("I didn't understand how to do that.".into()))
    }

    /// The app in front and a short outline of its window.
    fn observe(&self) -> Result<(String, String), AdapterError> {
        let app = self.adapter.frontmost_app()?;
        let tree = self.adapter.get_ui_tree(self.limits.tree_depth)?;
        Ok((app.name, tree.outline(self.limits.outline_chars)))
    }

    fn execute(&self, action: &Action, expected_app: &mut Option<String>) -> Result<String, AdapterError> {
        match action {
            Action::FocusApp { app } => {
                let found = self.adapter.focus_app(app)?;
                *expected_app = Some(found.name.clone());
                Ok(format!("{} is in front", found.name))
            }
            Action::Press { label, role } => {
                self.ensure_front(expected_app)?;
                let query = ElementQuery { text: label.clone(), role: role.clone() };
                let hits = self.adapter.find_element(&query)?;
                let how = self.adapter.perform(&hits[0].0, crate::platform::Action::Press)?;
                Ok(format!("pressed ({how:?})"))
            }
            Action::Type { text } => {
                self.ensure_front(expected_app)?;
                self.adapter.type_text(text)?;
                let count = text.chars().count();
                // Read the field back, so the model is told whether the text arrived
                // instead of having to guess from an outline (and then "saving" or retrying).
                Ok(match self.typed_text_is_visible(text) {
                    Some(true) => format!("typed {count} characters; the text now shows in the field. If that was everything asked, the next action is done"),
                    Some(false) => format!("typed {count} characters; the field does not show it yet"),
                    None => format!("typed {count} characters"),
                })
            }
            Action::Keys { keys } => {
                self.ensure_front(expected_app)?;
                self.adapter.press_keys(keys)?;
                Ok("keys pressed".into())
            }
            Action::Script { script } => {
                let output = self.adapter.run_os_script(script)?;
                // The script's output is not echoed: it can hold the user's content.
                Ok(if output.is_empty() { "script ran".into() } else { format!("script ran, returned {} characters", output.chars().count()) })
            }
            Action::Wait { ms } => {
                thread::sleep(Duration::from_millis(*ms).mul_f32(self.limits.wait_scale));
                Ok(format!("waited {ms} ms"))
            }
            Action::Done { .. } | Action::Ask { .. } | Action::Fail { .. } => Ok(String::new()),
        }
    }

    /// Whether the start of `typed` appears in the focused text field. `None` when
    /// that cannot be told (no focused field found, or the field's text is cut off).
    fn typed_text_is_visible(&self, typed: &str) -> Option<bool> {
        thread::sleep(self.limits.settle.min(Duration::from_millis(150)));
        let tree = self.adapter.get_ui_tree(self.limits.tree_depth).ok()?;
        let value = focused_value(&tree)?;
        let flat = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
        let snippet: String = flat(typed).chars().take(20).collect();
        if snippet.is_empty() {
            return None;
        }
        let shown = flat(&value);
        if shown.contains(&snippet) {
            Some(true)
        } else if shown.chars().count() >= 190 {
            None // the field's text is cut at 200 characters, so absence proves nothing
        } else {
            Some(false)
        }
    }

    /// Keys and typing go to whatever is in front, so make sure it is the app the
    /// task is working in. Something else may have taken the front (a chat
    /// message, a system dialog); ask once, and if that fails, do not type.
    fn ensure_front(&self, expected_app: &Option<String>) -> Result<(), AdapterError> {
        let Some(expected) = expected_app else { return Ok(()) };
        let front = self.adapter.frontmost_app()?;
        if front.name.eq_ignore_ascii_case(expected) {
            return Ok(());
        }
        let refocused = self.adapter.focus_app(expected)?;
        if refocused.name.eq_ignore_ascii_case(expected) { Ok(()) } else { Err(AdapterError::NoApp(format!("{expected} is not in front"))) }
    }

    fn speak(&self, run: &mut Run, line: &str) {
        if line.trim().is_empty() || run.last_line.eq_ignore_ascii_case(line.trim()) {
            return;
        }
        run.last_line = line.trim().to_string();
        run.note_speech();
        self.narrator.dynamic(line);
    }

    #[allow(clippy::too_many_arguments)]
    fn log_step(&self, run: &mut Run, number: usize, step: &Step, risk: Risk, confirmed: Option<bool>, ok: bool, result: String, began: Instant) {
        run.log.push(LogEntry {
            step: number,
            subtask: step.subtask.clone(),
            action: step.action.describe(),
            risk,
            confirmed,
            ok,
            result,
            ms: began.elapsed().as_millis(),
        });
    }
}

/// The value of the focused text field or area, if the tree has one.
fn focused_value(node: &crate::platform::UiNode) -> Option<String> {
    if node.info.focused && matches!(node.info.role.as_str(), "AXTextArea" | "AXTextField" | "AXComboBox") {
        return Some(node.info.value.clone().unwrap_or_default());
    }
    node.children.iter().find_map(focused_value)
}

/// Whether a request is probably conversation (a greeting, thanks, a question), so
/// the quick "On it." acknowledgement is skipped. Only a guess: a wrong guess just
/// means a missing or slightly odd acknowledgement.
pub fn looks_like_chat(request: &str) -> bool {
    let lower = request.trim().to_lowercase();
    if lower.contains('?') {
        return true;
    }
    // Whitespace, not "alphanumeric", so Devanagari words are not cut at their vowel signs.
    let first = lower.split_whitespace().next().unwrap_or("").trim_matches(|c: char| c.is_ascii_punctuation() || c == '।');
    const OPENERS: [&str; 22] = [
        "hi", "hello", "hey", "thanks", "thank", "namaste", "namaskar", "who", "what", "why", "how", "when", "where", "are",
        "is", "can", "could", "do", "does", "tell", "good", "नमस्ते",
    ];
    // "what did you just do" and the like are handled by rules before this is asked.
    OPENERS.contains(&first) || first == "शुक्रिया" || first == "धन्यवाद" || first == "क्या" || first == "कैसे"
}

/// Short spoken acknowledgements, rotated so Chirp does not say the same thing every time.
pub const PLAIN_ACKS: [&str; 4] = ["On it.", "Sure.", "Okay, one moment.", "Got it."];
pub const SASSY_ACKS: [&str; 7] = ["On it.", "Say less.", "Consider it done.", "Ooh, fun. One sec.", "You got it, boss.", "Easy. Watch this.", "Already on it."];
pub const HINDI_ACKS: [&str; 3] = ["ठीक है।", "अभी करता हूँ।", "एक पल।"];
pub const HINDI_SASSY_ACKS: [&str; 4] = ["ठीक है।", "अभी करता हूँ।", "हो गया समझो।", "एक पल।"];

pub fn acknowledgement_for(persona: schema::Persona, request: &str, n: usize) -> &'static str {
    let hindi = request.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c));
    let set: &[&'static str] = match (hindi, persona) {
        (true, schema::Persona::Sassy) => &HINDI_SASSY_ACKS,
        (true, _) => &HINDI_ACKS,
        (false, schema::Persona::Sassy) => &SASSY_ACKS,
        (false, _) => &PLAIN_ACKS,
    };
    set[n % set.len()]
}

enum FastKind {
    ReadOnly,
    Acts,
}

fn fast_kind(action: &str) -> FastKind {
    if action.starts_with("read ") { FastKind::ReadOnly } else { FastKind::Acts }
}

/// Past tense of a fast-path action description, for the log.
fn past_of(action: &str) -> String {
    match action.split_once(' ') {
        Some(("open", app)) => format!("opened {app}"),
        Some(("scroll", dir)) => format!("scrolled {}", dir.to_lowercase()),
        Some(("go", "back")) => "went back".into(),
        _ => action.to_string(),
    }
}

/// A short spoken reason for an adapter error.
pub fn failure_line(error: &AdapterError) -> String {
    match error {
        AdapterError::NotTrusted => "I need Accessibility permission to do that.".into(),
        AdapterError::NoApp(_) => "I couldn't find that app.".into(),
        AdapterError::NotFound(_) => "I couldn't find that on the screen.".into(),
        AdapterError::AutomationDenied(app) => format!("I'm not allowed to control {app} yet."),
        AdapterError::Blocked(_) => "I won't type into a password field.".into(),
        AdapterError::Timeout(_) => "That took too long.".into(),
        _ => "That didn't work.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Action as Act, AppInfo, ElementInfo, Performed, Result as PResult, UiNode};
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// A stand-in for the Mac: records what it is asked to do.
    #[derive(Default)]
    struct FakeMac {
        calls: Mutex<Vec<String>>,
        front: Mutex<String>,
        /// Labels that exist on screen.
        buttons: Vec<String>,
        /// Make focus_app fail for this app name.
        missing_app: Option<String>,
        /// The text a focused text field holds, if there is one.
        field: Mutex<Option<String>>,
    }

    impl FakeMac {
        fn new(buttons: &[&str]) -> Self {
            FakeMac { front: Mutex::new("Terminal".into()), buttons: buttons.iter().map(|b| b.to_string()).collect(), ..Default::default() }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl PlatformAdapter for FakeMac {
        type Element = String;

        fn frontmost_app(&self) -> PResult<AppInfo> {
            Ok(AppInfo { name: self.front.lock().unwrap().clone(), bundle_id: None, pid: 1 })
        }

        fn get_ui_tree(&self, _depth: usize) -> PResult<UiNode> {
            let info = |role: &str, label: &str| ElementInfo { role: role.into(), label: Some(label.into()), enabled: true, ..Default::default() };
            let mut children: Vec<UiNode> =
                self.buttons.iter().map(|b| UiNode { info: info("AXButton", b), children: vec![], truncated: false }).collect();
            if let Some(text) = self.field.lock().unwrap().clone() {
                let mut field = info("AXTextArea", "");
                field.label = None;
                field.value = Some(text);
                field.focused = true;
                children.push(UiNode { info: field, children: vec![], truncated: false });
            }
            Ok(UiNode { info: info("AXWindow", &self.front.lock().unwrap()), children, truncated: false })
        }

        fn find_element(&self, query: &ElementQuery) -> PResult<Vec<(String, ElementInfo)>> {
            self.buttons
                .iter()
                .find(|b| b.eq_ignore_ascii_case(&query.text))
                .map(|b| vec![(b.clone(), ElementInfo::default())])
                .ok_or_else(|| AdapterError::NotFound(query.text.clone()))
        }

        fn perform(&self, element: &String, _action: Act) -> PResult<Performed> {
            self.calls.lock().unwrap().push(format!("press {element}"));
            Ok(Performed::Directly)
        }

        fn type_text(&self, text: &str) -> PResult<()> {
            self.calls.lock().unwrap().push(format!("type {} chars into {}", text.chars().count(), self.front.lock().unwrap()));
            Ok(())
        }

        fn press_keys(&self, keys: &str) -> PResult<()> {
            self.calls.lock().unwrap().push(format!("keys {keys}"));
            Ok(())
        }

        fn focus_app(&self, name: &str) -> PResult<AppInfo> {
            if self.missing_app.as_deref() == Some(name) {
                return Err(AdapterError::NoApp(name.into()));
            }
            self.calls.lock().unwrap().push(format!("focus {name}"));
            *self.front.lock().unwrap() = name.to_string();
            Ok(AppInfo { name: name.into(), bundle_id: None, pid: 2 })
        }

        fn run_os_script(&self, script: &str) -> PResult<String> {
            self.calls.lock().unwrap().push(format!("script {} chars", script.len()));
            Ok(String::new())
        }
    }

    /// Replies from a list, in order, and remembers what it was asked.
    struct ScriptedLlm {
        replies: Mutex<Vec<Value>>,
        asked: Mutex<Vec<String>>,
    }

    impl ScriptedLlm {
        fn new(replies: Vec<Value>) -> Arc<Self> {
            Arc::new(ScriptedLlm { replies: Mutex::new(replies), asked: Mutex::new(Vec::new()) })
        }
    }

    impl LlmClient for ScriptedLlm {
        fn complete(&self, _system: &str, user: &str, _name: &str, _schema: &Value) -> Result<LlmReply, LlmError> {
            self.asked.lock().unwrap().push(user.to_string());
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err(LlmError::Failed("script ran out".into()));
            }
            Ok(LlmReply { value: replies.remove(0), input_tokens: 10, output_tokens: 5, provider: "test".into(), ms: 1 })
        }
    }

    #[derive(Default)]
    struct Heard {
        fixed: Mutex<Vec<String>>,
        dynamic: Mutex<Vec<String>>,
    }

    impl Narrate for Heard {
        fn fixed(&self, line: &str) {
            self.fixed.lock().unwrap().push(line.into());
        }
        fn dynamic(&self, line: &str) {
            self.dynamic.lock().unwrap().push(line.into());
        }
        fn cancel(&self) {}
    }

    fn intent(risk: &str, ask: Option<&str>) -> Value {
        json!({"goal": "Open Notes and write a list", "app": "Notes",
               "params": {"text": "milk, eggs, bread", "target": null, "location": null, "query": null},
               "risk": risk, "needs_clarification": ask.is_some(), "question": ask})
    }

    fn step(plan: &[&str], subtask: &str, say: Option<&str>, risk: &str, kind: &str, fields: Value) -> Value {
        let mut action = json!({"kind": kind, "app": null, "label": null, "role": null, "text": null, "keys": null,
                                "script": null, "ms": null, "message": null, "question": null});
        for (k, v) in fields.as_object().unwrap() {
            action[k] = v.clone();
        }
        json!({"plan": plan, "subtask": subtask, "say": say, "risk": risk, "action": action})
    }

    fn agent(mac: FakeMac, llm: Option<Arc<ScriptedLlm>>, heard: Arc<Heard>) -> Agent<FakeMac> {
        let limits = Limits { settle: Duration::ZERO, wait_scale: 0.0, ..Limits::default() };
        Agent::new(mac, llm.map(|l| l as Arc<dyn LlmClient>), heard, ActionLog::in_memory(), limits)
    }

    /// Runs with the user always answering `answer` to a question; the questions asked are kept.
    fn run_answering(agent: &Agent<FakeMac>, request: &str, context: &str, answer: Confirmation, asked: &Mutex<Vec<String>>) -> Report {
        agent.run(
            request,
            context,
            &Hooks {
                cancelled: &|| false,
                on_state: &|_| {},
                confirm: &|question| {
                    asked.lock().unwrap().push(question.to_string());
                    answer
                },
            },
        )
    }

    fn run(agent: &Agent<FakeMac>, request: &str) -> Report {
        // A user who says no to anything asked, so a test that asks by accident fails safe.
        run_answering(agent, request, "", Confirmation::No, &Mutex::new(Vec::new()))
    }

    #[test]
    fn a_multi_step_request_is_broken_into_subtasks_and_finished() {
        let plan = ["open Notes", "make a new note", "type the list"];
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&plan, "open Notes", Some("Opening Notes."), "reversible", "focus_app", json!({"app": "Notes"})),
            step(&[], "make a new note", None, "reversible", "press", json!({"label": "New Note", "role": "button"})),
            step(&[], "type the list", Some("Writing your list."), "reversible", "type", json!({"text": "milk, eggs, bread"})),
            step(&[], "type the list", None, "safe", "done", json!({"message": "Your list is in Notes."})),
        ]);
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&["New Note"]), Some(llm.clone()), heard.clone());
        let report = run(&agent, "open notes and write my shopping list milk eggs bread");

        assert_eq!(report.outcome, Outcome::Done("Your list is in Notes.".into()));
        assert_eq!(agent.adapter.calls(), ["focus Notes", "press New Note", "type 17 chars into Notes"]);
        assert_eq!(report.plan, plan);
        assert_eq!(report.log.len(), 4);
        assert_eq!(report.llm_calls, 5);
        assert_eq!(heard.fixed.lock().unwrap().as_slice(), ["On it."]);
        assert_eq!(heard.dynamic.lock().unwrap().as_slice(), ["Opening Notes.", "Writing your list.", "Your list is in Notes."]);
        assert!(report.timings.intent_ms.is_some() && report.timings.first_action_ms.is_some() && report.timings.first_speech_ms.is_some());
        // The model saw the plan and the earlier actions on later turns.
        let asked = llm.asked.lock().unwrap();
        assert!(asked[3].contains("1. open Notes") && asked[3].contains("press button \"New Note\""), "{}", asked[3]);
        assert!(asked[3].contains("App in front: Notes"));
    }

    #[test]
    fn the_log_holds_actions_not_content() {
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&["type"], "type", None, "reversible", "type", json!({"text": "my private diary entry"})),
            step(&[], "type", None, "safe", "done", json!({"message": "Done."})),
        ]);
        let agent = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let report = run(&agent, "write my diary entry");
        let logged: String = report.log.iter().map(|e| format!("{} {}", e.action, e.result)).collect();
        assert!(!logged.contains("diary"), "{logged}");
    }

    #[test]
    fn simple_commands_use_no_model() {
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&[]), None, heard.clone());
        let report = run(&agent, "Open Notes.");
        assert_eq!(report.outcome, Outcome::Done("Opened Notes.".into()));
        assert!(report.fast_path && report.llm_calls == 0);
        assert_eq!(agent.adapter.calls(), ["focus Notes"]);
        assert_eq!(heard.dynamic.lock().unwrap().as_slice(), ["Opened Notes."]);

        let report = run(&agent, "scroll down");
        assert_eq!(report.outcome, Outcome::Done(String::new()));
        assert_eq!(agent.adapter.calls().last().unwrap(), "keys pagedown");
    }

    #[test]
    fn a_fast_open_of_a_missing_app_fails_politely() {
        let mut mac = FakeMac::new(&[]);
        mac.missing_app = Some("Door".into());
        let agent = agent(mac, None, Arc::new(Heard::default()));
        assert_eq!(run(&agent, "open the door").outcome, Outcome::Failed("I couldn't find that app.".into()));
    }

    #[test]
    fn without_a_model_a_real_request_says_so() {
        let agent = agent(FakeMac::new(&[]), None, Arc::new(Heard::default()));
        assert_eq!(run(&agent, "open notes and write hello").outcome, Outcome::NoModel);
    }

    #[test]
    fn a_missing_fact_becomes_one_question_and_no_action() {
        let llm = ScriptedLlm::new(vec![intent("reversible", Some("Which folder?"))]);
        let agent = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let report = run(&agent, "move my file");
        assert_eq!(report.outcome, Outcome::Ask("Which folder?".into()));
        assert!(agent.adapter.calls().is_empty());
    }

    fn send_steps(risk: &str) -> Vec<Value> {
        vec![
            intent("reversible", None),
            step(&["write the email", "send it"], "send it", Some("Sending."), risk, "press", json!({"label": "Send", "role": "button"})),
            step(&[], "send it", None, "safe", "done", json!({"message": "Sent."})),
        ]
    }

    #[test]
    fn an_irreversible_step_asks_first_and_runs_only_after_a_yes() {
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&["Send"]), Some(ScriptedLlm::new(send_steps("irreversible"))), heard.clone());
        let asked = Mutex::new(Vec::new());
        let report = run_answering(&agent, "send the email", "", Confirmation::Yes, &asked);
        assert_eq!(report.outcome, Outcome::Done("Sent.".into()));
        assert_eq!(agent.adapter.calls(), ["press Send"]);
        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].contains("press Send in Terminal") && asked[0].ends_with("Is that okay?"), "{}", asked[0]);
        assert_eq!(report.log[0].confirmed, Some(true));
        // The chatty "Sending." is replaced by the question.
        assert!(!heard.dynamic.lock().unwrap().contains(&"Sending.".to_string()));
        assert_eq!(agent.log.spoken_summary(), "I pressed \"Send\" after you said yes.");
    }

    #[test]
    fn a_no_means_it_does_not_run_and_the_task_ends_kindly() {
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&["Send"]), Some(ScriptedLlm::new(send_steps("irreversible"))), heard.clone());
        let report = run(&agent, "send the email");
        assert_eq!(report.outcome, Outcome::Done("Okay, I'll leave it.".into()));
        assert!(agent.adapter.calls().is_empty(), "{:?}", agent.adapter.calls());
        assert_eq!(report.log[0].confirmed, Some(false));
        assert!(heard.dynamic.lock().unwrap().contains(&"Okay, I'll leave it.".to_string()));
        assert_eq!(agent.log.spoken_summary(), "I asked to press button \"Send\", and you said no.");
    }

    #[test]
    fn interrupting_the_question_cancels_and_runs_nothing() {
        let agent = agent(FakeMac::new(&["Send"]), Some(ScriptedLlm::new(send_steps("irreversible"))), Arc::new(Heard::default()));
        let report = run_answering(&agent, "send the email", "", Confirmation::Cancelled, &Mutex::new(Vec::new()));
        assert_eq!(report.outcome, Outcome::Cancelled);
        assert!(agent.adapter.calls().is_empty());
    }

    #[test]
    fn the_rules_catch_a_risk_the_model_calls_safe() {
        // The model labels pressing Send "safe"; the classifier does not agree.
        let agent = agent(FakeMac::new(&["Send"]), Some(ScriptedLlm::new(send_steps("safe"))), Arc::new(Heard::default()));
        let asked = Mutex::new(Vec::new());
        let report = run_answering(&agent, "send the email", "", Confirmation::No, &asked);
        assert_eq!(asked.lock().unwrap().len(), 1, "it must still ask");
        assert!(agent.adapter.calls().is_empty());
        assert_eq!(report.log[0].risk, Risk::Irreversible);
    }

    #[test]
    fn each_irreversible_step_asks_separately() {
        let delete = |label: &str| step(&["tidy"], "tidy", None, "irreversible", "press", json!({"label": label}));
        let llm = ScriptedLlm::new(vec![
            intent("irreversible", None),
            delete("Delete"),
            delete("Remove"),
            step(&[], "tidy", None, "safe", "done", json!({"message": "Tidied."})),
        ]);
        let agent = agent(FakeMac::new(&["Delete", "Remove"]), Some(llm), Arc::new(Heard::default()));
        let asked = Mutex::new(Vec::new());
        let report = run_answering(&agent, "tidy up", "", Confirmation::Yes, &asked);
        assert_eq!(report.outcome, Outcome::Done("Tidied.".into()));
        assert_eq!(asked.lock().unwrap().len(), 2);
        assert_eq!(agent.adapter.calls(), ["press Delete", "press Remove"]);
    }

    #[test]
    fn moving_a_file_by_script_is_confirmed_and_shell_scripts_are_refused() {
        let mv = r#"tell application "Finder" to move file "a.pdf" of folder "Downloads" to folder "Documents""#;
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&["move it"], "move it", None, "reversible", "script", json!({"script": mv})),
            step(&[], "move it", None, "safe", "done", json!({"message": "Moved."})),
        ]);
        let agent1 = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let asked = Mutex::new(Vec::new());
        run_answering(&agent1, "move my latest pdf", "", Confirmation::Yes, &asked);
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(agent1.adapter.calls().len(), 1);

        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["x"], "x", None, "safe", "script", json!({"script": "do shell script \"rm -rf ~\""})),
        ]);
        let agent2 = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let asked = Mutex::new(Vec::new());
        let report = run_answering(&agent2, "tidy up", "", Confirmation::Yes, &asked);
        assert!(matches!(report.outcome, Outcome::Refused(_)));
        assert!(asked.lock().unwrap().is_empty(), "a refused step must not even ask");
        assert!(agent2.adapter.calls().is_empty());
    }

    #[test]
    fn what_did_you_just_do_reads_the_log_and_is_not_itself_logged() {
        let heard = Arc::new(Heard::default());
        let plan = ["open Notes", "type"];
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&plan, "open Notes", None, "reversible", "focus_app", json!({"app": "Notes"})),
            step(&[], "type", None, "reversible", "type", json!({"text": "milk"})),
            step(&[], "type", None, "safe", "done", json!({"message": "Done."})),
        ]);
        let agent = agent(FakeMac::new(&[]), Some(llm), heard.clone());
        run(&agent, "open notes and write milk");
        let report = run(&agent, "What did you just do?");
        assert_eq!(report.outcome, Outcome::Done("I opened Notes and typed 4 characters.".into()));
        assert!(report.fast_path && report.llm_calls == 0);
        // Asking twice gives the same answer: reading the log does not change it.
        assert_eq!(run(&agent, "what did you do").outcome, report.outcome);
        assert_eq!(heard.dynamic.lock().unwrap().last().unwrap(), "I opened Notes and typed 4 characters.");
    }

    #[test]
    fn with_nothing_done_the_log_says_so() {
        let agent = agent(FakeMac::new(&[]), None, Arc::new(Heard::default()));
        assert_eq!(run(&agent, "what did you do").outcome, Outcome::Done("I haven't done anything yet.".into()));
    }

    #[test]
    fn conversation_is_answered_in_one_call_with_no_acknowledgement_and_no_action() {
        let mut chat = intent("safe", None);
        chat["goal"] = json!("chat");
        chat["app"] = Value::Null;
        chat["params"]["text"] = Value::Null;
        chat["reply"] = json!("Hi! I'm doing well. What shall we do?");
        let llm = ScriptedLlm::new(vec![chat]);
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&[]), Some(llm.clone()), heard.clone());
        let report = run(&agent, "Hey, how are you?");
        assert_eq!(report.outcome, Outcome::Reply("Hi! I'm doing well. What shall we do?".into()));
        assert_eq!(report.llm_calls, 1);
        assert!(agent.adapter.calls().is_empty() && report.log.is_empty());
        assert!(heard.fixed.lock().unwrap().is_empty(), "no \"On it\" before a chat answer");
        assert_eq!(heard.dynamic.lock().unwrap().as_slice(), ["Hi! I'm doing well. What shall we do?"]);
    }

    #[test]
    fn the_recent_conversation_reaches_the_model_for_follow_ups() {
        let llm = ScriptedLlm::new(vec![intent("reversible", None), step(&["a"], "a", None, "safe", "done", json!({"message": "Added."}))]);
        let agent = agent(FakeMac::new(&[]), Some(llm.clone()), Arc::new(Heard::default()));
        let context = "User: write my shopping list\nChirp: Your list is in Notes.";
        run_answering(&agent, "also add bread", context, Confirmation::No, &Mutex::new(Vec::new()));
        let asked = llm.asked.lock().unwrap();
        assert!(asked[0].contains("Chirp: Your list is in Notes.") && asked[0].contains("Latest request: also add bread"), "{}", asked[0]);
        assert!(asked[1].contains("Your list is in Notes."), "the planner sees it too");
    }

    #[test]
    fn acknowledgements_vary_and_follow_the_language_and_personality() {
        use schema::Persona;
        for persona in [Persona::Plain, Persona::Sassy] {
            let english: std::collections::HashSet<_> = (0..7).map(|n| acknowledgement_for(persona, "open notes and write hi", n)).collect();
            assert!(english.len() >= 4, "{persona:?}");
            assert!(acknowledgement_for(persona, "नोट्स खोलो और लिखो", 0).contains('।'));
            assert_ne!(acknowledgement_for(persona, "नोट्स खोलो और लिखो", 0), acknowledgement_for(persona, "नोट्स खोलो और लिखो", 1));
        }
        assert!(SASSY_ACKS.len() > PLAIN_ACKS.len());
    }

    #[test]
    fn the_same_line_is_not_said_twice_in_a_row() {
        let say = Some("Creating a new note for you.");
        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["a"], "a", say, "safe", "wait", json!({})),
            step(&[], "a", say, "safe", "wait", json!({})),
            step(&[], "a", None, "safe", "done", json!({"message": "Finished."})),
        ]);
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&[]), Some(llm), heard.clone());
        run(&agent, "do the thing please");
        let spoken = heard.dynamic.lock().unwrap();
        assert_eq!(spoken.iter().filter(|l| *l == "Creating a new note for you.").count(), 1, "{spoken:?}");
    }

    #[test]
    fn chat_detection_is_a_cheap_guess() {
        for chat in ["Hello!", "how are you", "What can you do?", "thanks a lot", "क्या तुम मेरी मदद कर सकते हो", "can you help me"] {
            assert!(looks_like_chat(chat), "{chat}");
        }
        for task in ["open notes and write hi", "move my latest pdf", "नोट्स खोलो और लिखो", "scroll down", "send the email to Rahul"] {
            assert!(!looks_like_chat(task), "{task}");
        }
    }

    #[test]
    fn a_bad_reply_gets_one_retry_that_explains_the_problem() {
        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["a"], "a", None, "safe", "press", json!({})), // press with no label
            step(&["a"], "a", None, "safe", "done", json!({"message": "All good."})),
        ]);
        let agent = agent(FakeMac::new(&[]), Some(llm.clone()), Arc::new(Heard::default()));
        let report = run(&agent, "do something unusual please");
        assert_eq!(report.outcome, Outcome::Done("All good.".into()));
        assert!(llm.asked.lock().unwrap()[2].contains("needs label"));

        let llm = ScriptedLlm::new(vec![intent("safe", None), json!({"nonsense": 1}), json!({"nonsense": 2})]);
        let agent = agent_for_failure(llm);
        assert!(matches!(run(&agent, "do something unusual please").outcome, Outcome::Failed(_)));
    }

    fn agent_for_failure(llm: Arc<ScriptedLlm>) -> Agent<FakeMac> {
        agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()))
    }

    #[test]
    fn repeating_a_failing_action_stops_the_task() {
        let press = || step(&["x"], "x", None, "reversible", "press", json!({"label": "Missing"}));
        let llm = ScriptedLlm::new(vec![intent("reversible", None), press(), press(), press(), press()]);
        let agent = agent(FakeMac::new(&["Other"]), Some(llm), Arc::new(Heard::default()));
        let report = run(&agent, "press the missing button");
        assert!(matches!(report.outcome, Outcome::Failed(_)));
        assert_eq!(report.log.len(), 3, "gave up after three identical failures");
        assert!(report.log.iter().all(|e| !e.ok));
    }

    #[test]
    fn doing_the_same_thing_over_and_over_stops_early_even_when_it_works() {
        // The real failure: Cmd+N, Cmd+W, Cmd+N, Cmd+W... each "succeeds".
        let key = |k: &str| step(&["x"], "x", None, "safe", "keys", json!({"keys": k}));
        let llm = ScriptedLlm::new(vec![intent("safe", None), key("cmd+n"), key("down"), key("cmd+n"), key("up"), key("cmd+n"), key("left")]);
        let agent = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let report = run(&agent, "keep making new documents");
        assert_eq!(report.outcome, Outcome::Failed("I seem to be going in circles, so I'll stop here.".into()));
        assert_eq!(report.log.len(), 5, "stopped on the third cmd+n, not at the step limit");
    }

    #[test]
    fn closing_a_window_asks_first() {
        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["close"], "close", None, "safe", "keys", json!({"keys": "cmd+w"})),
            step(&[], "close", None, "safe", "done", json!({"message": "Closed."})),
        ]);
        let agent = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let asked = Mutex::new(Vec::new());
        run_answering(&agent, "close this", "", Confirmation::No, &asked);
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert!(agent.adapter.calls().is_empty());
    }

    #[test]
    fn typing_is_read_back_and_the_model_is_told() {
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&["type"], "type", None, "reversible", "type", json!({"text": "milk, eggs, bread"})),
            step(&[], "type", None, "safe", "done", json!({"message": "Done."})),
        ]);
        let mac = FakeMac::new(&[]);
        *mac.field.lock().unwrap() = Some("milk, eggs, bread".into());
        let first = agent(mac, Some(llm.clone()), Arc::new(Heard::default()));
        run(&first, "write milk, eggs, bread");
        let asked = llm.asked.lock().unwrap();
        assert!(asked[2].contains("the text now shows in the field"), "{}", asked[2]);

        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&["type"], "type", None, "reversible", "type", json!({"text": "milk, eggs, bread"})),
            step(&[], "type", None, "safe", "done", json!({"message": "Done."})),
        ]);
        let mac = FakeMac::new(&[]);
        *mac.field.lock().unwrap() = Some("something else entirely".into());
        let second = agent(mac, Some(llm.clone()), Arc::new(Heard::default()));
        run(&second, "write milk, eggs, bread");
        assert!(llm.asked.lock().unwrap()[2].contains("does not show it yet"));
    }

    #[test]
    fn a_task_that_never_ends_is_cut_off() {
        let wait = || step(&["x"], "x", None, "safe", "wait", json!({"ms": 10}));
        let mut replies = vec![intent("safe", None)];
        replies.extend((0..20).map(|_| wait()));
        let agent = agent(FakeMac::new(&[]), Some(ScriptedLlm::new(replies)), Arc::new(Heard::default()));
        let report = run(&agent, "keep waiting forever");
        assert_eq!(report.outcome, Outcome::Failed("That took too many steps, so I stopped.".into()));
        assert_eq!(report.log.len(), 12);
    }

    #[test]
    fn pressing_the_talk_key_cancels_between_steps() {
        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["a", "b"], "a", None, "safe", "focus_app", json!({"app": "Notes"})),
            step(&[], "b", None, "safe", "focus_app", json!({"app": "Finder"})),
        ]);
        let agent = agent(FakeMac::new(&[]), Some(llm), Arc::new(Heard::default()));
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let cancelled = || calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 6;
        let report = agent.run("go and do two things", "", &Hooks { cancelled: &cancelled, on_state: &|_| {}, confirm: &|_| Confirmation::No });
        assert_eq!(report.outcome, Outcome::Cancelled);
        assert!(!agent.adapter.calls().contains(&"focus Finder".to_string()));
    }

    #[test]
    fn typing_goes_to_the_right_app_even_if_something_took_the_front() {
        let llm = ScriptedLlm::new(vec![
            intent("reversible", None),
            step(&["open", "type"], "open", None, "reversible", "focus_app", json!({"app": "Notes"})),
            step(&[], "type", None, "reversible", "type", json!({"text": "hello"})),
            step(&[], "type", None, "safe", "done", json!({"message": "Done."})),
        ]);
        let mac = FakeMac::new(&[]);
        let agent = agent(mac, Some(llm), Arc::new(Heard::default()));
        // Something steals the front right after Notes opens: simulate by changing it before typing.
        let hooks = Hooks {
            cancelled: &|| false,
            confirm: &|_| Confirmation::No,
            on_state: &|state| {
                if state == State::Thinking {
                    // Runs before every model turn; after the first it flips the front app.
                    let mut front = agent.adapter.front.lock().unwrap();
                    if *front == "Notes" {
                        *front = "WhatsApp".into();
                    }
                }
            },
        };
        let report = agent.run("open notes and type hello", "", &hooks);
        assert_eq!(report.outcome, Outcome::Done("Done.".into()));
        let calls = agent.adapter.calls();
        assert_eq!(calls.last().unwrap(), "type 5 chars into Notes", "{calls:?}");
    }

    #[test]
    fn at_most_three_step_narrations_are_spoken() {
        let llm = ScriptedLlm::new(vec![
            intent("safe", None),
            step(&["a"], "a", Some("One."), "safe", "wait", json!({})),
            step(&[], "a", Some("Two."), "safe", "wait", json!({})),
            step(&[], "a", Some("Three."), "safe", "keys", json!({"keys": "down"})),
            step(&[], "a", Some("Four."), "safe", "keys", json!({"keys": "up"})),
            step(&[], "a", None, "safe", "done", json!({"message": "Finished."})),
        ]);
        let heard = Arc::new(Heard::default());
        let agent = agent(FakeMac::new(&[]), Some(llm), heard.clone());
        run(&agent, "move around a bit please");
        let spoken = heard.dynamic.lock().unwrap();
        assert_eq!(spoken.as_slice(), ["One.", "Two.", "Three.", "Finished."]);
    }
}
