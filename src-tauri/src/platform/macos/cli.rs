//! `chirp --ax <command>`: drives the macOS adapter from typed commands, so the
//! accessibility layer can be tested in real apps before any voice is involved.
//!
//! Commands (add `--app <name>` to act on a named app instead of the one in
//! front, which from a terminal is the terminal itself):
//!
//! ```text
//! chirp --ax frontmost
//! chirp --ax tree [depth] --app Notes
//! chirp --ax outline [depth] --app Notes   (the short text summary for the planner)
//! chirp --ax find "new note" [--role button] --app Notes
//! chirp --ax press "new note" [--role button] --app Notes
//! chirp --ax type "milk, eggs, bread"
//! chirp --ax keys cmd+n
//! chirp --ax focus Notes
//! chirp --ax selftest        (the S4 acceptance test: TextEdit, Finder, Notes)
//! chirp --ax script 'tell application "Finder" to get name of startup disk'
//! ```

use std::time::Instant;

use super::adapter::MacAdapter;
use crate::platform::{Action, ElementInfo, ElementQuery, PlatformAdapter, UiNode};

const USAGE: &str = "Usage: chirp --ax <frontmost|tree [depth]|outline [depth]|find <label>|press <label>|type <text>|keys <combo>|focus <app>|script <applescript>|selftest> [--app <name>] [--role <role>] [--action <action>]";

/// Returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    let mut words: Vec<String> = Vec::new();
    let mut app: Option<String> = None;
    let mut role: Option<String> = None;
    let mut action = Action::Press;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--app" => app = iter.next().cloned(),
            "--role" => role = iter.next().cloned(),
            "--action" => match iter.next().and_then(|a| Action::parse(a)) {
                Some(parsed) => action = parsed,
                None => {
                    eprintln!("--action takes press, focus, raise, menu, increment, decrement or click");
                    return 2;
                }
            },
            _ => words.push(arg.clone()),
        }
    }
    let adapter = match &app {
        Some(name) => MacAdapter::pinned_to(name),
        None => MacAdapter::new(),
    };
    let Some(command) = words.first().map(String::as_str) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if command == "selftest" {
        return super::selftest::run(words.get(1).map(String::as_str));
    }
    let rest = words[1..].join(" ");
    let started = Instant::now();
    let outcome = dispatch(&adapter, command, &rest, role, action);
    eprintln!("[chirp] {command} took {} ms", started.elapsed().as_millis());
    match outcome {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("error: {message}");
            1
        }
    }
}

fn dispatch(adapter: &MacAdapter, command: &str, rest: &str, role: Option<String>, action: Action) -> Result<(), String> {
    match command {
        "frontmost" => {
            let app = adapter.frontmost_app().map_err(|e| e.to_string())?;
            println!("{} (pid {}, {})", app.name, app.pid, app.bundle_id.as_deref().unwrap_or("no bundle id"));
        }
        "tree" => {
            let depth = rest.trim().parse().unwrap_or(6);
            let tree = adapter.get_ui_tree(depth).map_err(|e| e.to_string())?;
            print_tree(&tree, 0);
            eprintln!("[chirp] {} elements to depth {depth}", tree.count());
        }
        "outline" => {
            let tree = adapter.get_ui_tree(rest.trim().parse().unwrap_or(8)).map_err(|e| e.to_string())?;
            let text = tree.outline(4000);
            println!("{text}");
            eprintln!("[chirp] outline is {} characters", text.chars().count());
        }
        "find" | "press" => {
            if rest.trim().is_empty() {
                return Err("give the label to look for".into());
            }
            let query = ElementQuery { text: rest.to_string(), role };
            let hits = adapter.find_element(&query).map_err(|e| e.to_string())?;
            if command == "find" {
                for (_, info) in hits.iter().take(10) {
                    println!("{}", line(info));
                }
                if hits.len() > 10 {
                    println!("... and {} more", hits.len() - 10);
                }
            } else {
                let (element, info) = &hits[0];
                println!("{action:?} {}", line(info));
                let how = adapter.perform(element, action).map_err(|e| e.to_string())?;
                println!("done: {how:?}");
            }
        }
        "type" => adapter.type_text(rest).map_err(|e| e.to_string())?,
        "keys" => adapter.press_keys(rest).map_err(|e| e.to_string())?,
        "focus" => {
            let app = adapter.focus_app(rest).map_err(|e| e.to_string())?;
            println!("{} is in front (pid {})", app.name, app.pid);
        }
        "script" => println!("{}", adapter.run_os_script(rest).map_err(|e| e.to_string())?),
        _ => return Err(USAGE.to_string()),
    }
    Ok(())
}

fn line(info: &ElementInfo) -> String {
    let mut text = info.role.trim_start_matches("AX").to_lowercase();
    if let Some(subrole) = &info.subrole {
        text.push_str(&format!("/{}", subrole.trim_start_matches("AX").to_lowercase()));
    }
    if let Some(label) = &info.label {
        text.push_str(&format!(" \"{label}\""));
    }
    if let Some(value) = &info.value {
        if info.label.as_ref() != Some(value) {
            text.push_str(&format!(" = \"{value}\""));
        }
    }
    if !info.enabled {
        text.push_str(" (disabled)");
    }
    if info.focused {
        text.push_str(" (focused)");
    }
    if let Some(f) = info.frame {
        text.push_str(&format!(" @{:.0},{:.0} {:.0}x{:.0}", f.x, f.y, f.w, f.h));
    }
    text
}

fn print_tree(node: &UiNode, depth: usize) {
    println!("{}{}{}", "  ".repeat(depth), line(&node.info), if node.truncated { "  …" } else { "" });
    for child in &node.children {
        print_tree(child, depth + 1);
    }
}
