//! `chirp --ax selftest [textedit|finder|notes]`: the S4 acceptance test. Drives TextEdit, Finder and
//! Notes through the adapter (open, find by label, press, type, read back) and
//! reports each step as PASS, FAIL or SKIP, with timings.
//!
//! It acts on the real apps with fake data only, and cleans up after itself:
//! the TextEdit document is closed without saving, the Finder window is closed,
//! and the Notes note is deleted only after its text proves it is the test note.
//! It never types into a field that already holds text.

use std::{
    thread,
    time::{Duration, Instant},
};

use super::adapter::MacAdapter;
use crate::platform::{
    Action, AdapterError, ElementInfo, ElementQuery, Performed, PlatformAdapter, UiNode,
};

const MARKER: &str = "Chirp S4 test";
const SHOPPING: &str = "milk, eggs, bread";
const SHOPPING_HI: &str = "दूध, अंडे, ब्रेड";

enum Outcome {
    Pass(String),
    Fail(String),
    Skip(String),
}

struct Report {
    rows: Vec<(&'static str, String, Outcome, u128)>,
}

impl Report {
    fn step(&mut self, app: &'static str, name: &str, f: impl FnOnce() -> Outcome) -> bool {
        let started = Instant::now();
        let outcome = f();
        let ms = started.elapsed().as_millis();
        let (tag, detail) = match &outcome {
            Outcome::Pass(d) => ("PASS", d),
            Outcome::Fail(d) => ("FAIL", d),
            Outcome::Skip(d) => ("SKIP", d),
        };
        println!("{tag}  {app:<9} {name} ({ms} ms){}", if detail.is_empty() { String::new() } else { format!(": {detail}") });
        let ok = matches!(outcome, Outcome::Pass(_));
        self.rows.push((app, name.to_string(), outcome, ms));
        ok
    }
}

fn fail(error: impl ToString) -> Outcome {
    Outcome::Fail(error.to_string())
}

/// Returns the process exit code.
pub fn run(only: Option<&str>) -> i32 {
    if !super::ax::is_trusted() {
        eprintln!("The program running this (your terminal or editor) needs Accessibility permission.");
        eprintln!("System Settings > Privacy & Security > Accessibility.");
        return 2;
    }
    let mut report = Report { rows: Vec::new() };
    let wanted = |app: &str| only.is_none_or(|o| o.eq_ignore_ascii_case(app));
    if wanted("textedit") {
        textedit(&mut report);
    }
    if wanted("finder") {
        finder(&mut report);
    }
    if wanted("notes") {
        notes(&mut report);
    }

    let count = |f: fn(&Outcome) -> bool| report.rows.iter().filter(|r| f(&r.2)).count();
    let passed = count(|o| matches!(o, Outcome::Pass(_)));
    let failed = count(|o| matches!(o, Outcome::Fail(_)));
    let skipped = count(|o| matches!(o, Outcome::Skip(_)));
    println!("\n{passed} passed, {failed} failed, {skipped} skipped");
    i32::from(failed > 0)
}

fn poll<T>(timeout_ms: u64, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let started = Instant::now();
    loop {
        if let Some(found) = f() {
            return Some(found);
        }
        if started.elapsed() > Duration::from_millis(timeout_ms) {
            return None;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn in_front(adapter: &MacAdapter, name: &str) -> Result<(), String> {
    let app = adapter.frontmost_app().map_err(|e| e.to_string())?;
    if app.name.eq_ignore_ascii_case(name) {
        return Ok(());
    }
    // Something took the front (a chat app showing a message, say): ask once more.
    let refocused = adapter.focus_app(name).is_ok() && adapter.frontmost_app().is_ok_and(|a| a.name.eq_ignore_ascii_case(name));
    if refocused {
        Ok(())
    } else {
        Err(format!("{} is in front, not {name}; refusing to type or press keys", app.name))
    }
}

fn find_node<'a>(node: &'a UiNode, matches: &impl Fn(&ElementInfo) -> bool) -> Option<&'a ElementInfo> {
    if matches(&node.info) {
        return Some(&node.info);
    }
    node.children.iter().find_map(|child| find_node(child, matches))
}

/// The text area or text field that has keyboard focus, if any.
fn focused_text(adapter: &MacAdapter) -> Option<ElementInfo> {
    let tree = adapter.get_ui_tree(9).ok()?;
    find_node(&tree, &|i| i.focused && matches!(i.role.as_str(), "AXTextArea" | "AXTextField")).cloned()
}

fn window_titles(adapter: &MacAdapter) -> Vec<String> {
    adapter
        .get_ui_tree(1)
        .map(|tree| {
            tree.children
                .iter()
                .filter(|c| c.info.role == "AXWindow")
                .map(|c| c.info.label.clone().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default()
}

fn press(adapter: &MacAdapter, label: &str, role: &str) -> Result<Performed, String> {
    let query = ElementQuery { text: label.to_string(), role: Some(role.to_string()) };
    let hits = adapter.find_element(&query).map_err(|e| e.to_string())?;
    adapter.perform(&hits[0].0, Action::Press).map_err(|e| e.to_string())
}

fn checkbox_value(adapter: &MacAdapter, label: &str) -> Option<String> {
    let query = ElementQuery { text: label.to_string(), role: Some("checkbox".into()) };
    adapter.find_element(&query).ok()?.into_iter().next()?.1.value
}

fn textedit(report: &mut Report) {
    const APP: &str = "TextEdit";
    let adapter = MacAdapter::pinned_to(APP);
    if !report.step(APP, "open the app", || match adapter.focus_app(APP) {
        Ok(app) => Outcome::Pass(format!("pid {}", app.pid)),
        Err(e) => fail(e),
    }) {
        return;
    }
    let had_before = window_titles(&adapter).len();
    report.step(APP, "new document (cmd+n), empty text area appears", || {
        if let Err(e) = in_front(&adapter, APP) {
            return fail(e);
        }
        if let Err(e) = adapter.press_keys("cmd+n") {
            return fail(e);
        }
        match poll(4000, || focused_text(&adapter)) {
            Some(area) if area.value.is_none() => Outcome::Pass(format!("{} windows now", window_titles(&adapter).len())),
            Some(area) => fail(format!("text area was not empty: {:?}", area.value)),
            None => fail("no focused text area appeared"),
        }
    });
    let typed = report.step(APP, "type English text, read it back", || {
        if let Err(e) = in_front(&adapter, APP) {
            return fail(e);
        }
        if let Err(e) = adapter.type_text(&format!("Shopping list:\n{SHOPPING}")) {
            return fail(e);
        }
        match poll(3000, || focused_text(&adapter).filter(|a| a.value.as_deref().is_some_and(|v| v.contains(SHOPPING)))) {
            Some(a) => Outcome::Pass(format!("\"{}\"", a.value.unwrap_or_default())),
            None => fail(format!("text area holds {:?}", focused_text(&adapter).and_then(|a| a.value))),
        }
    });
    if typed {
        report.step(APP, "find the Bold checkbox by label and press it", || {
            let before = checkbox_value(&adapter, "bold");
            let how = match press(&adapter, "bold", "checkbox") {
                Ok(how) => how,
                Err(e) => return fail(e),
            };
            // The checkbox follows the document a moment later.
            let after = poll(2000, || checkbox_value(&adapter, "bold").filter(|v| Some(v) != before.as_ref()));
            if after.is_some() {
                // Put it back, so the document is as it was.
                let _ = press(&adapter, "bold", "checkbox");
            }
            match after {
                Some(after) => Outcome::Pass(format!("{before:?} -> {after:?} ({how:?})")),
                None => fail(format!("value stayed {before:?}")),
            }
        });
        report.step(APP, "type Hindi text, read it back", || {
            if let Err(e) = in_front(&adapter, APP) {
                return fail(e);
            }
            if let Err(e) = adapter.type_text(&format!("\n{SHOPPING_HI}")) {
                return fail(e);
            }
            match poll(3000, || focused_text(&adapter).filter(|a| a.value.as_deref().is_some_and(|v| v.contains(SHOPPING_HI)))) {
                Some(a) => Outcome::Pass(format!("\"{}\"", a.value.unwrap_or_default())),
                None => fail(format!("text area holds {:?}", focused_text(&adapter).and_then(|a| a.value))),
            }
        });
    }
    report.step(APP, "close the test document without saving (AppleScript)", || {
        match adapter.run_os_script(r#"tell application "TextEdit" to close front document saving no"#) {
            Ok(_) => {
                thread::sleep(Duration::from_millis(400));
                let now = window_titles(&adapter).len();
                if now <= had_before { Outcome::Pass(format!("{now} windows")) } else { fail(format!("{now} windows, was {had_before}")) }
            }
            Err(AdapterError::AutomationDenied(app)) => Outcome::Skip(format!("no Automation permission for {app}; close the document by hand")),
            Err(e) => fail(e),
        }
    });
}

fn finder(report: &mut Report) {
    const APP: &str = "Finder";
    let adapter = MacAdapter::pinned_to(APP);
    if !report.step(APP, "open the app", || match adapter.focus_app(APP) {
        Ok(app) => Outcome::Pass(format!("pid {}", app.pid)),
        Err(e) => fail(e),
    }) {
        return;
    }
    let before = window_titles(&adapter).len();
    let opened = report.step(APP, "new window (cmd+n)", || {
        if let Err(e) = in_front(&adapter, APP) {
            return fail(e);
        }
        if let Err(e) = adapter.press_keys("cmd+n") {
            return fail(e);
        }
        match poll(4000, || Some(window_titles(&adapter)).filter(|t| t.len() > before)) {
            Some(titles) => Outcome::Pass(format!("titles {titles:?}")),
            None => fail("no new window appeared"),
        }
    });
    if opened {
        for folder in ["Documents", "Downloads"] {
            report.step(APP, &format!("find \"{folder}\" in the sidebar and press it"), || {
                let how = match press(&adapter, folder, "cell") {
                    Ok(how) => how,
                    Err(e) => return fail(e),
                };
                match poll(3000, || window_titles(&adapter).into_iter().find(|t| t == folder)) {
                    Some(_) => Outcome::Pass(format!("window now titled {folder} ({how:?})")),
                    None => fail(format!("window titles are {:?}", window_titles(&adapter))),
                }
            });
        }
        report.step(APP, "close the window we opened (cmd+w)", || {
            if let Err(e) = in_front(&adapter, APP).and_then(|()| adapter.press_keys("cmd+w").map_err(|e| e.to_string())) {
                return fail(e);
            }
            match poll(3000, || Some(window_titles(&adapter).len()).filter(|n| *n <= before)) {
                Some(n) => Outcome::Pass(format!("{n} windows")),
                None => fail(format!("{} windows, was {before}", window_titles(&adapter).len())),
            }
        });
    }
    report.step(APP, "OS scripting: ask Finder for the startup disk name", || {
        match adapter.run_os_script(r#"tell application "Finder" to get name of startup disk"#) {
            Ok(name) if !name.is_empty() => Outcome::Pass(name),
            Ok(_) => fail("empty answer"),
            Err(AdapterError::AutomationDenied(app)) => Outcome::Skip(format!("macOS has not allowed controlling {app} yet")),
            Err(e) => fail(e),
        }
    });
}

fn notes(report: &mut Report) {
    const APP: &str = "Notes";
    let adapter = MacAdapter::pinned_to(APP);
    if !report.step(APP, "open the app", || match adapter.focus_app(APP) {
        Ok(app) => Outcome::Pass(format!("pid {}", app.pid)),
        Err(e) => fail(e),
    }) {
        return;
    }
    let created = report.step(APP, "find \"New Note\" by label and press it", || {
        if let Err(e) = in_front(&adapter, APP) {
            return fail(e);
        }
        let how = match press(&adapter, "new note", "button") {
            Ok(how) => how,
            Err(e) => return fail(e),
        };
        // The new note must be empty and focused; otherwise typing would land in a real note.
        match poll(4000, || focused_text(&adapter).filter(|a| a.value.is_none())) {
            Some(_) => Outcome::Pass(format!("empty note focused ({how:?})")),
            None => fail("no empty, focused note appeared; nothing was typed"),
        }
    });
    if !created {
        return;
    }
    let text = format!("{MARKER}: {SHOPPING}");
    let typed = report.step(APP, "type the shopping list, read it back", || {
        if let Err(e) = in_front(&adapter, APP) {
            return fail(e);
        }
        if let Err(e) = adapter.type_text(&text) {
            return fail(e);
        }
        match poll(3000, || focused_text(&adapter).filter(|a| a.value.as_deref().is_some_and(|v| v.contains(SHOPPING)))) {
            Some(a) => Outcome::Pass(format!("\"{}\"", a.value.unwrap_or_default())),
            None => fail(format!("note holds {:?}", focused_text(&adapter).and_then(|a| a.value))),
        }
    });
    report.step(APP, "delete the test note (AppleScript, only notes starting with the marker)", || {
        let ours = focused_text(&adapter).and_then(|a| a.value).is_some_and(|v| v.starts_with(MARKER));
        if !typed && !ours {
            return Outcome::Skip("nothing to delete".into());
        }
        // Notes has no menu command for this while the editor has focus, so use
        // scripting. The match is on the marker, so no real note can be touched.
        let script = format!(r#"tell application "Notes" to delete (every note whose name starts with "{MARKER}")"#);
        match adapter.run_os_script(&script) {
            Ok(_) => {
                // The editor can keep showing the old text, so ask Notes where the note is.
                let check = format!(
                    r#"tell application "Notes" to get count of (notes in folder "Recently Deleted" whose name starts with "{MARKER}")"#
                );
                match adapter.run_os_script(&check).map(|n| n.parse::<u32>().unwrap_or(0)) {
                    Ok(n) if n > 0 => Outcome::Pass(format!("{n} in Recently Deleted (Notes purges it after 30 days)")),
                    Ok(_) => fail("the test note is not in Recently Deleted"),
                    Err(e) => fail(e),
                }
            }
            Err(AdapterError::AutomationDenied(app)) => {
                Outcome::Skip(format!("no Automation permission for {app}; delete the \"{MARKER}\" note by hand"))
            }
            Err(e) => fail(e),
        }
    });
}
