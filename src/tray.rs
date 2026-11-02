//! The menu bar icon: Heyra lives here, not in the Dock.

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

pub struct Tray {
    _icon: TrayIcon,
    open: MenuItem,
    paste_last: MenuItem,
    /// Voice model: one tick per listed model.
    models: Vec<(&'static str, CheckMenuItem)>,
    quit: MenuItem,
}

pub enum Action {
    Open,
    PasteLast,
    UseModel(&'static str),
    Quit,
}

/// A small gauge mark: three rounded bars, tall-short-tall like an h, drawn at 2x for a crisp template icon.
fn mark() -> Icon {
    const S: usize = 36;
    let mut rgba = vec![0u8; S * S * 4];
    // (x centre, half height) for each bar
    let bars = [(10.0f32, 12.0f32), (18.0, 6.0), (26.0, 12.0)];
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
        let paste_last = MenuItem::new("Paste last take", true, None);
        let quit = MenuItem::new("Quit Heyra", true, None);
        let voice = Submenu::new("Voice model", true);
        let models: Vec<(&'static str, CheckMenuItem)> =
            crate::model::ALL.iter().map(|m| (m.id, CheckMenuItem::new(m.name, true, false, None))).collect();
        for (_, item) in &models {
            voice.append(item).ok()?;
        }
        let menu = Menu::new();
        menu.append_items(&[&open, &paste_last, &voice, &PredefinedMenuItem::separator(), &quit]).ok()?;
        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon_templated(mark())
            .with_tooltip("Heyra: hold fn and talk")
            .build()
            .ok()?;
        Some(Self { _icon: icon, open, paste_last, models, quit })
    }

    pub fn poll(&self) -> Option<Action> {
        let event = MenuEvent::receiver().try_recv().ok()?;
        if event.id == *self.open.id() {
            Some(Action::Open)
        } else if event.id == *self.paste_last.id() {
            Some(Action::PasteLast)
        } else if event.id == *self.quit.id() {
            Some(Action::Quit)
        } else {
            self.models.iter().find(|(_, item)| event.id == *item.id()).map(|(id, _)| Action::UseModel(id))
        }
    }

    /// Tick the model in use (none for a folder of your own).
    pub fn show_model(&self, current: Option<&str>) {
        for (id, item) in &self.models {
            item.set_checked(current == Some(*id));
        }
    }
}
