//! Typed ways to try the agent without speaking.
//!
//! `chirp --plan "request"` asks the model what it would do first and prints
//! it. It touches no app and speaks nothing. It sends no screen text unless
//! given `--screen`, and spends a few thousand model tokens.
//!
//! `chirp --run "request" [--speak]` runs the request for real, exactly as a
//! spoken one would, and prints every step and the timings.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use super::{
    agent::{Agent, Confirmation, Hooks, Limits, Outcome},
    narrator::{Narrate, Narrator},
    state::State,
    usage::Ledger,
};
use crate::{
    llm::{
        schema::{self},
        LlmClient, Router,
    },
    platform::{macos::adapter::MacAdapter, PlatformAdapter},
    safety::{
        confirm::{parse_answer, Answer},
        log::ActionLog,
    },
};

fn ledger() -> Option<Arc<Mutex<Ledger>>> {
    let home = PathBuf::from(std::env::var("HOME").ok()?);
    Some(Arc::new(Mutex::new(Ledger::load(&home.join("Library/Application Support/com.chirp.app"), 90.0))))
}

/// Prints what would be spoken instead of speaking it.
struct Printer;

impl Narrate for Printer {
    fn fixed(&self, line: &str) {
        println!("  says (fixed): {line}");
    }
    fn dynamic(&self, line: &str) {
        println!("  says: {line}");
    }
    fn cancel(&self) {}
}

pub fn plan_only(request: &str, with_screen: bool) -> i32 {
    let router = Router::from_env(ledger());
    if !router.has_provider() {
        eprintln!("No language-model key found. Set OPENAI_API_KEY, LLM_API_KEY or SARVAM_API_KEY.");
        return 2;
    }
    eprintln!("[chirp] models: {}", router.describe());
    let intent = match router.complete(&schema::intent_system(schema::Persona::from_env()), request, "intent", &schema::intent_schema()) {
        Ok(reply) => {
            eprintln!("[chirp] intent in {} ms from {} ({}+{} tokens)", reply.ms, reply.provider, reply.input_tokens, reply.output_tokens);
            match schema::parse_intent(&reply.value) {
                Ok(intent) => intent,
                Err(bad) => {
                    eprintln!("The reply did not fit the intent schema: {bad}\n{}", reply.value);
                    return 1;
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    println!("intent: {}", serde_json::to_string_pretty(&intent).unwrap_or_default());
    if intent.reply.is_some() || intent.needs_clarification {
        println!("(no steps: Chirp would just {})", if intent.reply.is_some() { "answer" } else { "ask the question" });
        return 0;
    }

    // The screen is left out unless asked for: its text would be sent to the model host.
    let adapter = MacAdapter::new();
    let (app, screen) = if with_screen {
        match (adapter.frontmost_app(), adapter.get_ui_tree(8)) {
            (Ok(app), Ok(tree)) => (app.name, tree.outline(2500)),
            _ => ("unknown".to_string(), "(the screen could not be read)".to_string()),
        }
    } else {
        ("Terminal".to_string(), "window \"Terminal\"\n".to_string())
    };
    let prompt = schema::step_prompt(request, &intent, &[], &[], &app, &screen);
    match router.complete(&schema::step_system(schema::Persona::from_env()), &prompt, "step", &schema::step_schema()) {
        Ok(reply) => {
            eprintln!("[chirp] first step in {} ms ({}+{} tokens)", reply.ms, reply.input_tokens, reply.output_tokens);
            println!("first step: {}", serde_json::to_string_pretty(&reply.value).unwrap_or_default());
            match schema::parse_step(&reply.value) {
                Ok(step) => {
                    println!("plan: {:?}\nnext action: {}", step.plan, step.action.describe());
                    0
                }
                Err(bad) => {
                    eprintln!("The reply did not fit the step schema: {bad}");
                    1
                }
            }
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

pub fn run_request(request: &str, speak: bool) -> i32 {
    if !crate::platform::macos::ax::is_trusted() {
        eprintln!("The program running this needs Accessibility permission (System Settings > Privacy & Security).");
        return 2;
    }
    let router = Router::from_env(ledger());
    let model: Option<Arc<dyn LlmClient>> = router.has_provider().then(|| Arc::new(router) as Arc<dyn LlmClient>);
    let narrator: Arc<dyn Narrate> = if speak { Narrator::start() } else { Arc::new(Printer) };
    let log = ActionLog::in_memory();
    let agent = Agent::new(MacAdapter::new(), model, narrator, log, Limits::default());
    println!("request: {request}");
    let report = agent.run(
        request,
        "",
        &Hooks {
            cancelled: &|| false,
            on_state: &|state: State| eprintln!("[chirp] state: {state:?}"),
            confirm: &|question| {
                // Typed, since there is no microphone here.
                println!("  asks: {question}");
                print!("  your answer (type yes or no): ");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                match parse_answer(&line) {
                    Answer::Yes => Confirmation::Yes,
                    _ => Confirmation::No,
                }
            },
        },
    );
    for entry in &report.log {
        println!(
            "  step {} [{}] {} ({:?}) -> {}{} ({} ms)",
            entry.step,
            entry.subtask,
            entry.action,
            entry.risk,
            if entry.ok { "ok: " } else { "FAILED: " },
            entry.result,
            entry.ms
        );
    }
    println!("plan: {:?}", report.plan);
    println!("outcome: {:?}", report.outcome);
    println!(
        "timings: intent {:?} ms, first action {:?} ms, first speech {:?} ms, total {} ms; {} model call(s), {}+{} tokens{}",
        report.timings.intent_ms,
        report.timings.first_action_ms,
        report.timings.first_speech_ms,
        report.timings.total_ms,
        report.llm_calls,
        report.input_tokens,
        report.output_tokens,
        if report.fast_path { " (fast path)" } else { "" }
    );
    if speak {
        // Let the queued narration finish before the process exits.
        std::thread::sleep(std::time::Duration::from_secs(6));
    }
    match report.outcome {
        Outcome::Done(_) | Outcome::Reply(_) => 0,
        _ => 1,
    }
}
