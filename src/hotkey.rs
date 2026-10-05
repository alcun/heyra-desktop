//! Push-to-talk keys (macOS): hold fn, or ctrl+opt+F12, which is what a TING
//! sends through tingle + tinghold. Needs the Accessibility permission.

use std::sync::mpsc::Sender;

use core_foundation::runloop::{CFRunLoop, kCFRunLoopCommonModes};
use core_graphics::event::{
    CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
    CGEventType, EventField,
};

#[derive(Debug, Clone, Copy)]
pub enum Key {
    Down,
    Up,
}

const FN_KEY: i64 = 63;
const F12: i64 = 111;

/// Runs forever on the calling thread.
pub fn listen(tx: Sender<Key>) -> Result<(), String> {
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown, CGEventType::KeyUp],
        move |_, kind, event| {
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            let flags = event.get_flags();
            match kind {
                CGEventType::FlagsChanged if code == FN_KEY => {
                    let down = flags.contains(CGEventFlags::CGEventFlagSecondaryFn);
                    let _ = tx.send(if down { Key::Down } else { Key::Up });
                }
                CGEventType::KeyDown | CGEventType::KeyUp if code == F12 => {
                    let combo = CGEventFlags::CGEventFlagControl | CGEventFlags::CGEventFlagAlternate;
                    let held = matches!(kind, CGEventType::KeyUp) || flags.contains(combo);
                    if held {
                        let repeat = event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT);
                        if repeat == 0 {
                            let down = matches!(kind, CGEventType::KeyDown);
                            let _ = tx.send(if down { Key::Down } else { Key::Up });
                        }
                        return None; // swallow it so no app sees a stray F12
                    }
                }
                _ => {}
            }
            Some(event.clone())
        },
    )
    .map_err(|_| "could not watch the keyboard: grant Accessibility (System Settings → Privacy & Security)".to_string())?;

    let source = tap
        .mach_port
        .create_runloop_source(0)
        .map_err(|_| "event tap run loop source".to_string())?;
    unsafe { CFRunLoop::get_current().add_source(&source, kCFRunLoopCommonModes) };
    tap.enable();
    CFRunLoop::run_current();
    Ok(())
}
