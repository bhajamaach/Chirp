use std::process::Command;

use block2::RcBlock;
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    Microphone,
    Accessibility,
    InputMonitoring,
}

impl Permission {
    /// In the order they are asked on first run.
    pub const ALL: [Permission; 3] = [
        Permission::Microphone,
        Permission::Accessibility,
        Permission::InputMonitoring,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Permission::Microphone => "Microphone",
            Permission::Accessibility => "Accessibility",
            Permission::InputMonitoring => "Input Monitoring",
        }
    }

    /// Spoken reason, 12 words or fewer.
    pub fn reason(self) -> &'static str {
        match self {
            Permission::Microphone => "Chirp needs your microphone to hear you. Please choose Allow.",
            Permission::Accessibility => "Chirp needs Accessibility to control apps. Please turn it on.",
            Permission::InputMonitoring => {
                "Chirp needs Input Monitoring to detect your talk key. Please enable it."
            }
        }
    }

    fn settings_url(self) -> &'static str {
        match self {
            Permission::Microphone => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            Permission::Accessibility => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
            }
            Permission::InputMonitoring => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
            }
        }
    }
}

fn microphone_status() -> AVAuthorizationStatus {
    // SAFETY: AVMediaTypeAudio is a constant exported by AVFoundation.
    let media = unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio is always present");
    unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) }
}

pub fn is_granted(permission: Permission) -> bool {
    match permission {
        Permission::Microphone => microphone_status() == AVAuthorizationStatus::Authorized,
        Permission::Accessibility => unsafe { AXIsProcessTrusted() },
        Permission::InputMonitoring => unsafe { CGPreflightListenEventAccess() },
    }
}

/// Asks macOS for the permission. Microphone and Input Monitoring show the
/// system dialog the first time; when the user has already denied it, or for
/// Accessibility (which has no dialog API), the matching Settings pane opens.
pub fn request(permission: Permission) {
    match permission {
        Permission::Microphone => {
            if microphone_status() == AVAuthorizationStatus::NotDetermined {
                let media = unsafe { AVMediaTypeAudio }.expect("AVMediaTypeAudio is always present");
                let handler = RcBlock::new(|_granted: Bool| {});
                unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(media, &handler) };
            } else {
                open_settings(permission);
            }
        }
        Permission::InputMonitoring => {
            // Shows the system dialog the first time and registers the app in
            // the Input Monitoring list; returns false until the user agrees.
            if !unsafe { CGRequestListenEventAccess() } {
                open_settings(permission);
            }
        }
        Permission::Accessibility => open_settings(permission),
    }
}

pub fn open_settings(permission: Permission) {
    let _ = Command::new("open").arg(permission.settings_url()).status();
}
