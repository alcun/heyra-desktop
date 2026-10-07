//! Put text into whatever text box has focus: clipboard, a real cmd+V, then
//! the old clipboard back.
#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use std::thread::sleep;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

const CMD: CGKeyCode = 55;
const V: CGKeyCode = 9;
pub const RETURN: CGKeyCode = 36;
pub const Z: CGKeyCode = 6;

/// Press and release one key, with cmd held when `command` is set.
pub fn tap(code: CGKeyCode, command: bool) -> Result<(), String> {
    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "event source".to_string())?;
    let flags = if command { CGEventFlags::CGEventFlagCommand } else { CGEventFlags::CGEventFlagNull };
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(source.clone(), code, down)
            .map_err(|_| "keyboard event".to_string())?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
    }
    Ok(())
}

pub fn paste(text: &str) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    let previous = clipboard.get_text().ok();
    clipboard.set_text(text).map_err(|e| e.to_string())?;
    sleep(Duration::from_millis(30));

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "event source".to_string())?;
    let key = |code: CGKeyCode, down: bool, flags: CGEventFlags| -> Result<(), String> {
        let event = CGEvent::new_keyboard_event(source.clone(), code, down)
            .map_err(|_| "keyboard event".to_string())?;
        event.set_flags(flags);
        event.post(CGEventTapLocation::HID);
        Ok(())
    };
    let cmd = CGEventFlags::CGEventFlagCommand;
    key(CMD, true, cmd)?;
    key(V, true, cmd)?;
    key(V, false, cmd)?;
    key(CMD, false, CGEventFlags::CGEventFlagNull)?;

    // Give the app time to read the clipboard before putting the old one back.
    sleep(Duration::from_millis(300));
    if let Some(previous) = previous {
        let _ = clipboard.set_text(previous);
    }
    Ok(())
}

/// Terminals, where a take is usually a command or a prompt to a coding agent.
const TERMINALS: [&str; 9] = [
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.mitchellh.ghostty",
    "dev.warp.Warp-Stable",
    "net.kovidgoyal.kitty",
    "org.alacritty",
    "com.github.wez.wezterm",
    "co.zeit.hyper",
    "com.stablyai.orca",
];

/// Is the app the text is about to go into a terminal?
pub fn in_terminal() -> bool {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: id = msg_send![workspace, frontmostApplication];
        if app == nil {
            return false;
        }
        let bundle: id = msg_send![app, bundleIdentifier];
        if bundle == nil {
            return false;
        }
        let utf8: *const std::ffi::c_char = msg_send![bundle, UTF8String];
        let name = std::ffi::CStr::from_ptr(utf8).to_string_lossy();
        TERMINALS.contains(&name.as_ref())
    }
}
