//! Push-to-talk keys (macOS): hold fn, or ctrl+opt+F12, which is what a TING
//! sends through tingle + tinghold, or a key or mouse button the user chose.
//! Needs the Accessibility permission.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::store::Button;

use core_foundation::base::TCFType;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, EventField,
};

#[derive(Debug, Clone, Copy)]
pub enum Key {
    Down,
    Up,
    /// Another key was pressed while fn was held (fn+arrow, fn+delete…): not dictation.
    Cancel,
    /// Esc during a take: throw it away.
    Escape,
}

const FN_KEY: i64 = 63;
const F12: i64 = 111;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: core_foundation::dictionary::CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: core_foundation::string::CFStringRef;
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
}

/// Is Heyra allowed to watch the keyboard? With `prompt`, macOS shows its
/// "allow in Accessibility settings" dialog.
pub fn trusted(prompt: bool) -> bool {
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::from(prompt))]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

const ESCAPE: i64 = 53;

/// A take is recording, so Esc belongs to Heyra and no other app sees it.
static TAKING: AtomicBool = AtomicBool::new(false);

pub fn set_taking(on: bool) {
    TAKING.store(on, Ordering::Relaxed);
}

/// The user's own push-to-talk button, if any.
static BUTTON: Mutex<Option<Button>> = Mutex::new(None);
/// The next key or mouse button pressed becomes the button.
static RECORDING: AtomicBool = AtomicBool::new(false);

fn modifiers(flags: CGEventFlags) -> u64 {
    (flags
        & (CGEventFlags::CGEventFlagCommand
            | CGEventFlags::CGEventFlagControl
            | CGEventFlags::CGEventFlagAlternate
            | CGEventFlags::CGEventFlagShift))
        .bits()
}

pub fn set_button(button: Option<Button>) {
    *BUTTON.lock().unwrap() = button;
}

pub fn button() -> Option<Button> {
    *BUTTON.lock().unwrap()
}

/// Listen for the next press and make it the button; escape cancels.
pub fn record(on: bool) {
    RECORDING.store(on, Ordering::Relaxed);
}

pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// "⌃⌥F12", "Mouse button 4".
pub fn name(button: Button) -> String {
    if button.mouse {
        return format!("Mouse button {}", button.code + 1);
    }
    let flags = CGEventFlags::from_bits_truncate(button.mods);
    let mut out = String::new();
    for (flag, mark) in [
        (CGEventFlags::CGEventFlagControl, "⌃"),
        (CGEventFlags::CGEventFlagAlternate, "⌥"),
        (CGEventFlags::CGEventFlagShift, "⇧"),
        (CGEventFlags::CGEventFlagCommand, "⌘"),
    ] {
        if flags.contains(flag) {
            out.push_str(mark);
        }
    }
    const LETTERS: &str = "asdfhgzxcv?bqweryt123465=97-80]ou[ip?lj'k;\\,/nm.";
    let key = match button.code {
        122 => "F1".into(), 120 => "F2".into(), 99 => "F3".into(), 118 => "F4".into(),
        96 => "F5".into(), 97 => "F6".into(), 98 => "F7".into(), 100 => "F8".into(),
        101 => "F9".into(), 109 => "F10".into(), 103 => "F11".into(), 111 => "F12".into(),
        105 => "F13".into(), 107 => "F14".into(), 113 => "F15".into(), 106 => "F16".into(),
        64 => "F17".into(), 79 => "F18".into(), 80 => "F19".into(), 90 => "F20".into(),
        36 => "Return".into(), 48 => "Tab".into(), 49 => "Space".into(), 51 => "Delete".into(),
        117 => "Forward Delete".into(), 115 => "Home".into(), 119 => "End".into(),
        116 => "Page Up".into(), 121 => "Page Down".into(),
        123 => "←".into(), 124 => "→".into(), 125 => "↓".into(), 126 => "↑".into(), 50 => "`".into(),
        c @ 0..=47 => LETTERS.chars().nth(c as usize).filter(|&c| c != '?').map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| format!("key {c}")),
        c => format!("key {c}"),
    };
    out + &key
}

/// The tap's port, so the callback can switch the tap back on if macOS turns it off.
static TAP: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Runs forever on the calling thread. The callback must stay fast: macOS
/// disables a tap that keeps it waiting, so it only flips flags and sends.
pub fn listen(on_key: impl Fn(Key) + 'static) -> Result<(), String> {
    let fn_held = AtomicBool::new(false);
    let button_held = AtomicBool::new(false);
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![
            CGEventType::FlagsChanged,
            CGEventType::KeyDown,
            CGEventType::KeyUp,
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
        ],
        move |_, kind, event| {
            if matches!(kind, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
                let port = TAP.load(Ordering::Relaxed);
                if !port.is_null() {
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
                }
                return Some(event.clone());
            }
            let mouse = matches!(kind, CGEventType::OtherMouseDown | CGEventType::OtherMouseUp);
            let code = event.get_integer_value_field(if mouse {
                EventField::MOUSE_EVENT_BUTTON_NUMBER
            } else {
                EventField::KEYBOARD_EVENT_KEYCODE
            });
            let flags = event.get_flags();
            let down = matches!(kind, CGEventType::KeyDown | CGEventType::OtherMouseDown);
            let repeat = !mouse && event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;

            // Choosing a button in Settings: the next press is it.
            if down && !repeat && recording() {
                record(false);
                if !(code == ESCAPE && !mouse) {
                    let button = Button { mouse, code, mods: modifiers(flags) };
                    set_button(Some(button));
                    std::thread::spawn(move || {
                        let settings = crate::store::load_settings();
                        crate::store::save_settings(&crate::store::Settings { button: Some(button), ..settings });
                    });
                }
                return None;
            }
            if let Some(button) = button()
                && button.mouse == mouse
                && button.code == code
                && !matches!(kind, CGEventType::FlagsChanged)
            {
                if down && (repeat || modifiers(flags) == button.mods) {
                    if !repeat {
                        button_held.store(true, Ordering::Relaxed);
                        on_key(Key::Down);
                    }
                    return None;
                }
                if !down && button_held.swap(false, Ordering::Relaxed) {
                    on_key(Key::Up);
                    return None;
                }
            }

            if matches!(kind, CGEventType::KeyDown) && !mouse && code == ESCAPE && TAKING.load(Ordering::Relaxed) {
                on_key(Key::Escape);
                return None;
            }

            match kind {
                CGEventType::FlagsChanged if code == FN_KEY => {
                    let down = flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
                    if fn_held.swap(down, Ordering::Relaxed) != down {
                        on_key(if down { Key::Down } else { Key::Up });
                    }
                }
                CGEventType::KeyDown | CGEventType::KeyUp if code == F12 => {
                    let combo = CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagAlternate;
                    let held = matches!(kind, CGEventType::KeyUp) || flags.contains(combo);
                    if held {
                        let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT);
                        if repeat == 0 {
                            let down = matches!(kind, CGEventType::KeyDown);
                            on_key(if down { Key::Down } else { Key::Up });
                        }
                        return None; // swallow it so no app sees a stray F12
                    }
                }
                CGEventType::KeyDown if fn_held.load(Ordering::Relaxed) => on_key(Key::Cancel),
                _ => {}
            }
            Some(event.clone())
        },
    )
    .map_err(|_| "could not watch the keyboard: grant Accessibility (System Settings → Privacy & Security)".to_string())?;

    TAP.store(tap.mach_port.as_concrete_TypeRef() as *mut _, Ordering::Relaxed);
    let source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| "event tap run loop source".to_string())?;
    unsafe { CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes) };
    tap.enable();
    crate::store::log("keyboard tap running");
    CFRunLoop::run_current();
    Ok(())
}
