//! Heyra's window and the edge gauge.
//!
//! Design: a field recorder's faceplate. Graphite ground, cream readouts, gold for
//! anything live, slate for labels. Plex Mono for every number and label, Plex Sans
//! for your words. One segmented level meter, shared by the deck and the edge gauge.

use std::sync::mpsc::Sender;
use std::time::Instant;

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, App, Bounds, Context, Div, FontWeight, Hsla, SharedString, Stateful, Window,
    WindowBounds, WindowOptions, div, point,
    prelude::*, px, rgb, size,
};

use crate::setup::{self, Mic};
use crate::store;
use crate::worker::{Cmd, Phase, Shared};

// ---- tokens ----
const GROUND: u32 = 0x1f2026; // graphite, biased toward the slate
const PANEL: u32 = 0x262831;
const RAIL: u32 = 0x1a1b20;
const HAIR: u32 = 0x343744;
const CREAM: u32 = 0xede0c4; // readouts and your words
const SLATE: u32 = 0x8a9aa6; // labels
const DIM: u32 = 0x5b6470; // off segments, quiet text
const GOLD: u32 = 0xd9a86a; // live signal
const REC: u32 = 0xe0795a; // recording light

pub const SANS: &str = "IBM Plex Sans";
pub const MONO: &str = "IBM Plex Mono";

fn alpha(color: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb(color).into();
    c.a = a;
    c
}

/// Small uppercase mono label, the faceplate's lettering.
fn label(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_xs().text_color(rgb(SLATE)).child(text.into())
}

fn readout(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_color(rgb(CREAM)).child(text.into())
}

fn title(text: &'static str) -> Div {
    div()
        .font_family(SANS)
        .text_xl()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(CREAM))
        .child(text)
}

fn hint(text: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(rgb(SLATE)).child(text.into())
}

fn button(id: &'static str, text: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(HAIR))
        .font_family(MONO)
        .text_xs()
        .text_color(rgb(CREAM))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(GOLD)).text_color(rgb(GOLD)))
        .child(text)
}

fn light(phase: Phase) -> Div {
    let color = match phase {
        Phase::Listening => REC,
        Phase::Transcribing | Phase::Loading => GOLD,
        Phase::Ready => SLATE,
        Phase::Error => REC,
    };
    div().flex_none().size(px(7.)).rounded_full().bg(rgb(color))
}

/// The segmented meter: `lit` of `count` segments on, graded from slate to cream to gold.
fn meter(count: usize, lit: usize, live: bool, horizontal: bool) -> Div {
    let segments = (0..count).map(move |i| {
        let on = i < lit;
        let color = if !on {
            alpha(DIM, 0.35)
        } else if i * 10 >= count * 8 {
            rgb(GOLD).into()
        } else if live {
            rgb(CREAM).into()
        } else {
            rgb(SLATE).into()
        };
        let seg = div().rounded(px(1.)).bg(color);
        if horizontal { seg.w(px(5.)).h(px(14.)) } else { seg.w(px(8.)).h(px(3.)) }
    });
    let row = div().flex().gap(px(3.)).children(segments);
    if horizontal { row.flex_row().items_center() } else { row.flex_col_reverse().items_center() }
}

fn clock(at: u64) -> String {
    Local
        .timestamp_opt(at as i64, 0)
        .single()
        .map(|t| {
            if store::now().saturating_sub(at) < 86_400 {
                t.format("%H:%M").to_string()
            } else {
                t.format("%d %b").to_string()
            }
        })
        .unwrap_or_default()
}

fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---- main window ----

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
    born: Instant,
}

pub fn open_main(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    let bounds = Bounds::centered(None, size(px(780.), px(540.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Heyra".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            window_min_size: Some(size(px(600.), px(420.))),
            ..Default::default()
        },
        |_, cx| {
            cx.new(|_| Main { state, cmds, page: Page::Home, copied: None, born: Instant::now() })
        },
    )
    .ok();
}

impl Main {
    fn nav(&self, id: &'static str, text: &'static str, page: Page, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.page == page;
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py(px(7.))
            .cursor_pointer()
            .font_family(MONO)
            .text_xs()
            .text_color(rgb(if active { CREAM } else { SLATE }))
            .hover(|s| s.text_color(rgb(CREAM)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                this.copied = None;
                cx.notify();
            }))
            .child(div().w(px(2.)).h(px(12.)).bg(rgb(if active { GOLD } else { RAIL })))
            .child(text)
    }

    /// One take: number and time, your words, duration.
    fn take(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.lock().unwrap();
        let entry = &s.history[index];
        let text = entry.text.clone();
        let copied = self.copied == Some(index);
        div()
            .id(("take", index))
            .flex()
            .gap_4()
            .py_3()
            .px_2()
            .border_b_1()
            .border_color(rgb(HAIR))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(PANEL)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(text.clone());
                }
                this.copied = Some(index);
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(64.))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(readout(format!("T-{:03}", index + 1)).text_xs())
                    .child(label(clock(entry.at))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family(SANS)
                    .text_sm()
                    .line_height(px(21.))
                    .text_color(rgb(CREAM))
                    .line_clamp(3)
                    .child(entry.text.clone()),
            )
            .child(
                div().flex_none().w(px(64.)).flex().justify_end().child(if copied {
                    label("COPIED").text_color(rgb(GOLD))
                } else {
                    label(format!("{:.1} s", entry.secs))
                }),
            )
            .into_any_element()
    }

    fn takes(&self, indices: impl Iterator<Item = usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        indices.map(|i| self.take(i, cx)).collect()
    }

    fn home(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.lock().unwrap();
        let (phase, message) = match &s.blocker {
            Some(b) => (Phase::Error, b.clone()),
            None => (s.phase, s.message.clone()),
        };
        let words: usize = s.history.iter().map(|e| e.words()).sum();
        let secs: f32 = s.history.iter().map(|e| e.secs).sum();
        let midnight = Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(Local).single())
            .map(|t| t.timestamp() as u64)
            .unwrap_or(0);
        let today: usize = s.history.iter().filter(|e| e.at >= midnight).map(|e| e.words()).sum();
        let pace = if secs > 0.0 { (words as f32 / secs * 60.0).round() as usize } else { 0 };
        let timed: Vec<f32> = s.history.iter().filter(|e| e.took > 0.0).map(|e| e.took).collect();
        let latency = if timed.is_empty() {
            "–".to_string()
        } else {
            format!("{:.2} s", timed.iter().sum::<f32>() / timed.len() as f32)
        };
        let count = s.history.len();
        let level = s.level;
        let progress = s.progress;
        let checks = s.checks;
        let model_ready = matches!(s.phase, Phase::Ready | Phase::Listening | Phase::Transcribing);
        let mic = s.mic_in_use.clone();
        let rate = s.sample_rate;
        drop(s);

        let t = self.born.elapsed().as_secs_f32();
        const SEGMENTS: usize = 32;
        let lit = match phase {
            Phase::Listening => ((level * 1.15).min(1.0) * SEGMENTS as f32) as usize,
            Phase::Transcribing => ((t * 24.0) as usize) % SEGMENTS,
            Phase::Loading => progress.map(|p| (p * SEGMENTS as f32) as usize).unwrap_or(0),
            _ => 0,
        };
        let state_word = match phase {
            Phase::Listening => "REC",
            Phase::Transcribing => "WRITING",
            Phase::Loading => "LOADING",
            Phase::Ready => "READY",
            Phase::Error => "CHECK",
        };

        let deck = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded(px(6.))
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(HAIR))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(light(phase))
                            .child(readout(state_word).text_sm().font_weight(FontWeight::MEDIUM)),
                    )
                    .child(meter(SEGMENTS, lit, phase == Phase::Listening, true)),
            )
            .child(div().min_w_0().font_family(SANS).text_color(rgb(CREAM)).child(message))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .child(label(format!("IN  {mic}")))
                    .child(label(if rate > 0 {
                        format!("{} kHz → 16 kHz", rate / 1000)
                    } else {
                        String::new()
                    }))
                    .child(label("PARAKEET TDT 0.6B · ON THIS MAC")),
            );

        let cell = |name: &'static str, value: String| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .px_4()
                .py_3()
                .child(label(name))
                .child(readout(value).text_lg())
        };
        let strip = div()
            .flex()
            .border_1()
            .border_color(rgb(HAIR))
            .rounded(px(6.))
            .child(cell("TODAY", format!("{} w", grouped(today))))
            .child(div().w(px(1.)).bg(rgb(HAIR)))
            .child(cell("TOTAL", format!("{} w", grouped(words))))
            .child(div().w(px(1.)).bg(rgb(HAIR)))
            .child(cell("PACE", format!("{pace} wpm")))
            .child(div().w(px(1.)).bg(rgb(HAIR)))
            .child(cell("LATENCY", latency));

        // First-run setup: each step ticks itself off as macOS reports it done.
        let step = |id: &'static str, done: bool, name: &'static str, detail: String, action: Option<(&'static str, &'static str)>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_2()
                .border_b_1()
                .border_color(rgb(HAIR))
                .child(
                    div()
                        .flex_none()
                        .w(px(36.))
                        .child(label(if done { "OK" } else { "··" }).text_color(rgb(if done { GOLD } else { DIM }))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().text_color(rgb(if done { SLATE } else { CREAM })).child(name))
                        .when(!done, |d| d.child(hint(detail))),
                )
                .when_some(action.filter(|_| !done), |d, (text, pane)| {
                    d.child(button(id, text).on_click(move |_, _, _| setup::open_settings(pane)))
                })
        };
        let model_detail = match progress {
            Some(p) => format!("Downloading once, {:.0}% done. After this Heyra never goes online.", p * 100.0),
            None => "Loading…".to_string(),
        };
        let steps = [
            (model_ready, "Speech model on this Mac"),
            (checks.mic == Mic::Allowed, "Microphone"),
            (checks.accessibility, "Accessibility, to hear fn and type for you"),
            (checks.fn_free, "fn key free for Heyra"),
            (count > 0, "First take"),
        ];
        let done = steps.iter().filter(|(d, _)| *d).count();
        let setup_list = div()
            .flex()
            .flex_col()
            .child(label(format!("SETUP · {done} OF {} DONE", steps.len())).pb_2())
            .child(step("s-model", model_ready, steps[0].1, model_detail, None))
            .child(step(
                "s-mic",
                steps[1].0,
                steps[1].1,
                if checks.mic == Mic::Denied {
                    "Heyra was refused the microphone. Turn it on in Settings.".into()
                } else {
                    "macOS asks the first time you hold fn.".into()
                },
                (checks.mic == Mic::Denied).then_some(("OPEN SETTINGS", setup::PANE_MICROPHONE)),
            ))
            .child(step(
                "s-ax",
                steps[2].0,
                steps[2].1,
                "Turn on Heyra in Privacy & Security → Accessibility.".into(),
                Some(("OPEN SETTINGS", setup::PANE_ACCESSIBILITY)),
            ))
            .child(step(
                "s-fn",
                steps[3].0,
                steps[3].1,
                "Keyboard → Press 🌐 key to → Do nothing, so fn doesn't also open emoji or dictation.".into(),
                Some(("KEYBOARD", setup::PANE_KEYBOARD)),
            ))
            .child(step("s-take", steps[4].0, steps[4].1, "Click into any text box, hold fn, say a sentence, let go.".into(), None));
        let setup_done = done == steps.len();

        let recent: Vec<usize> = (0..count).rev().take(6).collect();
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(title("Home"))
            .child(deck)
            .when(!setup_done, |d| d.child(setup_list))
            .child(strip)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label(if recent.is_empty() { "NO TAKES YET" } else { "RECENT TAKES" }).pb_2())
                    .when(recent.is_empty(), |d| {
                        d.child(hint("Hold fn in any app and talk. Let go and your words are typed where the cursor is."))
                    })
                    .children(self.takes(recent.into_iter(), cx)),
            )
    }

    fn history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().unwrap().history.len();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(title("History"))
            .child(hint("Every take, kept on this Mac only. Click one to copy it."))
            .child(div().flex().flex_col().children(self.takes((0..count).rev(), cx)))
    }

    fn dictionary(&self) -> impl IntoElement {
        let rules = store::load_dictionary();
        let empty = rules.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(title("Dictionary"))
            .child(hint("Words Heyra mishears, fixed every time. One rule per line: heard => meant."))
            .child(div().flex().child(button("edit-dict", "EDIT DICTIONARY").on_click(|_, _, _| {
                let _ = std::process::Command::new("open").arg("-t").arg(store::dictionary_path()).spawn();
            })))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(empty, |d| d.child(hint("No rules yet. Example: a cappy bar => capybara")))
                    .children(rules.into_iter().map(|(heard, meant)| {
                        div()
                            .flex()
                            .gap_3()
                            .py_2()
                            .border_b_1()
                            .border_color(rgb(HAIR))
                            .child(div().w(px(220.)).font_family(MONO).text_sm().text_color(rgb(SLATE)).child(heard))
                            .child(div().font_family(MONO).text_sm().text_color(rgb(DIM)).child("→"))
                            .child(div().font_family(MONO).text_sm().text_color(rgb(CREAM)).child(meant))
                    })),
            )
    }

    fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (devices, chosen, in_use) = {
            let s = self.state.lock().unwrap();
            (s.devices.clone(), s.mic.clone(), s.mic_in_use.clone())
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        let options = std::iter::once((String::from("System default"), None))
            .chain(devices.into_iter().map(|d| (d.clone(), Some(d))));
        for (i, (name, value)) in options.enumerate() {
            let selected = chosen == value;
            let cmds = self.cmds.clone();
            rows.push(
                div()
                    .id(("mic", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(HAIR))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(PANEL)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = cmds.send(Cmd::SetMic(value.clone()));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .size(px(9.))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(if selected { GOLD } else { DIM }))
                            .when(selected, |d| d.bg(rgb(GOLD))),
                    )
                    .child(div().text_sm().text_color(rgb(CREAM)).child(name))
                    .into_any_element(),
            );
        }
        let section = |name: &'static str| div().flex().flex_col().gap_2().child(label(name));
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(title("Settings"))
            .child(
                section("MICROPHONE")
                    .child(hint(format!("In use: {in_use}")))
                    .child(div().flex().flex_col().children(rows)),
            )
            .child(
                section("PUSH TO TALK")
                    .child(div().text_sm().text_color(rgb(CREAM)).child("Hold fn, talk, let go."))
                    .child(hint("A Teenage Engineering TING works too: its squeeze sends ctrl+opt+F12."))
                    .child(hint("If fn opens the emoji picker: System Settings → Keyboard → Press 🌐 key to → Do nothing.")),
            )
            .child(
                section("SPEECH MODEL")
                    .child(div().text_sm().text_color(rgb(CREAM)).child("NVIDIA Parakeet TDT 0.6B v3, running on this Mac."))
                    .child(hint("Your voice and your words never leave this machine."))
                    .child(div().flex().pt_1().child(button("data", "OPEN DATA FOLDER").on_click(|_, _, _| {
                        let _ = std::process::Command::new("open").arg(store::dir()).spawn();
                    }))),
            )
    }
}

impl Render for Main {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, mic) = {
            let s = self.state.lock().unwrap();
            (if s.blocker.is_some() { Phase::Error } else { s.phase }, s.mic_in_use.clone())
        };
        let page = match self.page {
            Page::Home => self.home(cx).into_any_element(),
            Page::History => self.history(cx).into_any_element(),
            Page::Dictionary => self.dictionary().into_any_element(),
            Page::Settings => self.settings(cx).into_any_element(),
        };
        let rail = div()
            .w(px(172.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .justify_between()
            .bg(rgb(RAIL))
            .border_r_1()
            .border_color(rgb(HAIR))
            .pt(px(52.))
            .pb_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px_4()
                            .pb_5()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(readout("HEYRA").text_sm().font_weight(FontWeight::MEDIUM).text_color(rgb(GOLD)))
                            .child(label("local dictation")),
                    )
                    .child(self.nav("nav-home", "HOME", Page::Home, cx))
                    .child(self.nav("nav-history", "HISTORY", Page::History, cx))
                    .child(self.nav("nav-dict", "DICTIONARY", Page::Dictionary, cx))
                    .child(self.nav("nav-settings", "SETTINGS", Page::Settings, cx)),
            )
            .child(
                div()
                    .px_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().flex().items_center().gap_2().child(light(phase)).child(label(match phase {
                        Phase::Listening => "REC",
                        Phase::Transcribing => "WRITING",
                        Phase::Loading => "LOADING",
                        Phase::Ready => "READY · FN",
                        Phase::Error => "CHECK HOME",
                    })))
                    .child(label(mic).text_color(rgb(DIM)).truncate()),
            );
        div()
            .flex()
            .size_full()
            .bg(rgb(GROUND))
            .font_family(SANS)
            .text_color(rgb(CREAM))
            .child(rail)
            .child(
                div()
                    .id("page")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .pt(px(44.))
                    .px_8()
                    .pb_8()
                    .child(div().w_full().min_w_0().max_w(px(720.)).child(page)),
            )
    }
}
