//! The menu bar icon: Heyra lives here, not in the Dock.

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    _icon: TrayIcon,
    open: MenuItem,
    quit: MenuItem,
}

pub enum Action {
    Open,
    Quit,
}

/// A small gauge mark: three rounded bars, drawn at 2x for a crisp template icon.
fn mark() -> Icon {
    const S: usize = 36;
    let mut rgba = vec![0u8; S * S * 4];
    // (x centre, half height) for each bar
    let bars = [(10.0f32, 6.0f32), (18.0, 12.0), (26.0, 8.0)];
    for y in 0..S {
        for x in 0..S {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut a = 0.0f32;
            for (cx, hh) in bars {
                // capsule: width 4.4, rounded ends
                let dx = (px - cx).abs();
                let dy = ((py - 18.0).abs() - (hh - 2.2)).max(0.0);
                let d = (dx * dx + dy * dy).sqrt() - 2.2;
                a = a.max((0.5 - d).clamp(0.0, 1.0));
            }
            let i = (y * S + x) * 4;
            rgba[i + 3] = (a * 255.0) as u8;
        }
    }
    Icon::from_rgba(rgba, S as u32, S as u32).expect("icon")
}

impl Tray {
    pub fn new() -> Option<Self> {
        let open = MenuItem::new("Open Heyra", true, None);
        let quit = MenuItem::new("Quit Heyra", true, None);
        let menu = Menu::new();
        menu.append_items(&[&open, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(mark())
            .with_tooltip("Heyra: hold fn and talk")
            .build()
            .ok()?;
        Some(Self { _icon: icon, open, quit })
    }

    pub fn poll(&self) -> Option<Action> {
        let event = MenuEvent::receiver().try_recv().ok()?;
        if event.id == *self.open.id() {
            Some(Action::Open)
        } else if event.id == *self.quit.id() {
            Some(Action::Quit)
        } else {
            None
        }
    }
}
