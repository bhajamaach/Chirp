//! The seam between Chirp's core and the operating system. The core only talks
//! to `PlatformAdapter`; macOS implements it now and Windows can later.

#[cfg(target_os = "macos")]
pub mod macos;

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// What the screen reader would say about one element: enough for a person or
/// the planner to tell it apart, never the contents of a password field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementInfo {
    /// Platform role, e.g. `AXButton`.
    pub role: String,
    pub subrole: Option<String>,
    /// Spoken-style role, e.g. "button".
    pub role_name: Option<String>,
    /// The best human label: title, description, placeholder or the label element's text.
    pub label: Option<String>,
    /// Current value (text field contents, checkbox state), shortened.
    pub value: Option<String>,
    pub enabled: bool,
    pub focused: bool,
    pub frame: Option<Rect>,
}

/// A snapshot of the interface, for dumping or sending to the planner.
#[derive(Clone, Debug, Default)]
pub struct UiNode {
    pub info: ElementInfo,
    pub children: Vec<UiNode>,
    /// True when the walk stopped here because of the depth or size limit.
    pub truncated: bool,
}

/// Roles that only group other elements. They are left out of the outline
/// unless they carry a label or value.
const STRUCTURAL: [&str; 6] = ["AXGroup", "AXSplitGroup", "AXScrollArea", "AXLayoutArea", "AXLayoutItem", "AXUnknown"];

/// Roles that are decoration for a listener: scroll bars, rulers, splitters.
/// They are left out of the outline together with everything inside them.
const NOISE: [&str; 5] = ["AXSplitter", "AXScrollBar", "AXValueIndicator", "AXRuler", "AXGrowArea"];

impl UiNode {
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(UiNode::count).sum::<usize>()
    }

    /// A short text description of what is on screen, for the planner or for
    /// reading aloud. The menu bar is collapsed to one line and purely
    /// structural elements are skipped, so a window fits in a few hundred
    /// tokens. Stops at `max_chars`.
    pub fn outline(&self, max_chars: usize) -> String {
        let mut out = String::new();
        self.outline_into(0, max_chars, &mut out);
        out
    }

    fn outline_into(&self, depth: usize, max_chars: usize, out: &mut String) -> bool {
        if out.chars().count() >= max_chars {
            return false;
        }
        let info = &self.info;
        if NOISE.contains(&info.role.as_str()) {
            return true;
        }
        if info.role == "AXMenuBar" {
            out.push_str(&format!("{}menu bar\n", "  ".repeat(depth)));
            return true;
        }
        let described = info.label.is_some() || info.value.is_some();
        let skip = STRUCTURAL.contains(&info.role.as_str()) && !described || (info.label.is_none() && info.value.is_none() && self.children.is_empty());
        let mut child_depth = depth;
        if !skip {
            let name = info.role_name.clone().unwrap_or_else(|| info.role.trim_start_matches("AX").to_lowercase());
            out.push_str(&"  ".repeat(depth));
            out.push_str(&name);
            if let Some(label) = &info.label {
                out.push_str(&format!(" \"{label}\""));
            }
            if let Some(value) = &info.value {
                if info.label.as_ref() != Some(value) {
                    out.push_str(&format!(": {value}"));
                }
            }
            if !info.enabled {
                out.push_str(" (dimmed)");
            }
            out.push('\n');
            child_depth += 1;
        }
        for child in &self.children {
            if !child.outline_into(child_depth, max_chars, out) {
                return false;
            }
        }
        true
    }
}

#[derive(Clone, Debug, Default)]
pub struct ElementQuery {
    /// Text to match against the element label (case, accents and trailing "…" are ignored).
    pub text: String,
    /// Optional role filter, with or without the `AX` prefix, e.g. `button`.
    pub role: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Activate the element (button, menu item, checkbox, link).
    Press,
    /// Give the element keyboard focus.
    Focus,
    /// Bring its window to the front.
    Raise,
    ShowMenu,
    Increment,
    Decrement,
    /// A real mouse click at the element's centre. Last resort, it moves the pointer.
    Click,
}

impl Action {
    pub fn parse(name: &str) -> Option<Action> {
        Some(match name.to_ascii_lowercase().as_str() {
            "press" => Action::Press,
            "focus" => Action::Focus,
            "raise" => Action::Raise,
            "menu" | "showmenu" => Action::ShowMenu,
            "increment" | "up" => Action::Increment,
            "decrement" | "down" => Action::Decrement,
            "click" => Action::Click,
            _ => return None,
        })
    }
}

/// How an action was carried out, for the action log and for narration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Performed {
    /// The element's own accessibility action ran.
    Directly,
    /// It had no press action, so it was selected (rows and cells).
    Selected,
    /// It had neither, so the pointer clicked its centre.
    Clicked,
}

#[derive(Debug)]
pub enum AdapterError {
    /// The OS permission needed (Accessibility) has not been granted.
    NotTrusted,
    /// No such app is running, or none is in front.
    NoApp(String),
    /// No element matched.
    NotFound(String),
    /// The element refused the action.
    ActionFailed(String),
    /// macOS has not allowed Chirp to send Apple events to this app (error -1743).
    /// Each app asks once, the first time it is scripted.
    AutomationDenied(String),
    /// Deliberately refused for safety, e.g. typing into a password field.
    Blocked(String),
    Timeout(String),
    #[allow(dead_code)] // returned by `screenshot` once a platform implements it
    Unsupported(&'static str),
    Other(String),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::NotTrusted => write!(f, "Accessibility permission is not granted"),
            AdapterError::NoApp(m) => write!(f, "no such app: {m}"),
            AdapterError::NotFound(m) => write!(f, "nothing found for {m}"),
            AdapterError::ActionFailed(m) => write!(f, "action failed: {m}"),
            AdapterError::AutomationDenied(app) => write!(f, "not allowed to control {app} (Automation permission)"),
            AdapterError::Blocked(m) => write!(f, "blocked: {m}"),
            AdapterError::Timeout(m) => write!(f, "timed out: {m}"),
            AdapterError::Unsupported(m) => write!(f, "not supported: {m}"),
            AdapterError::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for AdapterError {}

pub type Result<T> = std::result::Result<T, AdapterError>;

/// Everything the agent needs from the operating system. `get_ui_tree` and
/// `find_element` look at the app in front unless the adapter was pinned to one.
pub trait PlatformAdapter {
    /// A live handle to one element, valid for the current screen state.
    type Element: Clone;

    fn frontmost_app(&self) -> Result<AppInfo>;
    fn get_ui_tree(&self, depth: usize) -> Result<UiNode>;
    /// Best matches first.
    fn find_element(&self, query: &ElementQuery) -> Result<Vec<(Self::Element, ElementInfo)>>;
    /// `Press` falls back to selecting, then to clicking, when the element has
    /// no press action of its own; the result says which happened.
    fn perform(&self, element: &Self::Element, action: Action) -> Result<Performed>;
    fn type_text(&self, text: &str) -> Result<()>;
    /// A shortcut such as `cmd+shift+n`, or one key such as `return`.
    fn press_keys(&self, keys: &str) -> Result<()>;
    /// Brings the named app to the front, launching it if needed.
    fn focus_app(&self, name: &str) -> Result<AppInfo>;
    /// Runs OS scripting (AppleScript on macOS) and returns what it printed.
    fn run_os_script(&self, script: &str) -> Result<String>;
    /// Not built in v0.1: the interface is here for the vision fallback.
    #[allow(dead_code)]
    fn screenshot(&self) -> Result<Vec<u8>> {
        Err(AdapterError::Unsupported("screenshot"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str, label: Option<&str>, children: Vec<UiNode>) -> UiNode {
        UiNode {
            info: ElementInfo { role: role.into(), label: label.map(Into::into), enabled: true, ..Default::default() },
            children,
            truncated: false,
        }
    }

    #[test]
    fn outline_skips_structure_and_collapses_the_menu_bar() {
        let tree = node(
            "AXApplication",
            Some("Notes"),
            vec![
                node("AXWindow", Some("All Notes"), vec![node("AXGroup", None, vec![node("AXButton", Some("New Note"), vec![])])]),
                node("AXMenuBar", None, vec![node("AXMenuBarItem", Some("File"), vec![])]),
            ],
        );
        let text = tree.outline(1000);
        assert_eq!(text, "application \"Notes\"\n  window \"All Notes\"\n    button \"New Note\"\n  menu bar\n");
    }

    #[test]
    fn outline_stops_at_the_limit() {
        let kids = (0..100).map(|i| node("AXButton", Some(&format!("Button {i}")), vec![])).collect();
        let text = node("AXWindow", Some("W"), kids).outline(200);
        assert!(text.chars().count() < 260, "{}", text.chars().count());
        assert!(text.lines().count() < 100);
    }
}
