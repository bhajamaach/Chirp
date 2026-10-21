//! What the language model is asked for and how its answers are checked.
//!
//! Two calls: `Intent` (the brief's schema) once per request, then `Step`
//! (one action plus the running list of subtasks) once per loop turn. Both are
//! sent as strict JSON schemas, and every reply is validated here again,
//! because a schema constrains the shape, not the meaning.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Reversible,
    Irreversible,
}

/// Free-form `params` do not fit a strict schema, so the useful ones are named.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IntentParams {
    /// Words to write, exactly as they should appear, in the user's language.
    pub text: Option<String>,
    pub target: Option<String>,
    pub location: Option<String>,
    pub query: Option<String>,
}

/// The brief's intent schema.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Intent {
    pub goal: String,
    pub app: Option<String>,
    pub params: IntentParams,
    pub risk: Risk,
    pub needs_clarification: bool,
    pub question: Option<String>,
    /// For conversation (a greeting, thanks, a question that needs no action on
    /// the computer): the answer, ready to speak. `None` for a task. This is
    /// the one field added to the brief's intent schema.
    #[serde(default)]
    pub reply: Option<String>,
}

/// One thing the agent can do to the computer, or how a task ends.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    FocusApp { app: String },
    Press { label: String, role: Option<String> },
    Type { text: String },
    Keys { keys: String },
    Script { script: String },
    Wait { ms: u64 },
    Done { message: String },
    Ask { question: String },
    Fail { message: String },
}

impl Action {
    /// A description for the log and the history. Typed text and scripts are
    /// summarised, never copied: the log stores actions, not content.
    pub fn describe(&self) -> String {
        match self {
            Action::FocusApp { app } => format!("open {app}"),
            Action::Press { label, role } => match role {
                Some(role) => format!("press {role} \"{label}\""),
                None => format!("press \"{label}\""),
            },
            Action::Type { text } => format!("type {} characters", text.chars().count()),
            Action::Keys { keys } => format!("press keys {keys}"),
            Action::Script { script } => format!("run a script of {} characters", script.chars().count()),
            Action::Wait { ms } => format!("wait {ms} ms"),
            Action::Done { .. } => "finish".into(),
            Action::Ask { .. } => "ask a question".into(),
            Action::Fail { .. } => "give up".into(),
        }
    }

    /// The same, in the past tense, for reading the log aloud.
    pub fn past(&self) -> String {
        match self {
            Action::FocusApp { app } => format!("opened {app}"),
            Action::Press { label, .. } => format!("pressed \"{label}\""),
            Action::Type { text } => format!("typed {} characters", text.chars().count()),
            Action::Keys { keys } => format!("pressed {keys}"),
            Action::Script { .. } => "ran a script".into(),
            Action::Wait { .. } => "waited a moment".into(),
            Action::Done { .. } => "finished".into(),
            Action::Ask { .. } => "asked a question".into(),
            Action::Fail { .. } => "gave up".into(),
        }
    }

    /// Ends the task rather than touching the computer.
    pub fn is_terminal(&self) -> bool {
        matches!(self, Action::Done { .. } | Action::Ask { .. } | Action::Fail { .. })
    }
}

/// The action exactly as the model writes it: one flat object, because strict
/// schemas handle a flat object far more reliably than a union.
#[derive(Clone, Debug, Deserialize)]
struct RawAction {
    kind: String,
    app: Option<String>,
    label: Option<String>,
    role: Option<String>,
    text: Option<String>,
    keys: Option<String>,
    script: Option<String>,
    ms: Option<u64>,
    message: Option<String>,
    question: Option<String>,
}

#[derive(Debug, PartialEq)]
pub struct BadStep(pub String);

impl fmt::Display for BadStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn need(value: Option<String>, kind: &str, field: &str) -> Result<String, BadStep> {
    value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| BadStep(format!("action {kind} needs {field}")))
}

impl RawAction {
    fn into_action(self) -> Result<Action, BadStep> {
        let kind = self.kind.to_ascii_lowercase();
        Ok(match kind.as_str() {
            "focus_app" => Action::FocusApp { app: need(self.app, &kind, "app")? },
            "press" => Action::Press {
                label: need(self.label, &kind, "label")?,
                role: self.role.map(|r| r.trim().to_string()).filter(|r| !r.is_empty()),
            },
            // Text is typed as given, so spaces and newlines are kept.
            "type" => Action::Type { text: self.text.filter(|t| !t.is_empty()).ok_or_else(|| BadStep("action type needs text".into()))? },
            "keys" => Action::Keys { keys: need(self.keys, &kind, "keys")? },
            "script" => Action::Script { script: need(self.script, &kind, "script")? },
            "wait" => Action::Wait { ms: self.ms.unwrap_or(500).min(3000) },
            "done" => Action::Done { message: need(self.message, &kind, "message")? },
            "ask" => Action::Ask { question: need(self.question.or(self.message), &kind, "question")? },
            "fail" => Action::Fail { message: need(self.message, &kind, "message")? },
            other => return Err(BadStep(format!("unknown action kind \"{other}\""))),
        })
    }
}

/// One turn of the planner.
#[derive(Clone, Debug)]
pub struct Step {
    /// The subtask list. Filled on the first turn and whenever the plan changes; empty otherwise.
    pub plan: Vec<String>,
    /// The subtask this action belongs to.
    pub subtask: String,
    /// A short line to speak, or `None`.
    pub say: Option<String>,
    pub risk: Risk,
    /// For a risky step: one sentence saying what it is for, in the user's language.
    pub confirm: Option<String>,
    pub action: Action,
}

#[derive(Deserialize)]
struct RawStep {
    plan: Vec<String>,
    subtask: String,
    say: Option<String>,
    risk: Risk,
    #[serde(default)]
    confirm: Option<String>,
    action: RawAction,
}

pub fn parse_intent(value: &Value) -> Result<Intent, BadStep> {
    let mut intent: Intent = serde_json::from_value(value.clone()).map_err(|e| BadStep(format!("intent: {e}")))?;
    intent.goal = intent.goal.trim().to_string();
    if intent.goal.is_empty() {
        return Err(BadStep("intent: goal is empty".into()));
    }
    intent.reply = intent.reply.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if intent.needs_clarification && intent.question.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return Err(BadStep("intent: needs_clarification is true but there is no question".into()));
    }
    Ok(intent)
}

pub fn parse_step(value: &Value) -> Result<Step, BadStep> {
    let raw: RawStep = serde_json::from_value(value.clone()).map_err(|e| BadStep(format!("step: {e}")))?;
    let action = raw.action.into_action()?;
    Ok(Step {
        plan: raw.plan.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).take(8).collect(),
        subtask: raw.subtask.trim().to_string(),
        say: raw.say.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        risk: raw.risk,
        confirm: raw.confirm.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()),
        action,
    })
}

fn nullable_string() -> Value {
    json!({"type": ["string", "null"]})
}

pub fn intent_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["goal", "app", "params", "risk", "needs_clarification", "question", "reply"],
        "properties": {
            "goal": {"type": "string"},
            "app": nullable_string(),
            "params": {
                "type": "object",
                "additionalProperties": false,
                "required": ["text", "target", "location", "query"],
                "properties": {
                    "text": nullable_string(),
                    "target": nullable_string(),
                    "location": nullable_string(),
                    "query": nullable_string()
                }
            },
            "risk": {"type": "string", "enum": ["safe", "reversible", "irreversible"]},
            "needs_clarification": {"type": "boolean"},
            "question": nullable_string(),
            "reply": nullable_string()
        }
    })
}

pub fn step_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["plan", "subtask", "say", "risk", "confirm", "action"],
        "properties": {
            "plan": {"type": "array", "items": {"type": "string"}},
            "subtask": {"type": "string"},
            "say": nullable_string(),
            "risk": {"type": "string", "enum": ["safe", "reversible", "irreversible"]},
            "confirm": nullable_string(),
            "action": {
                "type": "object",
                "additionalProperties": false,
                "required": ["kind", "app", "label", "role", "text", "keys", "script", "ms", "message", "question"],
                "properties": {
                    "kind": {"type": "string", "enum": ["focus_app", "press", "type", "keys", "script", "wait", "done", "ask", "fail"]},
                    "app": nullable_string(),
                    "label": nullable_string(),
                    "role": nullable_string(),
                    "text": nullable_string(),
                    "keys": nullable_string(),
                    "script": nullable_string(),
                    "ms": {"type": ["integer", "null"]},
                    "message": nullable_string(),
                    "question": nullable_string()
                }
            }
        }
    })
}

const INTENT_BASE: &str = "\
You are the brain of Chirp, a warm, friendly voice assistant that controls a Mac for people who are blind or low-vision. \
They speak English, Hindi, or a mix, and you answer in the language they used. \
You may be given the recent conversation, so follow-ups like \"do that again\", \"also add bread\" or \"what about the other one\" make sense.
Read the latest request into the fields:
- goal: one short English sentence saying what the user wants. For plain conversation write \"chat\".
- app: the macOS app needed, by its English name (Notes, Finder, TextEdit, Safari), or null.
- params.text: words the user wants written, exactly as they should appear, in the user's own language and script. params.target: the thing acted on. params.location: a folder or place. params.query: a search. Use null for any that do not apply.
- risk: safe = only reads or looks. reversible = changes something that can be undone (open, type, create). irreversible = sends, pays, deletes, moves files, submits or overwrites.
- needs_clarification: true only if the request cannot be done without one missing fact. Then question is one short, natural question in the user's language, and reply is null.
- reply: for conversation that needs no action on the computer (a greeting, thanks, how are you, what can you do, a general question, what you just did or said), write the answer as you would say it: warm and natural, 25 words or fewer, in the user's language. Then goal is \"chat\", app is null, risk is safe. For anything that needs the computer, reply is null, always: if the user asks you to open, type, write, press, move, find, read or look at anything on the Mac, it is a task, so reply is null, and you never describe or announce the action in reply. Chirp can open and use apps, type, press buttons, move files, and describe what is on screen. It always asks before sending, deleting or moving anything.
Reply with the JSON object only.";

const STEP_BASE: &str = "\
You control a Mac for a blind or low-vision person, one step at a time, as Chirp: a warm, friendly voice assistant. \
Each turn you see their request, the recent conversation, your plan, what you have done so far, and an outline of the screen of the app in front. Choose the single next action.

Breaking the work down: on your first turn write plan as 2 to 6 short subtasks (for example: open Notes, \
create a new note, type the list, check it), in plain words and never as action names. On later turns plan is an empty list unless the plan must change, \
and then you write the whole new plan. subtask names the one you are working on now.

Actions (put unused fields as null):
- focus_app {app}: open or switch to an app by its English name. Always do this before typing or pressing keys in it.
- press {label, role}: press a visible control by its label as written in the outline. role is optional (button, menuitem, checkbox, cell). \
Prefer pressing a menu item by label over using a keyboard shortcut.
- type {text}: type into the field that has focus. Write the text exactly as the user would want it, in their language. It can type any character, including emoji (type the emoji itself, such as a pink heart, and never open the emoji picker). This is the only way to write text.
- keys {keys}: ONE shortcut or key, such as cmd+n, return, escape, down. Never text, words or commands: to write anything use type.
- script {script}: AppleScript. Use it for moving, copying or renaming files (one step is faster than clicking through Finder) and for things the screen cannot do. Keep it short. If a script fails because control of an app is not allowed, do the job through the screen instead.
- wait {ms}: wait up to 3000 ms for a window to appear.
- done {message}: the whole request is finished. message is the result or answer in the user's language, natural and warm, 20 words or fewer. \
To describe the screen, summarise the outline in plain words in this message.
- ask {question}: you need one fact only the user can give. One short, natural question.
- fail {message}: it cannot be done. Say why kindly, 20 words or fewer, and offer what to try instead if there is something.

Rules:
- One action per turn. Look at the screen outline after every action and check that the last one worked before you call it done.
- If an action failed, try a different way. Never repeat the same failing action.
- Do only what was asked. Do not add subtasks for saving, closing, checking again or tidying up. Notes, TextEdit, Reminders and most Mac apps save by themselves, so never press cmd+s, and never close a window or quit an app unless the user asked.
- Stop as soon as the request is met. If the screen already shows what was asked for (the text you typed is in the note, the app is open, the file has moved), your next action is done. Do not keep going to be safe.
- The result of an action tells you what happened, for example whether typed text now shows in the field. Believe it.
- Saying in `say` that something is done does not finish the task. When the work is complete, the action itself must be done.
- Never type into a password field, never open System Settings, and never use Terminal or run shell commands. If the request needs the terminal, call fail and say you can't.
- say is what you speak aloud while you work: 12 words or fewer, natural, in the user's language, or null. Vary your words; do not repeat the same phrase. Never joke about errors.
- risk is the risk of this one step. Mark irreversible for anything that sends, pays, deletes, moves files, submits or overwrites. \
Chirp will read it back and wait for the user's spoken yes before it runs, so do not use ask for that. For such a step, confirm is one sentence saying what it is for, in the user's language (for example: your reply to Rahul is ready). Otherwise confirm is null.
Reply with the JSON object only.";

/// How Chirp sounds. The default is sassy; `CHIRP_PERSONALITY=warm` or `plain` turns it down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Persona {
    Sassy,
    Warm,
    Plain,
}

impl Persona {
    pub fn from_env() -> Persona {
        match std::env::var("CHIRP_PERSONALITY").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
            "plain" | "neutral" => Persona::Plain,
            "warm" | "friendly" => Persona::Warm,
            _ => Persona::Sassy,
        }
    }

    fn block(self) -> &'static str {
        match self {
            Persona::Sassy => "\
Personality: you are Chirp, a quick-witted little sidekick with sass and a big heart. Talk like a funny, confident friend, \
not a customer-service bot: playful teasing, dry one-liners, a bit of swagger, short punchy lines. \
Never say \"How can I assist you\", \"I'm here to help\", \"certainly\", \"I apologize\", \"as an AI\" or \"is there anything else\". \
Tease the situation or the task, never the user: nothing about their abilities, accent, mistakes or disability. \
A quip is a few words, not a paragraph. In Hindi or mixed speech, banter in natural casual Hinglish, not textbook Hindi. \
Drop the sass completely, and be plain, calm and exact, when you ask for a yes or no, refuse something, report a failure, or the user sounds stressed or upset. \
Everything you write is spoken aloud, so use no emoji, no asterisks and no markdown in any field except text the user wants typed. \
For example, asked how you are: \"Fabulous. What are we breaking today?\"; thanked: \"Flattery will get you everywhere.\"; asked to do something silly, play along, within reason.",
            Persona::Warm => "\
Personality: you are Chirp, a warm, upbeat friend. Be natural and kind, never stiff or corporate. \
Never say \"How can I assist you\", \"certainly\" or \"as an AI\". Keep lines short. \
Everything you write is spoken aloud, so use no emoji, no asterisks and no markdown in any field except text the user wants typed.",
            Persona::Plain => "\
Personality: calm, clear and brief. No jokes. \
Everything you write is spoken aloud, so use no emoji, no asterisks and no markdown in any field except text the user wants typed.",
        }
    }
}

/// Extra guidance for the lines spoken while working. It is kept out of the intent
/// prompt on purpose: shown narration examples there, the model answered tasks
/// with a quip in `reply` instead of doing them.
fn narration(persona: Persona) -> &'static str {
    match persona {
        Persona::Sassy => r#"The `say` lines and the final `done` message carry the personality too. Never write flat narration like "Opening Notes for you." or "Opening Finder to start the move."; give it attitude in a few words, and never say the same line twice. For example: opening an app, "Notes, coming right up." or "Cracking open Finder."; making a note, "Fresh page, big plans."; typing, "Typing. Fast, I know."; moving a file, "Operation file shuffle, go."; finishing, "Done. You're welcome." or "Boom. Your list is in.""#,
        Persona::Warm | Persona::Plain => "",
    }
}

pub fn intent_system(persona: Persona) -> String {
    format!("{INTENT_BASE}\n\n{}", persona.block())
}

pub fn step_system(persona: Persona) -> String {
    format!("{STEP_BASE}\n\n{}\n{}", persona.block(), narration(persona))
}

/// The recent conversation as a block for the prompts, or nothing.
pub fn with_context(context: &str, request: &str) -> String {
    if context.trim().is_empty() {
        request.to_string()
    } else {
        format!("Recent conversation:\n{}\n\nLatest request: {request}", context.trim())
    }
}

/// Builds the user message for one planner turn.
pub fn step_prompt(request: &str, intent: &Intent, plan: &[String], history: &[String], app: &str, screen: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("Request: {request}\nGoal: {}\n", intent.goal));
    if let Some(text) = &intent.params.text {
        out.push_str(&format!("Text to write: {text}\n"));
    }
    if plan.is_empty() {
        out.push_str("Plan: none yet. Write one now.\n");
    } else {
        out.push_str("Plan:\n");
        for (i, item) in plan.iter().enumerate() {
            out.push_str(&format!("  {}. {item}\n", i + 1));
        }
    }
    if history.is_empty() {
        out.push_str("Done so far: nothing.\n");
    } else {
        out.push_str("Done so far:\n");
        for line in history {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push_str(&format!("App in front: {app}\nScreen:\n{screen}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(kind: &str) -> Value {
        json!({"kind": kind, "app": null, "label": null, "role": null, "text": null, "keys": null,
               "script": null, "ms": null, "message": null, "question": null})
    }

    fn step_with(mut action: Value, patch: Value) -> Value {
        for (k, v) in patch.as_object().unwrap() {
            action[k] = v.clone();
        }
        json!({"plan": ["open Notes", "write the list"], "subtask": "open Notes", "say": "Opening Notes.",
               "risk": "reversible", "action": action})
    }

    #[test]
    fn parses_a_focus_step() {
        let step = parse_step(&step_with(action("focus_app"), json!({"app": "Notes"}))).unwrap();
        assert_eq!(step.action, Action::FocusApp { app: "Notes".into() });
        assert_eq!(step.plan.len(), 2);
        assert_eq!(step.say.as_deref(), Some("Opening Notes."));
    }

    #[test]
    fn typed_text_keeps_its_spaces_and_newlines() {
        let step = parse_step(&step_with(action("type"), json!({"text": "milk\nदूध "}))).unwrap();
        assert_eq!(step.action, Action::Type { text: "milk\nदूध ".into() });
    }

    #[test]
    fn missing_fields_are_reported_for_the_retry() {
        let error = parse_step(&step_with(action("press"), json!({}))).unwrap_err();
        assert!(error.0.contains("needs label"), "{error}");
        let error = parse_step(&step_with(action("teleport"), json!({}))).unwrap_err();
        assert!(error.0.contains("unknown action kind"), "{error}");
    }

    #[test]
    fn wait_is_capped() {
        let step = parse_step(&step_with(action("wait"), json!({"ms": 60000}))).unwrap();
        assert_eq!(step.action, Action::Wait { ms: 3000 });
    }

    #[test]
    fn the_log_description_never_contains_typed_text() {
        let text = "my secret list".to_string();
        let described = Action::Type { text: text.clone() }.describe();
        assert!(!described.contains("secret"), "{described}");
        let described = Action::Script { script: "tell application \"Finder\" to delete x".into() }.describe();
        assert!(!described.contains("Finder"), "{described}");
    }

    #[test]
    fn intent_follows_the_brief_and_checks_the_question() {
        let good = json!({"goal": "Open Notes and write a list", "app": "Notes",
            "params": {"text": "milk, eggs, bread", "target": null, "location": null, "query": null},
            "risk": "reversible", "needs_clarification": false, "question": null});
        let intent = parse_intent(&good).unwrap();
        assert_eq!(intent.risk, Risk::Reversible);
        assert_eq!(intent.params.text.as_deref(), Some("milk, eggs, bread"));

        let mut bad = good.clone();
        bad["needs_clarification"] = json!(true);
        assert!(parse_intent(&bad).is_err());
        bad["risk"] = json!("catastrophic");
        assert!(parse_intent(&bad).is_err());
    }

    #[test]
    fn strict_schemas_require_every_property() {
        for schema in [intent_schema(), step_schema(), step_schema()["properties"]["action"].clone()] {
            let props: Vec<_> = schema["properties"].as_object().unwrap().keys().cloned().collect();
            let required: Vec<_> = schema["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
            for p in &props {
                assert!(required.contains(p), "{p} is not required");
            }
            assert_eq!(schema["additionalProperties"], json!(false));
        }
    }

    #[test]
    fn each_persona_asks_for_speakable_text_and_sassy_is_the_default() {
        for persona in [Persona::Sassy, Persona::Warm, Persona::Plain] {
            let text = intent_system(persona);
            assert!(text.contains("no emoji") && text.contains("Reply with the JSON object only."), "{persona:?}");
            assert!(step_system(persona).contains("no emoji"));
        }
        assert!(intent_system(Persona::Sassy).contains("sass"));
        assert!(!intent_system(Persona::Plain).contains("sass"));
        // Sass stops where the brief says it must: confirmations, errors and refusals.
        assert!(intent_system(Persona::Sassy).contains("ask for a yes or no, refuse something, report a failure"));
    }

    #[test]
    fn narration_examples_stay_out_of_the_intent_prompt() {
        // Regression: with these in the intent prompt, "open Notes and write hello" was
        // answered with a quip in `reply` and never carried out.
        for persona in [Persona::Sassy, Persona::Warm, Persona::Plain] {
            let intent = intent_system(persona);
            assert!(!intent.contains("coming right up") && !intent.contains("Fresh page"), "{persona:?}");
            assert!(intent.contains("reply is null, always"));
        }
        assert!(step_system(Persona::Sassy).contains("coming right up"));
    }

    #[test]
    fn prompt_lists_plan_and_history() {
        let intent = parse_intent(&json!({"goal": "g", "app": null,
            "params": {"text": "milk", "target": null, "location": null, "query": null},
            "risk": "safe", "needs_clarification": false, "question": null})).unwrap();
        let prompt = step_prompt("do it", &intent, &["a".into(), "b".into()], &["1. did a".into()], "Notes", "window");
        assert!(prompt.contains("1. a") && prompt.contains("2. b") && prompt.contains("1. did a"));
        assert!(prompt.contains("Text to write: milk") && prompt.ends_with("window"));
    }
}
