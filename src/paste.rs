//! Put text into whatever text box has focus: clipboard, a real cmd+V, then
//! the old clipboard back.

use std::thread::sleep;
use std::time::Duration;

use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

const CMD: CGKeyCode = 55;
const V: CGKeyCode = 9;

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
