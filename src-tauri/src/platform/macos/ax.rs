//! Thin, safe wrapper over the macOS Accessibility (AX) C API.
//!
//! Every function here is one AX call. The meaning (labels, matching, tree
//! limits) lives in `adapter.rs`.

use std::{ffi::c_void, ptr};

use core_foundation::{
    array::CFArray,
    base::{CFGetTypeID, CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    number::CFNumber,
    string::{CFString, CFStringRef},
};
use core_graphics::geometry::{CGPoint, CGSize};

use super::super::Rect;

type AXError = i32;
const AX_SUCCESS: AXError = 0;
/// kAXValueCGPointType and kAXValueCGSizeType.
const AX_VALUE_POINT: u32 = 1;
const AX_VALUE_SIZE: u32 = 2;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> CFTypeRef;
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: *mut CFTypeRef) -> AXError;
    fn AXUIElementCopyMultipleAttributeValues(
        element: CFTypeRef,
        attributes: CFTypeRef,
        options: u32,
        values: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementPerformAction(element: CFTypeRef, action: CFStringRef) -> AXError;
    fn AXUIElementSetAttributeValue(element: CFTypeRef, attribute: CFStringRef, value: CFTypeRef) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, seconds: f32) -> AXError;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> AXError;
    fn AXValueGetTypeID() -> usize;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut c_void) -> bool;
}

pub fn is_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

/// An `AXUIElementRef`. AX elements are plain CF objects that macOS lets any
/// thread use, so the handle is `Send` and `Sync`.
#[derive(Clone)]
pub struct AxElement(CFType);

unsafe impl Send for AxElement {}
unsafe impl Sync for AxElement {}

/// A decoded attribute value.
pub enum AxValue {
    Text(String),
    Bool(bool),
    Number(f64),
    Element(AxElement),
    Elements(Vec<AxElement>),
    Point(f64, f64),
    Size(f64, f64),
    Other,
}

impl AxValue {
    pub fn text(self) -> Option<String> {
        match self {
            AxValue::Text(s) => Some(s),
            AxValue::Number(n) => Some(if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }),
            AxValue::Bool(b) => Some(if b { "on".into() } else { "off".into() }),
            _ => None,
        }
    }

    pub fn boolean(&self) -> Option<bool> {
        match self {
            AxValue::Bool(b) => Some(*b),
            AxValue::Number(n) => Some(*n != 0.0),
            _ => None,
        }
    }

    pub fn elements(self) -> Vec<AxElement> {
        match self {
            AxValue::Elements(v) => v,
            AxValue::Element(e) => vec![e],
            _ => Vec::new(),
        }
    }
}

fn decode(value: CFType) -> AxValue {
    if let Some(s) = value.downcast::<CFString>() {
        return AxValue::Text(s.to_string());
    }
    if let Some(b) = value.downcast::<CFBoolean>() {
        return AxValue::Bool(b.into());
    }
    if let Some(n) = value.downcast::<CFNumber>() {
        return AxValue::Number(n.to_f64().unwrap_or(0.0));
    }
    if let Some(array) = value.downcast::<CFArray>() {
        let items = array
            .iter()
            .map(|item| unsafe { CFType::wrap_under_get_rule(*item) })
            .filter(|item| is_ax_element(item))
            .map(AxElement)
            .collect();
        return AxValue::Elements(items);
    }
    unsafe {
        let type_id = CFGetTypeID(value.as_CFTypeRef());
        if type_id == AXValueGetTypeID() {
            return match AXValueGetType(value.as_CFTypeRef()) {
                AX_VALUE_POINT => {
                    let mut p = CGPoint::new(0.0, 0.0);
                    if AXValueGetValue(value.as_CFTypeRef(), AX_VALUE_POINT, &mut p as *mut _ as *mut c_void) {
                        AxValue::Point(p.x, p.y)
                    } else {
                        AxValue::Other
                    }
                }
                AX_VALUE_SIZE => {
                    let mut s = CGSize::new(0.0, 0.0);
                    if AXValueGetValue(value.as_CFTypeRef(), AX_VALUE_SIZE, &mut s as *mut _ as *mut c_void) {
                        AxValue::Size(s.width, s.height)
                    } else {
                        AxValue::Other
                    }
                }
                _ => AxValue::Other,
            };
        }
    }
    if is_ax_element(&value) { AxValue::Element(AxElement(value)) } else { AxValue::Other }
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementGetTypeID() -> usize;
}

fn is_ax_element(value: &CFType) -> bool {
    unsafe { CFGetTypeID(value.as_CFTypeRef()) == AXUIElementGetTypeID() }
}

impl AxElement {
    pub fn application(pid: i32) -> AxElement {
        let element = unsafe { CFType::wrap_under_create_rule(AXUIElementCreateApplication(pid)) };
        let element = AxElement(element);
        element.set_timeout(2.0);
        element
    }

    pub fn system_wide() -> AxElement {
        let element = AxElement(unsafe { CFType::wrap_under_create_rule(AXUIElementCreateSystemWide()) });
        element.set_timeout(2.0);
        element
    }

    /// A hung app would otherwise block every call for the system default (6 s).
    pub fn set_timeout(&self, seconds: f32) {
        unsafe { AXUIElementSetMessagingTimeout(self.0.as_CFTypeRef(), seconds) };
    }

    pub fn pid(&self) -> Option<i32> {
        let mut pid = 0;
        (unsafe { AXUIElementGetPid(self.0.as_CFTypeRef(), &mut pid) } == AX_SUCCESS).then_some(pid)
    }

    pub fn attribute(&self, name: &str) -> Option<AxValue> {
        let name = CFString::new(name);
        let mut out: CFTypeRef = ptr::null();
        let status = unsafe { AXUIElementCopyAttributeValue(self.0.as_CFTypeRef(), name.as_concrete_TypeRef(), &mut out) };
        if status != AX_SUCCESS || out.is_null() {
            return None;
        }
        Some(decode(unsafe { CFType::wrap_under_create_rule(out) }))
    }

    /// Reads several attributes in one round trip to the app, which is much
    /// faster than one call each. A missing attribute comes back as `None`.
    pub fn attributes(&self, names: &[&str]) -> Vec<Option<AxValue>> {
        let cf_names: Vec<CFString> = names.iter().map(|n| CFString::new(n)).collect();
        let array = CFArray::from_CFTypes(&cf_names);
        let mut out: CFTypeRef = ptr::null();
        let status = unsafe {
            AXUIElementCopyMultipleAttributeValues(self.0.as_CFTypeRef(), array.as_CFTypeRef(), 0, &mut out)
        };
        if status != AX_SUCCESS || out.is_null() {
            return names.iter().map(|_| None).collect();
        }
        let result = unsafe { CFType::wrap_under_create_rule(out) };
        let Some(values) = result.downcast::<CFArray>() else {
            return names.iter().map(|_| None).collect();
        };
        let mut decoded: Vec<Option<AxValue>> = values
            .iter()
            .map(|item| {
                let item = unsafe { CFType::wrap_under_get_rule(*item) };
                // With options 0, a failed attribute is an AXValue of the error type.
                match decode(item) {
                    AxValue::Other => None,
                    value => Some(value),
                }
            })
            .collect();
        decoded.resize_with(names.len(), || None);
        decoded
    }

    pub fn perform(&self, action: &str) -> Result<(), i32> {
        let action = CFString::new(action);
        match unsafe { AXUIElementPerformAction(self.0.as_CFTypeRef(), action.as_concrete_TypeRef()) } {
            AX_SUCCESS => Ok(()),
            code => Err(code),
        }
    }

    pub fn set_bool(&self, attribute: &str, value: bool) -> Result<(), i32> {
        let attribute = CFString::new(attribute);
        let value = if value { CFBoolean::true_value() } else { CFBoolean::false_value() };
        match unsafe {
            AXUIElementSetAttributeValue(self.0.as_CFTypeRef(), attribute.as_concrete_TypeRef(), value.as_CFTypeRef())
        } {
            AX_SUCCESS => Ok(()),
            code => Err(code),
        }
    }

    pub fn frame(&self) -> Option<Rect> {
        let values = self.attributes(&["AXPosition", "AXSize"]);
        match (values.first()?.as_ref()?, values.get(1)?.as_ref()?) {
            (AxValue::Point(x, y), AxValue::Size(w, h)) => Some(Rect { x: *x, y: *y, w: *w, h: *h }),
            _ => None,
        }
    }
}

/// Turns an AX error code into words for the log. Only the common ones.
pub fn error_name(code: i32) -> &'static str {
    match code {
        -25200 => "failure",
        -25201 => "illegal argument",
        -25202 => "invalid element",
        -25204 => "cannot complete (app busy or not responding)",
        -25205 => "attribute unsupported",
        -25206 => "action unsupported",
        -25211 => "API disabled (Accessibility not granted)",
        -25212 => "no value",
        _ => "unknown error",
    }
}
