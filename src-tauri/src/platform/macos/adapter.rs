//! The macOS implementation of `PlatformAdapter`: layer 0 (AppleScript, `open`)
//! and layer 1 (the accessibility tree plus synthetic input).

use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use objc2_app_kit::{NSRunningApplication, NSWorkspace};

use super::{
    ax::{self, AxElement, AxValue},
    input,
};
use crate::platform::{
    Action, AdapterError, AppInfo, ElementInfo, ElementQuery, Performed, PlatformAdapter, Rect, Result,
    UiNode,
};

/// Attributes read for every node, in one round trip.
const ATTRS: [&str; 13] = [
    "AXRole",
    "AXSubrole",
    "AXRoleDescription",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXPlaceholderValue",
    "AXEnabled",
    "AXFocused",
    "AXPosition",
    "AXSize",
    "AXChildren",
    "AXTitleUIElement",
];

/// Longest value kept per element. A text area can hold a whole document.
const VALUE_CHARS: usize = 200;
/// Safety limits for one walk: nodes visited and wall-clock time.
const MAX_NODES: usize = 3000;
const WALK_BUDGET: Duration = Duration::from_millis(4000);
const SCRIPT_TIMEOUT: Duration = Duration::from_secs(20);
const FOCUS_TIMEOUT: Duration = Duration::from_secs(3);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(12);
/// How deep `find_element` looks. Menus and toolbars sit well inside this.
const FIND_DEPTH: usize = 14;

/// Reads and controls apps on macOS. By default it acts on the app in front;
/// `pinned_to` fixes it to one app by name, which is what tests use because
/// the terminal is the app in front when they run.
#[derive(Clone, Default)]
pub struct MacAdapter {
    pinned: Option<String>,
}

impl MacAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pinned_to(name: &str) -> Self {
        Self { pinned: Some(name.to_string()) }
    }

    fn require_trust() -> Result<()> {
        if ax::is_trusted() { Ok(()) } else { Err(AdapterError::NotTrusted) }
    }

    fn target(&self) -> Result<AppInfo> {
        match &self.pinned {
            Some(name) => running_app(name).ok_or_else(|| AdapterError::NoApp(format!("{name} is not running"))),
            None => self.frontmost_app(),
        }
    }

    /// `open -a` launches or activates, and macOS lets it take the front even
    /// when Chirp is not the active app. Then waits until the app is in front.
    fn focus_once(&self, name: &str) -> Result<AppInfo> {
        // macOS quits idle apps with no windows, so the app may have to start
        // from cold, which takes several seconds for Notes.
        let timeout = if running_app(name).is_some() { FOCUS_TIMEOUT } else { LAUNCH_TIMEOUT };
        let status = Command::new("/usr/bin/open")
            .args(["-a", name])
            .stderr(Stdio::null())
            .status()
            .map_err(|e| AdapterError::Other(format!("could not run open: {e}")))?;
        if !status.success() {
            return Err(AdapterError::NoApp(format!("{name} could not be opened")));
        }
        let started = Instant::now();
        loop {
            if let Some(app) = running_app(name) {
                let in_front = if ax::is_trusted() {
                    self.frontmost_app().map(|f| f.pid == app.pid).unwrap_or(false)
                } else {
                    true
                };
                if in_front {
                    return Ok(app);
                }
            }
            if started.elapsed() > timeout {
                // Usually a system dialog or another app holding the front.
                let holder = self.frontmost_app().map(|a| a.name).unwrap_or_else(|_| "something else".into());
                return Err(AdapterError::Timeout(format!("{name} did not come to the front; {holder} is in front")));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn walk(&self, depth: usize) -> Result<Node> {
        Self::require_trust()?;
        let app = self.target()?;
        let root = AxElement::application(app.pid);
        let mut walker = Walker { max_depth: depth, count: 0, deadline: Instant::now() + WALK_BUDGET };
        let mut node = walker.visit(root, 0);
        node.info.role = "AXApplication".into();
        node.info.label = Some(app.name);
        Ok(node)
    }
}

impl PlatformAdapter for MacAdapter {
    type Element = AxElement;

    fn frontmost_app(&self) -> Result<AppInfo> {
        // Ask the accessibility system for the focused app; it is live, while
        // NSWorkspace's answer depends on a run loop a command-line run lacks.
        if ax::is_trusted() {
            let focused = AxElement::system_wide().attribute("AXFocusedApplication");
            if let Some(AxValue::Element(app)) = focused {
                if let Some(info) = app.pid().and_then(app_by_pid) {
                    return Ok(info);
                }
            }
        }
        let workspace = NSWorkspace::sharedWorkspace();
        workspace
            .frontmostApplication()
            .and_then(|app| app_info(&app))
            .ok_or_else(|| AdapterError::NoApp("no app is in front".into()))
    }

    fn get_ui_tree(&self, depth: usize) -> Result<UiNode> {
        Ok(self.walk(depth)?.into_ui_node())
    }

    fn find_element(&self, query: &ElementQuery) -> Result<Vec<(AxElement, ElementInfo)>> {
        let root = self.walk(FIND_DEPTH)?;
        let wanted_role = query.role.as_deref().map(normalise_role);
        let mut hits: Vec<(i32, AxElement, ElementInfo)> = Vec::new();
        root.each(&mut |node| {
            if let Some(role) = &wanted_role {
                if normalise_role(&node.info.role) != *role {
                    return;
                }
            }
            let Some(label) = node.info.label.as_deref() else { return };
            let mut points = match_score(&query.text, label);
            if points == 0 {
                return;
            }
            if node.info.enabled {
                points += 5;
            }
            if node.info.frame.is_some_and(|f| f.w > 0.0 && f.h > 0.0) {
                points += 3;
            }
            hits.push((points, node.element.clone(), node.info.clone()));
        });
        // Stable, so equal scores keep tree order (top to bottom, front to back).
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        if hits.is_empty() {
            return Err(AdapterError::NotFound(format!("\"{}\"", query.text)));
        }
        Ok(hits.into_iter().map(|(_, element, info)| (element, info)).collect())
    }

    fn perform(&self, element: &AxElement, action: Action) -> Result<Performed> {
        Self::require_trust()?;
        let result = match action {
            Action::Press => return press(element),
            Action::Raise => element.perform("AXRaise"),
            Action::ShowMenu => element.perform("AXShowMenu"),
            Action::Increment => element.perform("AXIncrement"),
            Action::Decrement => element.perform("AXDecrement"),
            Action::Focus => element.set_bool("AXFocused", true),
            Action::Click => return click(element),
        };
        result
            .map(|()| Performed::Directly)
            .map_err(|code| AdapterError::ActionFailed(format!("{action:?}: {}", ax::error_name(code))))
    }

    fn type_text(&self, text: &str) -> Result<()> {
        Self::require_trust()?;
        // Never type into a password field: the blocklist covers passwords.
        if let Some(AxValue::Element(focused)) = AxElement::system_wide().attribute("AXFocusedUIElement") {
            let subrole = focused.attribute("AXSubrole").and_then(AxValue::text);
            if subrole.as_deref() == Some("AXSecureTextField") {
                return Err(AdapterError::Blocked("the focused field is a password field".into()));
            }
        }
        input::type_text(text)
    }

    fn press_keys(&self, keys: &str) -> Result<()> {
        Self::require_trust()?;
        input::press_keys(keys)
    }

    fn focus_app(&self, name: &str) -> Result<AppInfo> {
        // Another app can grab the front between the request and the check (a
        // chat app showing a message, say), so ask twice before giving up.
        let mut last = AdapterError::Timeout(String::new());
        for _ in 0..2 {
            match self.focus_once(name) {
                Ok(app) => return Ok(app),
                Err(error @ AdapterError::Timeout(_)) => last = error,
                Err(other) => return Err(other),
            }
        }
        Err(last)
    }

    fn run_os_script(&self, script: &str) -> Result<String> {
        let mut child = Command::new("/usr/bin/osascript")
            .args(["-e", script])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AdapterError::Other(format!("could not run osascript: {e}")))?;
        let started = Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if started.elapsed() > SCRIPT_TIMEOUT => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AdapterError::Timeout(
                        "the script did not finish; an Automation permission prompt may be waiting".into(),
                    ));
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(e) => return Err(AdapterError::Other(e.to_string())),
            }
        }
        let output = child.wait_with_output().map_err(|e| AdapterError::Other(e.to_string()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(automation_denied(&stderr).unwrap_or(AdapterError::ActionFailed(stderr)))
        }
    }
}

/// Presses an element: its own press action if it has one, else selects it
/// (rows and cells have no press action), else clicks its centre. The last two
/// are only tried when the element truly lacks the action, never when the app
/// is just busy.
fn press(element: &AxElement) -> Result<Performed> {
    // A disabled control accepts the press and does nothing, which would be
    // reported as success.
    if element.attribute("AXEnabled").and_then(|v| v.boolean()) == Some(false) {
        return Err(AdapterError::ActionFailed("the element is disabled".into()));
    }
    let code = match element.perform("AXPress") {
        Ok(()) => return Ok(Performed::Directly),
        Err(code) => code,
    };
    // -25206 action unsupported, -25205 attribute unsupported, -25200 failure.
    if !matches!(code, -25206 | -25205 | -25200) {
        return Err(AdapterError::ActionFailed(format!("Press: {}", ax::error_name(code))));
    }
    let role = element.attribute("AXRole").and_then(AxValue::text).unwrap_or_default();
    if matches!(role.as_str(), "AXRow" | "AXCell" | "AXOutlineRow") && element.set_bool("AXSelected", true).is_ok() {
        let selected = element.attribute("AXSelected").and_then(|v| v.boolean()).unwrap_or(false);
        if selected {
            return Ok(Performed::Selected);
        }
    }
    click(element)
}

fn click(element: &AxElement) -> Result<Performed> {
    let frame = element
        .frame()
        .filter(|f| f.w > 0.0 && f.h > 0.0)
        .ok_or_else(|| AdapterError::ActionFailed("element is not on screen, so it cannot be clicked".into()))?;
    let (x, y) = frame.center();
    input::click_at(x, y)?;
    Ok(Performed::Clicked)
}

/// A tree node that still holds its live element, so `find_element` can hand
/// it back for `perform`.
struct Node {
    element: AxElement,
    info: ElementInfo,
    children: Vec<Node>,
    truncated: bool,
}

impl Node {
    fn each(&self, visit: &mut impl FnMut(&Node)) {
        visit(self);
        for child in &self.children {
            child.each(visit);
        }
    }

    fn into_ui_node(self) -> UiNode {
        UiNode {
            info: self.info,
            truncated: self.truncated,
            children: self.children.into_iter().map(Node::into_ui_node).collect(),
        }
    }
}

struct Walker {
    max_depth: usize,
    count: usize,
    deadline: Instant,
}

impl Walker {
    fn visit(&mut self, element: AxElement, depth: usize) -> Node {
        self.count += 1;
        let (info, children) = read_node(&element);
        let mut node = Node { element, info, children: Vec::new(), truncated: false };
        if children.is_empty() {
            return node;
        }
        if depth >= self.max_depth || self.count >= MAX_NODES || Instant::now() > self.deadline {
            node.truncated = true;
            return node;
        }
        for child in children {
            if self.count >= MAX_NODES || Instant::now() > self.deadline {
                node.truncated = true;
                break;
            }
            node.children.push(self.visit(child, depth + 1));
        }
        node
    }
}

/// Reads one element's description and its children in a single batched call.
fn read_node(element: &AxElement) -> (ElementInfo, Vec<AxElement>) {
    let mut values = element.attributes(&ATTRS).into_iter();
    let mut next = || values.next().flatten();
    let role = next().and_then(AxValue::text).unwrap_or_default();
    let subrole = next().and_then(AxValue::text);
    let role_name = next().and_then(AxValue::text);
    let title = next().and_then(AxValue::text);
    let description = next().and_then(AxValue::text);
    let value = next();
    let placeholder = next().and_then(AxValue::text);
    let enabled = next().and_then(|v| v.boolean()).unwrap_or(true);
    let focused = next().and_then(|v| v.boolean()).unwrap_or(false);
    let position = next();
    let size = next();
    let children = next().map(AxValue::elements).unwrap_or_default();
    let title_element = next();

    let secure = subrole.as_deref() == Some("AXSecureTextField");
    // Password contents are never read, so they can never be logged or sent.
    let value_text = if secure { None } else { value.and_then(AxValue::text) }.filter(|v| !v.is_empty());

    let mut label = [title, description, placeholder].into_iter().flatten().find(|s| !s.trim().is_empty());
    if label.is_none() {
        // A text field is often labelled by a separate static text element.
        if let Some(AxValue::Element(owner)) = title_element {
            label = owner
                .attributes(&["AXValue", "AXTitle"])
                .into_iter()
                .flatten()
                .find_map(AxValue::text)
                .filter(|s| !s.trim().is_empty());
        }
    }
    if label.is_none() && role == "AXStaticText" {
        label = value_text.clone();
    }
    let frame = match (position, size) {
        (Some(AxValue::Point(x, y)), Some(AxValue::Size(w, h))) => Some(Rect { x, y, w, h }),
        _ => None,
    };
    let info = ElementInfo {
        role,
        subrole,
        role_name,
        label: label.map(|l| shorten(&l)),
        value: value_text.map(|v| shorten(&v)),
        enabled,
        focused,
        frame,
    };
    (info, children)
}

/// Recognises macOS's "not authorised to send Apple events" error (-1743).
fn automation_denied(stderr: &str) -> Option<AdapterError> {
    if !stderr.contains("(-1743)") {
        return None;
    }
    let app = stderr
        .split("send Apple events to ")
        .nth(1)
        .map(|rest| rest.split(|c| c == '.' || c == '(').next().unwrap_or("").trim())
        .filter(|name| !name.is_empty())
        .unwrap_or("that app");
    Some(AdapterError::AutomationDenied(app.to_string()))
}

fn shorten(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if flat.chars().count() <= VALUE_CHARS {
        flat
    } else {
        let cut: String = flat.chars().take(VALUE_CHARS).collect();
        format!("{cut}…")
    }
}

fn normalise_role(role: &str) -> String {
    let role = role.trim();
    role.strip_prefix("AX").unwrap_or(role).to_lowercase()
}

/// Lowercases, drops a trailing ellipsis and collapses spaces, so "New Note…"
/// and "new note" compare equal.
fn normalise_label(text: &str) -> String {
    let trimmed = text.trim().trim_end_matches('…').trim_end_matches("...");
    trimmed.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// How well a label matches what the user asked for. 0 means no match.
/// Exact beats prefix beats whole-word beats substring; shorter labels win ties
/// because "Note" should pick "Note" over "Note about notes".
pub fn match_score(query: &str, label: &str) -> i32 {
    let query = normalise_label(query);
    let label = normalise_label(label);
    if query.is_empty() || label.is_empty() {
        return 0;
    }
    let extra = (label.chars().count() as i32 - query.chars().count() as i32).clamp(0, 30);
    if label == query {
        100
    } else if label.starts_with(&query) {
        60 - extra
    } else if label.split(|c: char| !c.is_alphanumeric()).any(|word| word == query)
        || label.contains(&format!(" {query}"))
    {
        40 - extra
    } else if label.contains(&query) {
        20 - extra / 2
    } else {
        0
    }
}

fn app_info(app: &NSRunningApplication) -> Option<AppInfo> {
    let name = app.localizedName()?.to_string();
    Some(AppInfo { name, bundle_id: app.bundleIdentifier().map(|b| b.to_string()), pid: app.processIdentifier() })
}

fn app_by_pid(pid: i32) -> Option<AppInfo> {
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).and_then(|app| app_info(&app))
}

/// Finds a running app by name: exact (ignoring case) first, then a unique
/// partial match.
fn running_app(name: &str) -> Option<AppInfo> {
    let wanted = name.trim().to_lowercase();
    let apps: Vec<AppInfo> = NSWorkspace::sharedWorkspace()
        .runningApplications()
        .iter()
        .filter_map(|app| app_info(&app))
        .collect();
    if let Some(exact) = apps.iter().find(|a| a.name.to_lowercase() == wanted) {
        return Some(exact.clone());
    }
    let mut partial = apps.into_iter().filter(|a| a.name.to_lowercase().contains(&wanted));
    let first = partial.next()?;
    partial.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_beats_prefix_beats_substring() {
        let exact = match_score("new note", "New Note");
        let prefix = match_score("new", "New Note");
        let word = match_score("note", "New Note");
        let inside = match_score("ew", "New Note");
        assert!(exact > prefix && prefix > word && word > inside && inside > 0);
    }

    #[test]
    fn ignores_case_ellipsis_and_spacing() {
        assert_eq!(match_score("save as", "Save As…"), 100);
        assert_eq!(match_score("  Save   As ", "save as..."), 100);
    }

    #[test]
    fn shorter_label_wins_a_tie() {
        assert!(match_score("note", "Note") > match_score("note", "Note about everything else"));
        assert!(match_score("new", "New") > match_score("new", "New Folder With Selection"));
    }

    #[test]
    fn recognises_denied_automation() {
        let message = "33:37: execution error: Not authorised to send Apple events to Finder. (-1743)";
        match automation_denied(message) {
            Some(AdapterError::AutomationDenied(app)) => assert_eq!(app, "Finder"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(automation_denied("syntax error (-2741)").is_none());
    }

    #[test]
    fn no_match_is_zero() {
        assert_eq!(match_score("delete", "Duplicate"), 0);
        assert_eq!(match_score("", "Anything"), 0);
        assert_eq!(match_score("x", ""), 0);
    }

    #[test]
    fn roles_compare_without_prefix_or_case() {
        assert_eq!(normalise_role("AXButton"), normalise_role("button"));
        assert_eq!(normalise_role(" Button "), "button");
    }

    #[test]
    fn long_values_are_shortened_and_flattened() {
        let long = "a\n".repeat(300);
        let short = shorten(&long);
        assert!(short.chars().count() <= VALUE_CHARS + 1);
        assert!(!short.contains('\n'));
        assert_eq!(shorten("hello"), "hello");
    }
}
