//! First-run checks for the setup list on Home: microphone permission,
//! Accessibility, and what the fn key does. All read-only.
#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};

#[derive(Clone, Copy, PartialEq, Default)]
pub enum Mic {
    #[default]
    Unknown,
    Allowed,
    Denied,
}

#[derive(Clone, Copy, Default)]
pub struct Checks {
    pub mic: Mic,
    pub accessibility: bool,
    /// macOS "Press fn/🌐 key to: Do nothing", so holding fn doesn't also open something.
    pub fn_free: bool,
}

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVMediaTypeAudio: *mut Object;
}

unsafe extern "C" {
    fn CFPreferencesCopyAppValue(
        key: core_foundation::string::CFStringRef,
        app: core_foundation::string::CFStringRef,
    ) -> core_foundation::base::CFTypeRef;
}

fn mic() -> Mic {
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
    let status: isize = unsafe {
        msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio]
    };
    match status {
        3 => Mic::Allowed,
        1 | 2 => Mic::Denied,
        _ => Mic::Unknown,
    }
}

fn fn_usage() -> Option<i64> {
    unsafe {
        let key = CFString::new("AppleFnUsageType");
        let app = CFString::new("com.apple.HIToolbox");
        let value = CFPreferencesCopyAppValue(key.as_concrete_TypeRef(), app.as_concrete_TypeRef());
        if value.is_null() {
            return None;
        }
        CFNumber::wrap_under_create_rule(value as _).to_i64()
    }
}

pub fn check() -> Checks {
    Checks {
        mic: mic(),
        accessibility: crate::hotkey::trusted(false),
        // Missing means the macOS default, which is the emoji picker or dictation.
        fn_free: fn_usage() == Some(0),
    }
}

pub fn open_settings(pane: &str) {
    let _ = std::process::Command::new("open")
        .arg(format!("x-apple.systempreferences:{pane}"))
        .spawn();
}

pub const PANE_ACCESSIBILITY: &str = "com.apple.preference.security?Privacy_Accessibility";
pub const PANE_MICROPHONE: &str = "com.apple.preference.security?Privacy_Microphone";
pub const PANE_KEYBOARD: &str = "com.apple.Keyboard-Settings.extension";
