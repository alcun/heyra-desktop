//! The main window (sidebar + pages) and the floating listening pill.

use std::sync::mpsc::Sender;
use std::time::Instant;

use gpui::{
    App, Bounds, Context, Hsla, SharedString, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, point, prelude::*, px, rgb, size,
};

use crate::store;
use crate::worker::{Cmd, Phase, Shared};

// Palette: warm dark, one accent.
const BG: u32 = 0x18181b;
const SIDEBAR: u32 = 0x111113;
const CARD: u32 = 0x232327;
const TEXT: u32 = 0xededef;
const MUTED: u32 = 0x9f9fa9;
const ACCENT: u32 = 0xd9a86a;
const LISTEN: u32 = 0xe5484d;
const OK: u32 = 0x46a758;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    History,
    Dictionary,
    Settings,
}

pub struct Main {
    state: Shared,
    cmds: Sender<Cmd>,
    page: Page,
    copied: Option<usize>,
}

pub fn open_main(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    let bounds = Bounds::centered(None, size(px(760.), px(520.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Heyra".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            window_min_size: Some(size(px(560.), px(380.))),
            ..Default::default()
        },
        |_, cx| cx.new(|_| Main { state, cmds, page: Page::Home, copied: None }),
    )
    .ok();
}

fn dot(color: u32) -> impl IntoElement {
    div().flex_none().size_2().rounded_full().bg(rgb(color))
}

fn card() -> gpui::Div {
    div().p_4().rounded_lg().bg(rgb(CARD)).flex().flex_col().gap_2()
}

fn heading(text: &'static str) -> impl IntoElement {
    div().text_xl().text_color(rgb(TEXT)).child(text)
}

fn muted(text: impl Into<SharedString>) -> impl IntoElement {
    div().text_sm().text_color(rgb(MUTED)).child(text.into())
}

fn button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_md()
        .bg(rgb(0x2e2e33))
        .text_sm()
        .text_color(rgb(TEXT))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(0x3a3a40)))
        .child(label)
}

fn when(at: u64) -> String {
    let ago = store::now().saturating_sub(at);
    match ago {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", ago / 60),
        3600..=86_399 => format!("{} h ago", ago / 3600),
        _ => format!("{} days ago", ago / 86_400),
    }
}

impl Main {
    fn nav(&self, id: &'static str, label: &'static str, page: Page, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.page == page;
        div()
            .id(id)
            .px_3()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .text_color(rgb(if active { TEXT } else { MUTED }))
            .when(active, |d| d.bg(rgb(CARD)))
            .hover(|s| s.text_color(rgb(TEXT)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                this.copied = None;
                cx.notify();
            }))
            .child(label)
    }

    fn rows(&self, indices: impl Iterator<Item = usize>, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut rows = Vec::new();
        for i in indices {
            rows.push(self.history_row(i, cx).into_any_element());
        }
        rows
    }

    fn history_row(&self, index: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let s = self.state.lock().unwrap();
        let entry = &s.history[index];
        let text = entry.text.clone();
        let copied = self.copied == Some(index);
        div()
            .id(("history", index))
            .p_3()
            .rounded_md()
            .bg(rgb(CARD))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(0x2a2a2f)))
            .flex()
            .flex_col()
            .gap_1()
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(text.clone());
                }
                this.copied = Some(index);
                cx.notify();
            }))
            .child(div().text_color(rgb(TEXT)).child(entry.text.clone()))
            .child(muted(if copied {
                "Copied".to_string()
            } else {
                format!("{} · {} words · click to copy", when(entry.at), entry.words())
            }))
    }

    fn home(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, message, total_words, today_words, wpm, count) = {
            let s = self.state.lock().unwrap();
            let blocked = s.blocker.clone();
            let words: usize = s.history.iter().map(|e| e.words()).sum();
            let secs: f32 = s.history.iter().map(|e| e.secs).sum();
            let midnight = store::now() - store::now() % 86_400;
            let today: usize = s.history.iter().filter(|e| e.at >= midnight).map(|e| e.words()).sum();
            let wpm = if secs > 0.0 { (words as f32 / secs * 60.0).round() as usize } else { 0 };
            match blocked {
                Some(b) => (Phase::Error, b, words, today, wpm, s.history.len()),
                None => (s.phase, s.message.clone(), words, today, wpm, s.history.len()),
            }
        };
        let color = match phase {
            Phase::Listening => LISTEN,
            Phase::Error => LISTEN,
            Phase::Ready => OK,
            _ => ACCENT,
        };
        let stat = |value: String, label: &'static str| {
            card()
                .flex_1()
                .child(div().text_2xl().text_color(rgb(TEXT)).child(value))
                .child(muted(label))
        };
        let recent: Vec<usize> = (0..count).rev().take(5).collect();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(heading("Home"))
            .child(
                card().child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(dot(color))
                        .child(div().flex_1().min_w_0().text_color(rgb(TEXT)).child(message)),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(stat(today_words.to_string(), "words today"))
                    .child(stat(total_words.to_string(), "words in total"))
                    .child(stat(wpm.to_string(), "words per minute")),
            )
            .child(muted(if recent.is_empty() {
                "Hold fn anywhere and talk. What you say lands where your cursor is."
            } else {
                "Recent"
            }))
            .children(self.rows(recent.into_iter(), cx))
    }

    fn history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().unwrap().history.len();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading("History"))
            .child(muted("Everything you've said, kept on this machine only. Click to copy."))
            .children(self.rows((0..count).rev(), cx))
    }

    fn dictionary(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let rules = store::load_dictionary();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading("Dictionary"))
            .child(muted("Words it gets wrong, fixed every time. One rule per line: heard => meant."))
            .child(
                div().flex().gap_2().child(button("edit-dict", "Edit dictionary").on_click(|_, _, _| {
                    let _ = std::process::Command::new("open")
                        .arg("-t")
                        .arg(store::dictionary_path())
                        .spawn();
                })),
            )
            .children(rules.into_iter().map(|(heard, meant)| {
                card().child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().text_color(rgb(MUTED)).child(heard))
                        .child(div().text_color(rgb(MUTED)).child("→"))
                        .child(div().text_color(rgb(TEXT)).child(meant)),
                )
            }))
    }

    fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (devices, chosen, in_use) = {
            let s = self.state.lock().unwrap();
            (s.devices.clone(), s.mic.clone(), s.mic_in_use.clone())
        };
        let mic_row = |id: usize, label: String, value: Option<String>, selected: bool, cx: &mut Context<Self>| {
            let cmds = self.cmds.clone();
            div()
                .id(("mic", id))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0x2a2a2f)))
                .on_click(cx.listener(move |_, _, _, cx| {
                    let _ = cmds.send(Cmd::SetMic(value.clone()));
                    cx.notify();
                }))
                .child(dot(if selected { ACCENT } else { 0x3a3a40 }))
                .child(div().text_sm().text_color(rgb(TEXT)).child(label))
        };
        let mut rows = vec![mic_row(0, "System default".into(), None, chosen.is_none(), cx).into_any_element()];
        for (i, name) in devices.into_iter().enumerate() {
            let selected = chosen.as_deref() == Some(name.as_str());
            rows.push(mic_row(i + 1, name.clone(), Some(name), selected, cx).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(heading("Settings"))
            .child(
                card()
                    .child(div().text_color(rgb(TEXT)).child("Microphone"))
                    .child(muted(format!("In use: {in_use}")))
                    .children(rows),
            )
            .child(
                card()
                    .child(div().text_color(rgb(TEXT)).child("Push to talk"))
                    .child(muted("Hold fn. A Teenage Engineering TING works too (ctrl+opt+F12)."))
                    .child(muted("If fn opens the emoji picker: System Settings → Keyboard → Press 🌐 key to → Do nothing.")),
            )
            .child(
                card()
                    .child(div().text_color(rgb(TEXT)).child("Speech model"))
                    .child(muted("NVIDIA Parakeet TDT 0.6B v3, running on this machine. Nothing is sent anywhere."))
                    .child(div().flex().child(button("data", "Open data folder").on_click(|_, _, _| {
                        let _ = std::process::Command::new("open").arg(store::dir()).spawn();
                    }))),
            )
    }
}

impl Render for Main {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = match self.page {
            Page::Home => self.home(cx).into_any_element(),
            Page::History => self.history(cx).into_any_element(),
            Page::Dictionary => self.dictionary(cx).into_any_element(),
            Page::Settings => self.settings(cx).into_any_element(),
        };
        div()
            .flex()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .w(px(180.))
                    .h_full()
                    .bg(rgb(SIDEBAR))
                    .pt(px(48.))
                    .px_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().px_3().pb_4().text_lg().text_color(rgb(ACCENT)).child("Heyra"))
                    .child(self.nav("nav-home", "Home", Page::Home, cx))
                    .child(self.nav("nav-history", "History", Page::History, cx))
                    .child(self.nav("nav-dict", "Dictionary", Page::Dictionary, cx))
                    .child(self.nav("nav-settings", "Settings", Page::Settings, cx)),
            )
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .h_full()
                    .overflow_y_scroll()
                    .pt(px(44.))
                    .px_6()
                    .pb_6()
                    .child(page),
            )
    }
}

// ---- the floating pill ----

pub struct Pill {
    state: Shared,
    born: Instant,
}

pub fn open_pill(cx: &mut App, state: Shared) -> Option<WindowHandle<Pill>> {
    let pill = size(px(132.), px(36.));
    let display = cx.primary_display()?.bounds();
    let origin = point(
        display.origin.x + (display.size.width - pill.width) / 2.,
        display.origin.y + display.size.height - px(110.),
    );
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: pill })),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |_, cx| cx.new(|_| Pill { state, born: Instant::now() }),
    )
    .ok()
}

impl Render for Pill {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, level) = {
            let s = self.state.lock().unwrap();
            (s.phase, s.level)
        };
        let t = self.born.elapsed().as_secs_f32();
        let bars = (0..9).map(move |i| {
            let wave = ((t * 9.0 + i as f32 * 0.8).sin() * 0.5 + 0.5) * 0.6 + 0.4;
            let h = match phase {
                Phase::Listening => 4.0 + 18.0 * level.max(0.06) * wave,
                _ => 4.0 + 6.0 * (((t * 6.0 - i as f32 * 0.6).sin()) * 0.5 + 0.5),
            };
            let color: Hsla = if phase == Phase::Listening { rgb(TEXT).into() } else { rgb(ACCENT).into() };
            div().w(px(3.)).h(px(h)).rounded_full().bg(color)
        });
        div()
            .size_full()
            .rounded_full()
            .bg(rgb(0x0e0e10))
            .border_1()
            .border_color(rgb(0x2e2e33))
            .flex()
            .items_center()
            .justify_center()
            .gap(px(3.))
            .children(bars)
    }
}
