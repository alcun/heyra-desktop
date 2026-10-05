//! Push-to-talk keys (macOS): hold fn, or ctrl+opt+F12, which is what a TING
//! sends through tingle + tinghold. Needs the Accessibility permission.

use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

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

/// The tap's port, so the callback can switch the tap back on if macOS turns it off.
static TAP: AtomicPtr<std::ffi::c_void> = AtomicPtr::new(std::ptr::null_mut());

/// Runs forever on the calling thread. The callback must stay fast: macOS
/// disables a tap that keeps it waiting, so it only flips flags and sends.
pub fn listen(on_key: impl Fn(Key) + 'static) -> Result<(), String> {
    let fn_held = AtomicBool::new(false);
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown, CGEventType::KeyUp],
        move |_, kind, event| {
            if matches!(kind, CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput) {
                let port = TAP.load(Ordering::Relaxed);
                if !port.is_null() {
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
                }
                return Some(event.clone());
            }
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let flags = event.get_flags();
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
