//! Heyra's window and the edge gauge.
//!
//! Design: a field recorder's faceplate. Graphite ground, cream readouts, gold for
//! anything live, slate for labels. Plex Mono for every number and label, Plex Sans
//! for your words. One segmented level meter, shared by the deck and the edge gauge.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, App, Bounds, Context, Div, FontWeight, Hsla, SharedString, Stateful, Window,
    WindowBounds, WindowOptions, div, point,
    prelude::*, px, rgb, size,
};

use crate::setup::{self, Mic};
use crate::store;
use crate::worker::{Cmd, KEEP_MIC_READY, MUTE_WHILE_TALKING, Phase, Shared};
use std::sync::atomic::Ordering;

// ---- tokens ----
pub const GROUND: u32 = 0x1f2026; // graphite, biased toward the slate
pub const PANEL: u32 = 0x262831;
pub const RAIL: u32 = 0x1a1b20;
pub const HAIR: u32 = 0x343744;
pub const CREAM: u32 = 0xede0c4; // readouts and your words
pub const SLATE: u32 = 0x8a9aa6; // labels
pub const DIM: u32 = 0x5b6470; // off segments, quiet text
pub const GOLD: u32 = 0xd9a86a; // live signal
pub const REC: u32 = 0xe0795a; // recording light

pub const SANS: &str = "IBM Plex Sans";
pub const MONO: &str = "IBM Plex Mono";

pub fn alpha(color: u32, a: f32) -> Hsla {
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
/// A setting that's on or off: a gold dot when on. `set` gets the new value.
fn switch(id: &'static str, text: &'static str, on: bool, set: impl Fn(bool) + 'static) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_3()
        .py_2()
        .cursor_pointer()
        .on_click(move |_, _, _| set(!on))
        .child(
            div()
                .flex_none()
                .size(px(9.))
                .rounded_full()
                .border_1()
                .border_color(rgb(if on { GOLD } else { DIM }))
                .when(on, |d| d.bg(rgb(GOLD))),
        )
        .child(div().text_sm().text_color(rgb(if on { CREAM } else { SLATE })).child(text))
}

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
    show_other_mics: bool,
}

pub fn open_main(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    // Be the active app before the window opens. Opened while another app is active (a
    // click on the dot doesn't activate Heyra), GPUI's window-focus handler deadlocks.
    cx.activate(true);
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
            cx.new(|_| Main { state, cmds, page: Page::Home, copied: None, born: Instant::now(), show_other_mics: false })
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
        let (devices, chosen, in_use, ting_heard, level) = {
            let s = self.state.lock().unwrap();
            (s.devices.clone(), s.mic.clone(), s.mic_in_use.clone(), s.ting_heard, s.level)
        };
        let mac_default = crate::mics::default_name().unwrap_or_else(|| "none".into());
        // Real mics first; the Mac's own choice; then phones and virtual devices, folded away.
        let (real, other): (Vec<_>, Vec<_>) = devices.into_iter().partition(|m| m.kind != crate::mics::Kind::Other);
        let mut options: Vec<(String, Option<&'static str>, Option<String>)> = real
            .into_iter()
            .map(|m| {
                let tag = match m.kind {
                    crate::mics::Kind::BuiltIn => Some("BUILT-IN"),
                    crate::mics::Kind::Bluetooth => Some("BLUETOOTH"),
                    _ if ting_heard && m.name == in_use => Some("FX MIC"),
                    _ => None,
                };
                (m.name.clone(), tag, Some(m.name))
            })
            .collect();
        options.push((format!("Same as Mac's Sound settings ({mac_default})"), None, None));
        let other_count = other.len();
        if self.show_other_mics {
            options.extend(other.into_iter().map(|m| (m.name.clone(), None, Some(m.name))));
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        for (i, (name, tag, value)) in options.into_iter().enumerate() {
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
                            .flex_none()
                            .size(px(9.))
                            .rounded_full()
                            .border_1()
                            .border_color(rgb(if selected { GOLD } else { DIM }))
                            .when(selected, |d| d.bg(rgb(GOLD))),
                    )
                    .child(div().flex_1().text_sm().text_color(rgb(if selected { CREAM } else { SLATE })).child(name))
                    .children(tag.map(|t| div().font_family(MONO).text_xs().text_color(rgb(if t == "FX MIC" { GOLD } else { DIM })).child(t)))
                    .when(selected, |d| d.child(meter(10, ((level * 1.15).min(1.0) * 10.) as usize, true, true)))
                    .into_any_element(),
            );
        }
        if other_count > 0 {
            let show = self.show_other_mics;
            rows.push(
                div()
                    .id("other-mics")
                    .pt_2()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(SLATE))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(CREAM)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_other_mics = !show;
                        cx.notify();
                    }))
                    .child(if show { "HIDE OTHER DEVICES".to_string() } else { format!("OTHER DEVICES ({other_count})") })
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
                    .child(div().flex().flex_col().children(rows))
                    .child(hint("Plug in a mic and Heyra switches to it."))
                    .child(switch("ready", "Keep the mic ready", KEEP_MIC_READY.load(Ordering::Relaxed), |on| {
                        KEEP_MIC_READY.store(on, Ordering::Relaxed);
                        store::save_settings(&store::Settings { keep_mic_ready: on, ..store::load_settings() });
                    }))
                    .child(hint("Off: the mic opens only while you talk, so the orange mic light goes out, but the first moment can be missed. Plugged-in mics, like the FX mic, always stay ready.")),
            )
            .child(
                section("PUSH TO TALK")
                    .child(div().text_sm().text_color(rgb(CREAM)).child("Hold fn, talk, let go."))
                    .child(hint("Double-tap fn for hands-free; tap fn or ✓ to paste, ✕ to keep it in History only."))
                    .child(hint("Esc throws a take away. End with \"press enter\" and Heyra presses it after pasting."))
                    .child({
                        let recording = crate::hotkey::recording();
                        let chosen = crate::hotkey::button();
                        let text = if recording {
                            "Press the key or mouse button to use (esc cancels)…".to_string()
                        } else {
                            chosen.map_or("Another button: none".into(), |b| format!("Another button: {}", crate::hotkey::name(b)))
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .pt_1()
                            .child(div().text_sm().text_color(rgb(if recording { GOLD } else { CREAM })).child(text))
                            .child(button("choose", "CHOOSE").on_click(|_, _, _| crate::hotkey::record(true)))
                            .when(chosen.is_some() && !recording, |d| {
                                d.child(button("clear", "CLEAR").on_click(|_, _, _| {
                                    crate::hotkey::set_button(None);
                                    let settings = store::load_settings();
                                    store::save_settings(&store::Settings { button: None, ..settings });
                                }))
                            })
                    })
                    .child(hint("Any key, key combo or extra mouse button: a foot pedal, a macro pad, a mic's button that types a key."))
                    .child(hint(if ting_heard {
                        "FX mic heard on this microphone: squeeze to talk, bottom button for Enter, middle to undo."
                    } else {
                        "A Teenage Engineering FX mic (TING) works too: choose its line-in as the microphone, then squeeze."
                    }))
                    .child(hint("If fn opens the emoji picker: System Settings → Keyboard → Press 🌐 key to → Do nothing.")),
            )
            .child({
                let (disk, note) = {
                    let s = self.state.lock().unwrap();
                    (s.ting_disk, s.ting_note.clone())
                };
                let line = match disk {
                    None if note.is_some() => None,
                    None => Some("Plug your FX mic in over USB-C to set it up for Heyra. Once per mic."),
                    Some(false) => Some("FX mic plugged in. Setting it up adds a small script and four tones to its disk; its own files are backed up first."),
                    Some(true) => Some("This FX mic is set up. Unplug it, press the button above its USB port, then squeeze."),
                };
                let state = self.state.clone();
                section("FX MIC · TING")
                    .children(line.map(|l| div().text_sm().text_color(rgb(CREAM)).child(l)))
                    .when(disk == Some(false), |d| {
                        d.child(div().flex().pt_1().child(button("ting-setup", "SET UP THIS FX MIC").on_click(move |_, _, _| {
                            let state = state.clone();
                            std::thread::spawn(move || {
                                let note = match crate::ting_setup::disk().map(|d| crate::ting_setup::set_up(&d)) {
                                    Some(Ok(backup)) => format!(
                                        "Done. Unplug it, press the button above its USB port, then squeeze. Its old files: {}",
                                        backup.display()
                                    ),
                                    Some(Err(e)) => format!("Couldn't set it up: {e}"),
                                    None => "The FX mic was unplugged.".into(),
                                };
                                state.lock().unwrap().ting_note = Some(note);
                            });
                        })))
                    })
                    .children(note.map(|n| div().text_sm().text_color(rgb(CREAM)).child(n)))
            })
            .child(
                section("SOUND")
                    .child(switch("sounds", "Soft click when a take starts and ends", crate::sound::enabled(), |on| {
                        crate::sound::set_enabled(on);
                        store::save_settings(&store::Settings { sounds: on, ..store::load_settings() });
                    }))
                    .child(switch("mute", "Mute the Mac's sound while you talk", MUTE_WHILE_TALKING.load(Ordering::Relaxed), |on| {
                        MUTE_WHILE_TALKING.store(on, Ordering::Relaxed);
                        store::save_settings(&store::Settings { mute_while_talking: on, ..store::load_settings() });
                    })),
            )
            .child(section("START").child(switch("login", "Open Heyra at login", crate::login::enabled(), crate::login::set)))
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

// ---- the resting dot's click target ----
//
// The dot is drawn by the orb overlay, which ignores the mouse. This small window
// sits over it while Heyra is at rest so it shows a pointer and takes a click.
// Hover itself is read from the pointer position each frame (see main.rs).

use std::cell::Cell;
use std::rc::Rc;

pub struct Hotspot {
    pub clicked: Rc<Cell<bool>>,
}

const HOTSPOT: f32 = 26.;

pub fn open_hotspot(cx: &mut App, centre: (f64, f64), hotspot: Hotspot) -> Option<gpui::WindowHandle<Hotspot>> {
    open_popup(cx, centre, HOTSPOT, hotspot)
}

/// A small borderless, unfocused window centred on a point of the screen.
fn open_popup<V: Render>(cx: &mut App, centre: (f64, f64), side: f32, view: V) -> Option<gpui::WindowHandle<V>> {
    let origin = point(px(centre.0 as f32 - side / 2.), px(centre.1 as f32 - side / 2.));
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(side), px(side)) })),
            titlebar: None,
            focus: false,
            show: true,
            kind: gpui::WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |_, cx| cx.new(|_| view),
    )
    .ok()
    .inspect(|handle| {
        let _ = handle.update(cx, |_, window, _| no_shadow(window));
    })
}

/// The X and the tick beside the orb during a hands-free take.
pub struct OrbButton {
    pub glyph: &'static str,
    pub clicked: Rc<Cell<bool>>,
}

const ORB_BUTTON: f32 = 22.;

/// Opens a button just off the sphere's upper edge: `side` -1 for left, 1 for right.
pub fn open_orb_button(cx: &mut App, centre: (f64, f64), side: f64, button: OrbButton) -> Option<gpui::WindowHandle<OrbButton>> {
    open_popup(cx, (centre.0 + 33. * side, centre.1 - 33.), ORB_BUTTON, button)
}

impl Render for OrbButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicked = self.clicked.clone();
        div()
            .id(self.glyph)
            .size_full()
            .rounded_full()
            .bg(alpha(GROUND, 0.92))
            .border_1()
            .border_color(rgb(HAIR))
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(CREAM))
            .text_size(px(12.))
            .cursor_pointer()
            .hover(|d| d.text_color(rgb(GOLD)))
            .child(self.glyph)
            .on_click(move |_, _, _| clicked.set(true))
    }
}

/// A short note above the dot: "Using  CUBILUX HLMS-C4 Line IN".
pub struct Toast {
    pub lead: String,
    pub text: String,
}

const TOAST_HEIGHT: f32 = 30.;
/// Room around the pill for its shadow.
const TOAST_MARGIN: f32 = 36.;

/// `above`: how far above the dot's centre the pill sits.
pub fn open_toast(cx: &mut App, centre: (f64, f64), above: f32, toast: Toast) -> Option<gpui::WindowHandle<Toast>> {
    // Generous for Plex Sans at this size; the pill itself sizes to its words.
    let chars = (toast.lead.chars().count() + toast.text.chars().count()) as f32;
    let width = chars * 7.4 + 70. + TOAST_MARGIN * 2.;
    let height = TOAST_HEIGHT + TOAST_MARGIN * 2.;
    let origin = point(px(centre.0 as f32 - width / 2.), px(centre.1 as f32 - above - height / 2.));
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds { origin, size: size(px(width), px(height)) })),
            titlebar: None,
            focus: false,
            show: true,
            kind: gpui::WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            window_background: gpui::WindowBackgroundAppearance::Transparent,
            ..Default::default()
        },
        |_, cx| cx.new(|_| toast),
    )
    .ok()
    .inspect(|handle| {
        let _ = handle.update(cx, |_, window, _| {
            no_shadow(window);
            click_through(window);
        });
    })
}

impl Render for Toast {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The resting dot in miniature: black, with its gold point.
        let dot = div()
            .flex_none()
            .size(px(12.))
            .rounded_full()
            .bg(rgb(0x0c0c0f))
            .border_1()
            .border_color(alpha(CREAM, 0.10))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(3.)).rounded_full().bg(rgb(GOLD)));
        let pill = div()
            .h(px(TOAST_HEIGHT))
            .pl(px(9.))
            .pr(px(14.))
            .rounded_full()
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(alpha(0x30323c, 0.97), 0.),
                gpui::linear_color_stop(alpha(GROUND, 0.97), 1.),
            ))
            .border_1()
            .border_color(alpha(CREAM, 0.07))
            .shadow(vec![gpui::BoxShadow {
                color: alpha(0x000000, 0.32),
                offset: point(px(0.), px(3.)),
                blur_radius: px(10.),
                spread_radius: px(0.),
            }])
            .flex()
            .items_center()
            .gap(px(8.))
            .font_family(SANS)
            .text_size(px(12.5))
            .child(dot)
            .child(div().text_color(rgb(SLATE)).child(self.lead.clone()))
            .when(!self.text.is_empty(), |d| {
                d.child(div().font_weight(gpui::FontWeight::MEDIUM).text_color(rgb(CREAM)).child(self.text.clone()))
            });
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(gpui::AnimationExt::with_animation(pill,
                "toast-in",
                gpui::Animation::new(Duration::from_millis(320)).with_easing(gpui::ease_out_quint()),
                |pill, t| pill.opacity(t).mt(px(8. * (1. - t))),
            ))
    }
}

/// The toast only informs: clicks go to whatever is under it.
#[allow(unexpected_cfgs)]
fn click_through(window: &Window) {
    use objc::runtime::{Object, YES};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        unsafe {
            let view = appkit.ns_view.as_ptr() as *mut Object;
            let native: *mut Object = msg_send![view, window];
            let _: () = msg_send![native, setIgnoresMouseEvents: YES];
        }
    }
}

/// macOS draws a shadow around even a clear window; the dot's target must not have one.
#[allow(unexpected_cfgs)]
fn no_shadow(window: &Window) {
    use objc::runtime::{NO, Object};
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        unsafe {
            let view = appkit.ns_view.as_ptr() as *mut Object;
            let native: *mut Object = msg_send![view, window];
            let _: () = msg_send![native, setHasShadow: NO];
        }
    }
}

impl Render for Hotspot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicked = self.clicked.clone();
        // A barely-there fill: fully clear pixels would let clicks fall through.
        div()
            .id("dot")
            .size_full()
            .rounded_full()
            .bg(alpha(GROUND, 0.012))
            .cursor_pointer()
            .on_click(move |_, _, _| clicked.set(true))
    }
}
