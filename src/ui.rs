//! Heyra's window and the edge gauge.
//!
//! Design: a field recorder's faceplate. Graphite ground, cream readouts, gold for
//! anything live, slate for labels. The mono for every number and label, the sans
//! for your words. One segmented level meter, shared by the deck and the edge gauge.

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, App, Bounds, Context, Div, ElementId, FontWeight, Hsla, SharedString, Stateful, Window,
    WindowBounds, WindowOptions, div, point,
    prelude::*, px, rgb, size,
};

use crate::setup::{self, Mic};
use crate::model::{self, Choice};

/// Folder picks so far: a check that finishes after a later pick is ignored.
static FOLDER_PICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
use crate::store;
use crate::worker::{Cmd, KEEP_MIC_READY, MUTE_WHILE_TALKING, Phase, SHOW_WORDS, Shared};
use std::sync::atomic::Ordering;

// ---- tokens ----
pub const GROUND: u32 = 0x1f2026; // graphite, biased toward the slate
pub const PANEL: u32 = 0x262831;
pub const RAIL: u32 = 0x1a1b20;
pub const HAIR: u32 = 0x343744;
pub const CREAM: u32 = 0xebebed; // readouts and your words: a neutral off-white
pub const SLATE: u32 = 0x8a9aa6; // labels
pub const DIM: u32 = 0x5b6470; // off segments, quiet text
pub const GOLD: u32 = 0xd9a86a; // live signal
pub const REC: u32 = 0xe0795a; // recording light

pub const SANS: &str = "Space Grotesk";
pub const MONO: &str = "Martian Mono";
/// Titles and the wordmark.
pub const DISPLAY: &str = "Outland";


/// The main window's colours. The floating pieces (the dot, the words, the cards) keep
/// the dark set: they sit over any app.
pub struct Palette {
    pub ground: u32,
    pub panel: u32,
    pub rail: u32,
    pub hair: u32,
    /// Your words and the readouts.
    pub ink: u32,
    /// Labels.
    pub slate: u32,
    /// Quiet text, off segments.
    pub dim: u32,
    /// Anything live.
    pub gold: u32,
    pub rec: u32,
}

pub const DARK: Palette = Palette { ground: GROUND, panel: PANEL, rail: RAIL, hair: HAIR, ink: CREAM, slate: SLATE, dim: DIM, gold: GOLD, rec: REC };

/// White and true greys; the gold deepened to read on white.
pub const LIGHT: Palette = Palette {
    ground: 0xffffff,
    panel: 0xf7f7f8,
    rail: 0xf1f1f3,
    hair: 0xe3e3e7,
    ink: 0x18181b,
    slate: 0x5f6670,
    dim: 0xa1a1aa,
    gold: 0xa8701f,
    rec: 0xc4573a,
};

/// Settings → Appearance: 0 dark, 1 light, 2 match the Mac.
pub static APPEARANCE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// The Mac is in light mode; looked at once a second by the main window.
static MAC_LIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn is_light() -> bool {
    match APPEARANCE.load(Ordering::Relaxed) {
        1 => true,
        2 => MAC_LIGHT.load(Ordering::Relaxed),
        _ => false,
    }
}

/// The palette in use.
pub fn p() -> &'static Palette {
    if is_light() { &LIGHT } else { &DARK }
}

/// Is the Mac set to light mode? Main thread only.
#[allow(unexpected_cfgs)]
fn mac_is_light() -> bool {
    use cocoa::base::{id, nil};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let app: id = msg_send![class!(NSApplication), sharedApplication];
        let appearance: id = msg_send![app, effectiveAppearance];
        if appearance == nil {
            return false;
        }
        let name: id = msg_send![appearance, name];
        let utf8: *const std::ffi::c_char = msg_send![name, UTF8String];
        !std::ffi::CStr::from_ptr(utf8).to_string_lossy().contains("Dark")
    }
}

pub fn alpha(color: u32, a: f32) -> Hsla {
    let mut c: Hsla = rgb(color).into();
    c.a = a;
    c
}

/// The h: two tall cream bars either side of a short gold one, `height` tall.
fn mark(height: f32) -> Div {
    // Square: three bars and two gaps, each a fifth of the height.
    let bar = |h: f32, color: u32| div().w(px(height * 0.2)).h(px(h)).rounded_full().bg(rgb(color));
    div()
        .size(px(height))
        .flex()
        .items_center()
        .gap(px(height * 0.2))
        .child(bar(height, p().ink))
        .child(bar(height * 0.4, p().gold))
        .child(bar(height, p().ink))
}

/// A tooltip: a solid pill in the opposite of the page's colours, so it reads on anything.
fn tip_pill(text: SharedString) -> impl IntoElement {
    let pill = div()
        .px(px(9.))
        .py(px(5.))
        .rounded(px(6.))
        .bg(rgb(p().ink))
        .shadow(vec![gpui::BoxShadow {
            color: alpha(0x000000, 0.28),
            offset: point(px(0.), px(3.)),
            blur_radius: px(10.),
            spread_radius: px(0.),
        }])
        .font_family(MONO)
        .text_size(px(11.))
        .line_height(px(14.))
        .text_color(rgb(p().ground))
        .whitespace_nowrap()
        .child(text.clone());
    gpui::AnimationExt::with_animation(
        pill,
        ElementId::Name(text),
        gpui::Animation::new(Duration::from_millis(110)).with_easing(gpui::ease_out_quint()),
        |pill, t| pill.opacity(t),
    )
}

/// "DICTIONARY" as "Dictionary".
fn title_case(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// Where a tooltip sits against what it describes.
#[derive(Clone, Copy)]
enum Side {
    Right,
    Above,
}

/// The tooltip as an overlay on the top layer, so no edge clips it: directly right of
/// its target, centred on it, or centred above it.
fn tip_overlay(text: SharedString, side: Side) -> Div {
    // A pill is 24 tall; its width, near enough, for centring above.
    let width = text.chars().count() as f32 * 7.4 + 18.;
    let pill = gpui::deferred(gpui::anchored().child(match side {
        Side::Right => div().ml(px(10.)).mt(px(-1.)).child(tip_pill(text)),
        Side::Above => div().ml(px(-width / 2.)).mt(px(-32.)).child(tip_pill(text)),
    }))
    .with_priority(1);
    match side {
        Side::Right => div().absolute().left_full().top_0().h_full().flex().items_center().child(pill),
        Side::Above => div().absolute().bottom_full().left_0().w_full().flex().justify_center().child(pill),
    }
}

/// Small uppercase mono label, the faceplate's lettering.
fn label(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_size(px(10.5)).text_color(rgb(p().slate)).child(text.into())
}

fn readout(text: impl Into<SharedString>) -> Div {
    div().font_family(MONO).text_color(rgb(p().ink)).child(text.into())
}

/// The display face set wider: GPUI has no letter-spacing, so a thin space between letters.
fn tracked(text: &str) -> String {
    let mut out = String::new();
    for (i, c) in text.chars().enumerate() {
        if i > 0 {
            out.push_str("\u{2009}\u{200A}");
        }
        out.push(c);
    }
    out
}

fn title(text: &'static str) -> Div {
    div()
        .font_family(DISPLAY)
        .text_size(px(28.))
        .line_height(px(34.))
        .font_weight(FontWeight::BOLD)
        .text_color(rgb(p().gold))
        .child(tracked(text))
}

fn hint(text: impl Into<SharedString>) -> Div {
    div().text_size(px(13.)).line_height(px(20.)).text_color(rgb(p().slate)).child(text.into())
}

fn button(id: impl Into<gpui::ElementId>, text: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px_4()
        .py(px(7.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(p().hair))
        .font_family(MONO)
        .text_xs()
        .text_color(rgb(p().ink))
        .cursor_pointer()
        .hover(|s| s.border_color(rgb(p().gold)).text_color(rgb(p().gold)))
        .child(text)
}

fn light(phase: Phase) -> Div {
    let color = match phase {
        Phase::Listening => p().rec,
        Phase::Transcribing | Phase::Loading => p().gold,
        Phase::Ready => p().slate,
        Phase::Error => p().rec,
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
        .px_3()
        .py(px(10.))
        .cursor_pointer()
        .on_click(move |_, _, _| set(!on))
        .child(
            div()
                .flex_none()
                .size(px(9.))
                .rounded_full()
                .border_1()
                .border_color(rgb(if on { p().gold } else { p().dim }))
                .when(on, |d| d.bg(rgb(p().gold))),
        )
        .child(div().text_size(px(15.)).text_color(rgb(if on { p().ink } else { p().slate })).child(text))
}

fn meter(count: usize, lit: usize, live: bool, horizontal: bool) -> Div {
    let segments = (0..count).map(move |i| {
        let on = i < lit;
        let color = if !on {
            alpha(p().dim, 0.35)
        } else if i * 10 >= count * 8 {
            rgb(p().gold).into()
        } else if live {
            rgb(p().ink).into()
        } else {
            rgb(p().slate).into()
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

const RAIL_WIDE: f32 = 212.;
const RAIL_NARROW: f32 = 66.;

/// The icons, drawn for Heyra, built into the app.
pub struct Icons;

impl gpui::AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "icons/home.svg" => include_bytes!("../assets/icons/home.svg"),
            "icons/history.svg" => include_bytes!("../assets/icons/history.svg"),
            "icons/dictionary.svg" => include_bytes!("../assets/icons/dictionary.svg"),
            "icons/settings.svg" => include_bytes!("../assets/icons/settings.svg"),
            "icons/sidebar.svg" => include_bytes!("../assets/icons/sidebar.svg"),
            _ => return Ok(None),
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// One take: number and time, your words, duration. A click copies it.
fn take(index: usize, entry: &store::Entry, copied: bool, main: gpui::WeakEntity<Main>) -> AnyElement {
    let text = entry.text.clone();
    div()
        .id(("take", index))
        .flex()
        .gap_6()
        .py_5()
        .px_3()
        .border_b_1()
        .border_color(rgb(p().hair))
        .cursor_pointer()
        .hover(|s| s.bg(rgb(p().panel)))
        .on_click(move |_, _, cx| {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(text.clone());
            }
            let _ = main.update(cx, |main, cx| {
                main.copied = Some(index);
                cx.notify();
            });
        })
        .child(
            div()
                .flex_none()
                .w(px(64.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(label(clock(entry.at))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .font_family(SANS)
                .text_size(px(15.))
                .line_height(px(23.))
                .text_color(rgb(p().ink))
                .line_clamp(3)
                .child(entry.text.clone()),
        )
        .child(
            div().flex_none().w(px(64.)).flex().justify_end().child(if copied {
                label("COPIED").text_color(rgb(p().gold))
            } else {
                label(format!("{:.1} s", entry.secs))
            }),
        )
        .into_any_element()
}

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
    /// What the window last drew from: it redraws only when this changes.
    seen: u64,
    ticks: u32,
    slow: Slow,
    fetching: bool,
    /// The sidebar shows labels (or just icons), and since when, for the slide.
    rail_open: bool,
    rail_since: Instant,
    /// What the pointer is on, for its tooltip.
    hovered: Option<SharedString>,
    /// The width the latest slide started from.
    rail_from: f32,
    /// History, newest first: only the rows on screen are laid out.
    takes: gpui::ListState,
}

/// Answers that cost a system call or a disk read: fetched once a second and after
/// a click, never per frame, so scrolling stays smooth.
#[derive(Default, PartialEq)]
struct Slow {
    mac_default: String,
    login: bool,
    here: Vec<bool>,
    dictionary: Vec<(String, String)>,
}

impl Slow {
    fn fetch() -> Slow {
        Slow {
            mac_default: crate::mics::default_name().unwrap_or_else(|| "none".into()),
            login: crate::login::enabled(),
            here: model::ALL.iter().map(|m| m.find().is_some()).collect(),
            dictionary: store::load_dictionary(),
        }
    }
}

/// The page the next main window opens on, set by `open_main_at`.
static OPEN_ON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Open the main window on Settings (to watch a model download, say).
pub fn open_settings(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    OPEN_ON.store(3, Ordering::Relaxed);
    match cx.windows().into_iter().find_map(|w| w.downcast::<Main>()) {
        Some(w) => {
            let _ = w.update(cx, |main, window, cx| {
                main.page = Page::Settings;
                window.activate_window();
                cx.notify();
            });
            cx.activate(true);
        }
        None => open_main(cx, state, cmds),
    }
}

pub fn open_main(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    // Be the active app before the window opens. Opened while another app is active (a
    // click on the dot doesn't activate Heyra), GPUI's window-focus handler deadlocks.
    cx.activate(true);
    // Roomy by default, as big as Wispr Flow's, but never more than 90% of the screen.
    let (mut w, mut h) = (1320., 880.);
    if let Some(screen) = cx.primary_display().map(|d| d.bounds().size) {
        w = f32::min(w, f32::from(screen.width) * 0.9);
        h = f32::min(h, f32::from(screen.height) * 0.9);
    }
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Heyra".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(16.), px(16.))),
            }),
            window_min_size: Some(size(px(760.), px(520.))),
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| {
                cx.spawn(async move |this, cx| loop {
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                    if this.update(cx, |main: &mut Main, cx| main.poll(cx)).is_err() {
                        break;
                    }
                })
                .detach();
                Main {
                    state,
                    cmds,
                    // HEYRA_PAGE=settings (or history, dictionary) opens on that page: for screenshots.
                    page: match (OPEN_ON.swap(0, Ordering::Relaxed), std::env::var("HEYRA_PAGE").as_deref()) {
                        (3, _) | (_, Ok("settings")) => Page::Settings,
                        (_, Ok("history")) => Page::History,
                        (_, Ok("dictionary")) => Page::Dictionary,
                        _ => Page::Home,
                    },
                    copied: None,
                    born: Instant::now(),
                    show_other_mics: false,
                    seen: 0,
                    ticks: 59,
                    // Filled in by the first poll, off this thread.
                    slow: Slow::default(),
                    fetching: false,
                    rail_open: !store::load_settings().sidebar_narrow,
                    rail_since: Instant::now() - Duration::from_secs(2),
                    rail_from: RAIL_WIDE,
                    // HEYRA_TIP=nav-dict (any tooltip's key) shows it as if hovered: for screenshots.
                    hovered: std::env::var("HEYRA_TIP").ok().map(Into::into),
                    takes: gpui::ListState::new(0, gpui::ListAlignment::Top, px(600.)),
                }
            })
        },
    )
    .ok();
}

impl Main {
    /// The sidebar's width now, and whether it's still moving: a damped spring (a
    /// little overshoot, then settled) from where the last slide began.
    fn rail_width(&self) -> (f32, bool) {
        const STIFFNESS: f32 = 260.0; // ω² for ω ≈ 16 rad/s
        const DAMPING: f32 = 0.72; // ζ: under 1, so it overshoots a touch
        let to = if self.rail_open { RAIL_WIDE } else { RAIL_NARROW };
        let t = self.rail_since.elapsed().as_secs_f32();
        if t > 0.9 {
            return (to, false);
        }
        let w = STIFFNESS.sqrt();
        let wd = w * (1.0 - DAMPING * DAMPING).sqrt();
        let progress = 1.0 - (-DAMPING * w * t).exp() * ((wd * t).cos() + DAMPING * w / wd * (wd * t).sin());
        (self.rail_from + (to - self.rail_from) * progress, true)
    }

    /// Give `el` a tooltip that shows the moment it's hovered.
    fn with_tip(&self, el: Stateful<Div>, key: impl Into<SharedString>, text: impl Into<SharedString>, side: Side, cx: &Context<Self>) -> Stateful<Div> {
        let key: SharedString = key.into();
        let on = self.hovered.as_ref() == Some(&key);
        el.relative()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered = Some(key.clone());
                } else if this.hovered.as_ref() == Some(&key) {
                    this.hovered = None;
                }
                cx.notify();
            }))
            .when(on, |d| d.child(tip_overlay(text.into(), side)))
    }

    /// Each frame: redraw only if something shown has changed.
    fn poll(&mut self, cx: &mut Context<Self>) {
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(60) {
            self.refetch(cx);
            MAC_LIGHT.store(mac_is_light(), Ordering::Relaxed);
        }
        let seen = self.signature();
        if seen != self.seen {
            self.seen = seen;
            cx.notify();
        }
    }

    /// Fetch the slow answers on a background thread (asking launchd about open at login
    /// alone takes ~65 ms), and redraw if they changed. One fetch at a time.
    fn refetch(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.fetching, true) {
            return;
        }
        cx.spawn(async move |this, cx| {
            let slow = cx.background_executor().spawn(async { Slow::fetch() }).await;
            let _ = this.update(cx, |main: &mut Main, cx| {
                main.fetching = false;
                if slow != main.slow {
                    main.slow = slow;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A hash of everything the pages draw from shared state, coarse where the eye is.
    fn signature(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::mem::discriminant;
        let mut h = std::hash::DefaultHasher::new();
        {
            let s = self.state.lock().unwrap();
            discriminant(&s.phase).hash(&mut h);
            (&s.message, &s.blocker, s.history.len()).hash(&mut h);
            (((s.level * 1.15).min(1.0) * 32.0) as u32).hash(&mut h);
            s.devices.iter().for_each(|d| d.name.hash(&mut h));
            (&s.mic, &s.mic_in_use, s.sample_rate).hash(&mut h);
            s.progress.map(|p| (p * 1000.0) as u32).hash(&mut h);
            (discriminant(&s.checks.mic), s.checks.accessibility, s.checks.fn_free).hash(&mut h);
            (s.ting_heard, s.ting_disk, &s.ting_note).hash(&mut h);
            (&s.model, &s.switching, &s.model_error).hash(&mut h);
            s.retry.as_ref().map(|(why, at)| (why, at.saturating_duration_since(Instant::now()).as_secs())).hash(&mut h);
            if s.phase == Phase::Transcribing {
                ((self.born.elapsed().as_secs_f32() * 24.0) as u32).hash(&mut h);
            }
        }
        (crate::hotkey::recording(), crate::hotkey::button().map(crate::hotkey::name)).hash(&mut h);
        (crate::sound::enabled(), KEEP_MIC_READY.load(Ordering::Relaxed), MUTE_WHILE_TALKING.load(Ordering::Relaxed), SHOW_WORDS.load(Ordering::Relaxed), is_light()).hash(&mut h);
        h.finish()
    }

    fn nav(&self, id: &'static str, icon: &'static str, text: &'static str, page: Page, shown: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.page == page;
        let color = if active { p().ink } else { p().slate };
        let item = div().id(id);
        let item = if shown < 0.5 { self.with_tip(item, id, title_case(text), Side::Right, cx) } else { item };
        item
            .flex()
            .items_center()
            .gap_3()
            .mx_3()
            .px_3()
            .py(px(9.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(active, |d| d.bg(alpha(p().ink, 0.06)))
            .hover(|s| s.bg(alpha(p().ink, 0.04)))
            .font_family(MONO)
            .text_size(px(12.5))
            .text_color(rgb(color))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                this.copied = None;
                cx.notify();
            }))
            .child(gpui::svg().path(icon).size(px(17.)).flex_none().text_color(rgb(if active { p().gold } else { color })))
            .child(div().whitespace_nowrap().opacity(shown).child(text))
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
        // Against typing at 40 words a minute, less the time spent talking.
        let saved_mins = (words as f32 / 40.0 - secs / 60.0).max(0.0) as u32;
        let saved = if saved_mins >= 60 { format!("{} h {:02} m", saved_mins / 60, saved_mins % 60) } else { format!("{saved_mins} min") };
        // Days in a row with a take, up to today (or yesterday, if none yet today).
        let days: std::collections::BTreeSet<chrono::NaiveDate> = s
            .history
            .iter()
            .filter_map(|e| Local.timestamp_opt(e.at as i64, 0).single().map(|t| t.date_naive()))
            .collect();
        let mut day = Local::now().date_naive();
        if !days.contains(&day) {
            day = day.pred_opt().unwrap_or(day);
        }
        let mut streak = 0;
        while days.contains(&day) {
            streak += 1;
            day = day.pred_opt().unwrap_or(day);
            if streak > 3650 {
                break;
            }
        }
        // Words a day for the last fortnight, today last.
        let today_date = Local::now().date_naive();
        let mut daily = [0usize; 14];
        for e in &s.history {
            if let Some(d) = Local.timestamp_opt(e.at as i64, 0).single().map(|t| t.date_naive()) {
                let ago = (today_date - d).num_days();
                if (0..14).contains(&ago) {
                    daily[13 - ago as usize] += e.words();
                }
            }
        }
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
            Phase::Listening => "p().rec",
            Phase::Transcribing => "WRITING",
            Phase::Loading => "LOADING",
            Phase::Ready => "READY",
            Phase::Error => "CHECK",
        };

        let deck = div()
            .flex()
            .flex_col()
            .gap_5()
            .p_8()
            .rounded(px(14.))
            .bg(rgb(p().panel))
            .border_1()
            .border_color(rgb(p().hair))
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
            .child(div().min_w_0().font_family(SANS).text_size(px(17.)).line_height(px(25.)).text_color(rgb(p().ink)).child(message))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .child(label(format!(
                        "{} · {} · {}",
                        short(&mic, 28).to_uppercase(),
                        self.state.lock().unwrap().model.name().to_uppercase(),
                        latency.to_uppercase()
                    ))),
            );

        let card = || div().flex().flex_col().p_7().rounded(px(14.)).border_1().border_color(rgb(p().hair));
        let big = |text: String| readout(text).text_size(px(30.)).line_height(px(36.));
        // Words: today big, a fortnight of bars, today's in gold. Each bar says its day.
        let most = daily.iter().copied().max().unwrap_or(0).max(1) as f32;
        let bars = div().flex().items_end().gap(px(6.)).h(px(110.)).children(daily.iter().enumerate().map(|(i, &n)| {
            let date = today_date - chrono::Duration::days(13 - i as i64);
            let height = if n == 0 { 3. } else { (n as f32 / most * 104.).max(6.) };
            let color: Hsla = if i == 13 { rgb(p().gold).into() } else { alpha(p().ink, 0.16) };
            let bar = div().id(("day", i)).flex_1().h_full().flex().items_end();
            self.with_tip(bar, format!("day-{i}"), format!("{} · {} words", date.format("%a %-d %b"), grouped(n)), Side::Above, cx)
                .child(div().w_full().h(px(height)).rounded_t(px(4.)).bg(color))
        }));
        let words_card = card()
            .gap_6()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_end()
                    .child(div().flex().flex_col().gap_2().child(label("TODAY")).child(big(format!("{} words", grouped(today)))))
                    .child(label(format!("{} ALL TIME", grouped(words))).text_color(rgb(p().dim))),
            )
            .child(bars);
        // Streak: the last seven days, lit where you dictated.
        let week = div().flex().gap(px(7.)).children((7..14).map(|i| {
            let date = today_date - chrono::Duration::days(13 - i as i64);
            let lit = daily[i] > 0;
            let dot = div().id(("week", i)).size(px(12.)).rounded_full();
            self.with_tip(dot, format!("week-{i}"), date.format("%a %-d %b").to_string(), Side::Above, cx)
                .when(lit, |d| d.bg(rgb(p().gold)))
                .when(!lit, |d| d.border_1().border_color(rgb(p().hair)))
        }));
        // Pace against typing: two bars on one scale.
        let fastest = pace.max(40).max(1) as f32;
        let pace_bar = |name: &'static str, wpm: usize, color: Hsla| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(label(name).w(px(52.)))
                .child(div().flex_1().h(px(6.)).child(div().h_full().w(gpui::relative(wpm as f32 / fastest)).rounded_full().bg(color)))
        };
        let small = div()
            .flex()
            .gap_4()
            .child(card().flex_1().gap_2().child(label("TIME SAVED")).child(big(saved)))
            .child(
                card()
                    .flex_1()
                    .gap_2()
                    .child(label("STREAK"))
                    .child(big(format!("{streak} {}", if streak == 1 { "day" } else { "days" })))
                    .child(div().pt_2().child(week)),
            )
            .child(
                card()
                    .flex_1()
                    .gap_2()
                    .child(label("PACE"))
                    .child(big(format!("{pace} wpm")))
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(pace_bar("YOU", pace, rgb(p().gold).into()))
                            .child(pace_bar("TYPING", 40, alpha(p().ink, 0.16))),
                    ),
            );
        let strip = div().flex().flex_col().gap_4().child(words_card).child(small);

        // First-run setup: each step ticks itself off as macOS reports it done.
        let step = |id: &'static str, done: bool, name: &'static str, detail: String, action: Option<(&'static str, &'static str)>| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .py_3()
                .border_b_1()
                .border_color(rgb(p().hair))
                .child(
                    div()
                        .flex_none()
                        .w(px(36.))
                        .child(label(if done { "OK" } else { "··" }).text_color(rgb(if done { p().gold } else { p().dim }))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().text_size(px(15.)).text_color(rgb(if done { p().slate } else { p().ink })).child(name))
                        .when(!done, |d| d.child(hint(detail))),
                )
                .when_some(action.filter(|_| !done), |d, (text, pane)| {
                    d.child(button(id, text).on_click(move |_, _, _| setup::open_settings(pane)))
                })
        };
        let model_detail = match progress {
            Some(p) => format!("Downloading, {:.0}%", p * 100.0),
            None => "Loading…".to_string(),
        };
        let steps = [
            (model_ready, "Speech model on this Mac"),
            (checks.mic == Mic::Allowed, "Microphone"),
            (checks.accessibility, "Accessibility"),
            (checks.fn_free, "fn key"),
            (count > 0, "First take"),
        ];
        let done = steps.iter().filter(|(d, _)| *d).count();
        let setup_list = div()
            .flex()
            .flex_col()
            .child(label(format!("SETUP · {done} OF {} DONE", steps.len())).pb_3())
            .child(step("s-model", model_ready, steps[0].1, model_detail, None))
            .child(step(
                "s-mic",
                steps[1].0,
                steps[1].1,
                if checks.mic == Mic::Denied {
                    "Turned off. Allow it in Settings.".into()
                } else {
                    "Asked the first time you hold fn.".into()
                },
                (checks.mic == Mic::Denied).then_some(("OPEN SETTINGS", setup::PANE_MICROPHONE)),
            ))
            .child(step(
                "s-ax",
                steps[2].0,
                steps[2].1,
                "Turn on Heyra in Privacy & Security.".into(),
                Some(("OPEN SETTINGS", setup::PANE_ACCESSIBILITY)),
            ))
            .child(step(
                "s-fn",
                steps[3].0,
                steps[3].1,
                "Set Press 🌐 key to Do nothing.".into(),
                Some(("KEYBOARD", setup::PANE_KEYBOARD)),
            ))
            .child(step("s-take", steps[4].0, steps[4].1, "Hold fn in any text box and talk.".into(), None));
        let setup_done = done == steps.len();

        let me = cx.entity().downgrade();
        let recent: Vec<AnyElement> = {
            let s = self.state.lock().unwrap();
            (0..count).rev().take(6).map(|i| take(i, &s.history[i], self.copied == Some(i), me.clone())).collect()
        };
        div()
            .flex()
            .flex_col()
            .gap_10()
            .child(title("Home"))
            .child(deck)
            .when(!setup_done, |d| d.child(setup_list))
            .child(strip)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(label(if recent.is_empty() { "NO TAKES YET" } else { "RECENT" }).pb_2())
                    .when(recent.is_empty(), |d| {
                        d.child(hint("Hold fn anywhere and talk."))
                    })
                    .children(recent),
            )
    }

    fn history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.lock().unwrap().history.len();
        // New takes arrive at the top; keep the rows already measured.
        let known = self.takes.item_count();
        if count > known {
            self.takes.splice(0..0, count - known);
        } else if count < known {
            self.takes.reset(count);
        }
        let (state, copied, me) = (self.state.clone(), self.copied, cx.entity().downgrade());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(title("History"))
            .child(hint("Click one to copy it."))
            .child(
                gpui::list(self.takes.clone(), move |row, _, _| {
                    let s = state.lock().unwrap();
                    let i = s.history.len().saturating_sub(row + 1);
                    match s.history.get(i) {
                        Some(entry) => take(i, entry, copied == Some(i), me.clone()),
                        None => div().into_any_element(),
                    }
                })
                .flex_1()
                .min_h_0(),
            )
    }

    fn dictionary(&self) -> impl IntoElement {
        let rules = self.slow.dictionary.clone();
        let empty = rules.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_6()
            .child(title("Dictionary"))
            .child(hint("heard => meant, one per line."))
            .child(div().flex().child(button("edit-dict", "EDIT").on_click(|_, _, _| {
                let _ = std::process::Command::new("open").arg("-t").arg(store::dictionary_path()).spawn();
            })))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .when(empty, |d| d.child(hint("None yet. e.g. a cappy bar => capybara")))
                    .children(rules.into_iter().map(|(heard, meant)| {
                        div()
                            .flex()
                            .gap_3()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(p().hair))
                            .child(div().w(px(220.)).font_family(MONO).text_sm().text_color(rgb(p().slate)).child(heard))
                            .child(div().font_family(MONO).text_sm().text_color(rgb(p().dim)).child("→"))
                            .child(div().font_family(MONO).text_sm().text_color(rgb(p().ink)).child(meant))
                    })),
            )
    }

    fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (devices, chosen, in_use, ting_heard, level) = {
            let s = self.state.lock().unwrap();
            (s.devices.clone(), s.mic.clone(), s.mic_in_use.clone(), s.ting_heard, s.level)
        };
        let mac_default = &self.slow.mac_default;
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
        options.push((format!("Same as the Mac ({mac_default})"), None, None));
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
                    .py_3()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(p().hair))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(p().panel)))
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
                            .border_color(rgb(if selected { p().gold } else { p().dim }))
                            .when(selected, |d| d.bg(rgb(p().gold))),
                    )
                    .child(div().flex_1().text_size(px(15.)).text_color(rgb(if selected { p().ink } else { p().slate })).child(name))
                    .children(tag.map(|t| div().font_family(MONO).text_xs().text_color(rgb(if t == "FX MIC" { p().gold } else { p().dim })).child(t)))
                    .when(selected, |d| d.child(meter(10, ((level * 1.15).min(1.0) * 10.) as usize, true, true)))
                    .into_any_element(),
            );
        }
        if other_count > 0 {
            let show = self.show_other_mics;
            rows.push(
                div()
                    .id("other-mics")
                    .pt_3()
                    .px_3()
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(p().slate))
                    .cursor_pointer()
                    .hover(|s| s.text_color(rgb(p().ink)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.show_other_mics = !show;
                        cx.notify();
                    }))
                    .child(if show { "HIDE OTHER DEVICES".to_string() } else { format!("OTHER DEVICES ({other_count})") })
                    .into_any_element(),
            );
        }
        let section = |name: &'static str| div().flex().flex_col().gap_2().child(label(name).px_3().pb_1());
        let fn_free = self.state.lock().unwrap().checks.fn_free;
        let (disk, note) = {
            let s = self.state.lock().unwrap();
            (s.ting_disk, s.ting_note.clone())
        };
        div()
            .flex()
            .flex_col()
            .gap(px(56.))
            .child(title("Settings"))
            .child(self.models(cx))
            .child(
                section("MICROPHONE")
                    .child(div().flex().flex_col().children(rows))
                    .child(div().pt_2().child(switch("ready", "Keep the mic ready", KEEP_MIC_READY.load(Ordering::Relaxed), |on| {
                        KEEP_MIC_READY.store(on, Ordering::Relaxed);
                        store::save_settings(&store::Settings { keep_mic_ready: on, ..store::load_settings() });
                    })))
                    .child(hint("Off: the mic light goes out between takes.").px_3()),
            )
            .child(
                section("PUSH TO TALK")
                    .child(div().px_3().text_size(px(15.)).text_color(rgb(p().ink)).child("Hold fn, talk, let go."))
                    .child(hint("Double-tap for hands-free. Esc cancels.").px_3())
                    .when(!fn_free, |d| d.child(hint("fn opens emoji? Set Press 🌐 key to Do nothing.").px_3()))
                    .child({
                        let recording = crate::hotkey::recording();
                        let chosen = crate::hotkey::button();
                        let text = if recording {
                            "Press a key or mouse button…".to_string()
                        } else {
                            chosen.map_or("Another key: none".into(), |b| format!("Another key: {}", crate::hotkey::name(b)))
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .pt_3()
                            .px_3()
                            .child(div().flex_1().text_size(px(15.)).text_color(rgb(if recording { p().gold } else { p().slate })).child(text))
                            .when(chosen.is_some() && !recording, |d| {
                                d.child(button("clear", "CLEAR").on_click(|_, _, _| {
                                    crate::hotkey::set_button(None);
                                    let settings = store::load_settings();
                                    store::save_settings(&store::Settings { button: None, ..settings });
                                }))
                            })
                            .child(button("choose", "CHOOSE").on_click(|_, _, _| crate::hotkey::record(true)))
                    }),
            )
            // Only when there's an FX mic to talk about.
            .when(ting_heard || disk.is_some() || note.is_some(), |d| {
                let line = match disk {
                    None if note.is_some() => None,
                    None => Some("Squeeze to talk. Bottom button: Enter. Middle: undo."),
                    Some(false) => Some("Plugged in. Setting up adds a script and four tones; its files are backed up first."),
                    Some(true) => Some("Set up. Unplug it, press the button above its USB port, then squeeze."),
                };
                let state = self.state.clone();
                d.child(
                    section("FX MIC")
                        .children(line.map(|l| hint(l).px_3()))
                        .when(disk == Some(false), |d| {
                            d.child(div().flex().pt_1().px_3().child(button("ting-setup", "SET UP THIS FX MIC").on_click(move |_, _, _| {
                                let state = state.clone();
                                std::thread::spawn(move || {
                                    let note = match crate::ting_setup::disk().map(|d| crate::ting_setup::set_up(&d)) {
                                        Some(Ok(backup)) => format!(
                                            "Done. Unplug it, press the button above its USB port, then squeeze. Old files: {}",
                                            backup.display()
                                        ),
                                        Some(Err(e)) => format!("Couldn't set it up: {e}"),
                                        None => "The FX mic was unplugged.".into(),
                                    };
                                    state.lock().unwrap().ting_note = Some(note);
                                });
                            })))
                        })
                        .children(note.map(|n| hint(n).px_3())),
                )
            })
            .child(
                section("APPEARANCE").child({
                    let now = APPEARANCE.load(Ordering::Relaxed);
                    let choice = |id: &'static str, text: &'static str, value: u8| {
                        let on = now == value;
                        div()
                            .id(id)
                            .px_4()
                            .py(px(7.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(rgb(if on { p().gold } else { p().hair }))
                            .font_family(MONO)
                            .text_xs()
                            .text_color(rgb(if on { p().gold } else { p().slate }))
                            .cursor_pointer()
                            .hover(|s| s.text_color(rgb(p().ink)))
                            .on_click(move |_, _, _| {
                                APPEARANCE.store(value, Ordering::Relaxed);
                                let name = ["dark", "light", "auto"][value as usize];
                                store::save_settings(&store::Settings { appearance: Some(name.into()), ..store::load_settings() });
                            })
                            .child(text)
                    };
                    div()
                        .flex()
                        .gap_2()
                        .px_3()
                        .child(choice("look-dark", "DARK", 0))
                        .child(choice("look-light", "LIGHT", 1))
                        .child(choice("look-auto", "MATCH MAC", 2))
                }),
            )
            .child(
                section("GENERAL")
                    .child(switch("sounds", "Sounds", crate::sound::enabled(), |on| {
                        crate::sound::set_enabled(on);
                        store::save_settings(&store::Settings { sounds: on, ..store::load_settings() });
                    }))
                    .child(switch("mute", "Mute the Mac while you talk", MUTE_WHILE_TALKING.load(Ordering::Relaxed), |on| {
                        MUTE_WHILE_TALKING.store(on, Ordering::Relaxed);
                        store::save_settings(&store::Settings { mute_while_talking: on, ..store::load_settings() });
                    }))
                    .child(switch("words", "Live words above the dot", SHOW_WORDS.load(Ordering::Relaxed), |on| {
                        SHOW_WORDS.store(on, Ordering::Relaxed);
                        store::save_settings(&store::Settings { show_words: on, ..store::load_settings() });
                    }))
                    .child(switch("login", "Open at login", self.slow.login, crate::login::set)),
            )
            .child(label(concat!("HEYRA ", env!("CARGO_PKG_VERSION"))).px_3().text_color(rgb(p().dim)))
    }

    /// The speech models: Heyra's list, then a folder of your own. Clicking one
    /// downloads it if needed and switches; the current one works until then.
    fn models(&self, cx: &mut Context<Self>) -> Div {
        let (in_use, switching, progress, error, phase, retry) = {
            let s = self.state.lock().unwrap();
            (s.model.clone(), s.switching.clone(), s.progress, s.model_error.clone(), s.phase, s.retry.clone())
        };
        // One status for any model row: failed, downloading (or waiting to retry), or at rest.
        let status = |choice: &Choice, rest: (String, u32)| -> (String, u32) {
            let busy = switching.as_ref() == Some(choice) || (in_use == *choice && phase == Phase::Loading);
            if error.as_ref().is_some_and(|(c, _)| c == choice) && !busy {
                ("TRY AGAIN".into(), p().rec)
            } else if busy {
                match (&retry, progress) {
                    (Some((_, at)), _) => {
                        let secs = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                        (format!("RETRYING IN {secs}s"), p().gold)
                    }
                    // Downloaded: unpacking and test-loading it takes a few seconds more.
                    (None, Some(done)) if done >= 0.999 => ("SETTING UP".into(), p().gold),
                    (None, Some(done)) => (format!("DOWNLOADING {}%", (done * 100.) as u32), p().gold),
                    (None, None) => ("LOADING".into(), p().gold),
                }
            } else {
                rest
            }
        };
        let row = |id: (&'static str, usize), name: String, note: String, selected: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_3()
                .py_3()
                .px_3()
                .border_b_1()
                .border_color(rgb(p().hair))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(p().panel)))
                .child(
                    div()
                        .flex_none()
                        .size(px(9.))
                        .rounded_full()
                        .border_1()
                        .border_color(rgb(if selected { p().gold } else { p().dim }))
                        .when(selected, |d| d.bg(rgb(p().gold))),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().text_size(px(15.)).text_color(rgb(if selected { p().ink } else { p().slate })).child(name))
                        .child(div().text_size(px(13.)).text_color(rgb(p().dim)).child(note)),
                )
        };
        let mut rows = Vec::new();
        for (i, m) in model::ALL.iter().enumerate() {
            let choice = Choice::Listed(m.id);
            let using = in_use == choice;
            let here = self.slow.here.get(i).copied().unwrap_or(false);
            let busy = switching.as_ref() == Some(&choice);
            let (status, color) = status(&choice, if using {
                ("IN USE".into(), p().gold)
            } else if here {
                ("ON THIS MAC".into(), p().dim)
            } else {
                (format!("{} MB", m.mb), p().dim)
            });
            let cmds = self.cmds.clone();
            rows.push(
                row(("model", i), m.name.into(), m.note.into(), switching.as_ref().map_or(using, |c| *c == choice))
                    .on_click(move |_, _, _| {
                        let _ = cmds.send(Cmd::UseModel(Choice::Listed(m.id)));
                    })
                    .child(div().font_family(MONO).text_xs().text_color(rgb(color)).child(status))
                    .when(here && !using && !busy, |d| {
                        d.child(button(("remove", i), "REMOVE").on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            if let Err(e) = m.remove() {
                                store::log(&format!("couldn't remove {}: {e}", m.name));
                            }
                        }))
                    })
                    .into_any_element(),
            );
        }
        // A folder of your own: shown once chosen, and picked with the button below.
        let folder = match (&in_use, &switching) {
            (_, Some(c @ Choice::Folder(_))) | (c @ Choice::Folder(_), _) => Some(c.clone()),
            _ => None,
        };
        if let Some(choice @ Choice::Folder(dir)) = &folder {
            let using = in_use == *choice;
            let (status, color) = status(choice, if using { ("IN USE".into(), p().gold) } else { ("LOADING".into(), p().gold) });
            let picked = switching.as_ref().map_or(using, |c| c == choice);
            let (cmds, choice) = (self.cmds.clone(), choice.clone());
            rows.push(
                row(("model", 99), choice.name(), dir.display().to_string(), picked)
                    .on_click(move |_, _, _| {
                        let _ = cmds.send(Cmd::UseModel(choice.clone()));
                    })
                    .child(div().font_family(MONO).text_xs().text_color(rgb(color)).child(status))
                    .into_any_element(),
            );
        }
        let (cmds, state) = (self.cmds.clone(), self.state.clone());
        let choose = button("model-folder", "USE YOUR OWN…").on_click(cx.listener(move |_, _, _, cx| {
            let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
                files: false,
                directories: true,
                multiple: false,
                prompt: Some("Use this model".into()),
            });
            let (cmds, state) = (cmds.clone(), state.clone());
            cx.spawn(async move |_, _| {
                let Ok(Ok(Some(paths))) = paths.await else { return };
                let Some(dir) = paths.into_iter().next() else { return };
                // Loading the model to check it takes a moment: not on the window's thread.
                // Only the most recent pick counts, whichever check finishes first.
                let ticket = FOLDER_PICKS.fetch_add(1, Ordering::Relaxed) + 1;
                std::thread::spawn(move || {
                    let checked = model::check_folder(&dir);
                    if FOLDER_PICKS.load(Ordering::Relaxed) != ticket {
                        return;
                    }
                    match checked {
                        Ok(()) => {
                            let _ = cmds.send(Cmd::UseModel(Choice::Folder(dir)));
                        }
                        Err(e) => state.lock().unwrap().model_error = Some((Choice::Folder(dir), e)),
                    }
                });
            })
            .detach();
        }));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(label("VOICE MODEL").px_3().pb_1())
            .child(div().flex().flex_col().children(rows))
            .children(error.map(|(c, e)| div().text_sm().text_color(rgb(p().rec)).child(format!("{}: {e}", c.name()))))
            .children(retry.map(|(why, _)| div().text_sm().text_color(rgb(p().slate)).child(format!("{why} Heyra tries again by itself."))))
            .child(div().flex().pt_3().px_3().gap_2().child(choose).child(button("data", "OPEN DATA FOLDER").on_click(|_, _, _| {
                let _ = std::process::Command::new("open").arg(store::dir()).spawn();
            })))
    }
}

impl Render for Main {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        // The sidebar springs between labels and an icon rail. Labels stay laid out and
        // are clipped by its edge, fading with its width, so nothing reflows.
        let (width, moving) = self.rail_width();
        if moving {
            window.request_animation_frame();
        }
        let shown = ((width - RAIL_NARROW) / (RAIL_WIDE - RAIL_NARROW)).clamp(0.0, 1.0);
        let rail = div()
            .w(px(width))
            .overflow_hidden()
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .justify_between()
            .pt(px(6.))
            .pb_6()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .px(px(23.))
                            .pb_8()
                            .flex()
                            .items_center()
                            // The mark is the h: the name runs on from it.
                            .gap(px(15.))
                            .child(mark(18.))
                            .child(
                                div()
                                    .font_family(DISPLAY)
                                    .text_size(px(19.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(p().ink))
                                    .whitespace_nowrap()
                                    .opacity(shown)
                                    .child(tracked("eyra")),
                            ),
                    )
                    .child(self.nav("nav-home", "icons/home.svg", "HOME", Page::Home, shown, cx))
                    .child(self.nav("nav-history", "icons/history.svg", "HISTORY", Page::History, shown, cx))
                    .child(self.nav("nav-dict", "icons/dictionary.svg", "DICTIONARY", Page::Dictionary, shown, cx))
                    .child(self.nav("nav-settings", "icons/settings.svg", "SETTINGS", Page::Settings, shown, cx)),
            )
            .child(
                div()
                    .px(px(27.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .whitespace_nowrap()
                    .child(div().flex().items_center().gap_2().child(light(phase)).child(label(match phase {
                        Phase::Listening => "p().rec",
                        Phase::Transcribing => "WRITING",
                        Phase::Loading => "LOADING",
                        Phase::Ready => "READY · FN",
                        Phase::Error => "CHECK HOME",
                    }).opacity(shown)))
                    .child(label(short(&mic, 17)).text_color(rgb(p().dim)).whitespace_nowrap().opacity(shown)),
            );
        let toggle = self
            .with_tip(div().id("rail-toggle"), "rail-toggle", if self.rail_open { "Hide sidebar" } else { "Show sidebar" }, Side::Right, cx)
            .size(px(26.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(alpha(p().ink, 0.06)))
            .on_click(cx.listener(|this, _, _, cx| {
                // A reversal mid-slide starts from where it is.
                this.rail_from = this.rail_width().0;
                this.rail_open = !this.rail_open;
                this.rail_since = Instant::now();
                store::save_settings(&store::Settings { sidebar_narrow: !this.rail_open, ..store::load_settings() });
                cx.notify();
            }))
            .child(gpui::svg().path("icons/sidebar.svg").size(px(17.)).text_color(rgb(p().slate)));
        // A top bar for the traffic lights and the sidebar toggle; under it the sidebar,
        // and the page as a panel set into the window.
        let top = div().h(px(44.)).flex_none().flex().items_center().pl(px(84.)).child(toggle);
        div()
            .flex()
            .flex_col()
            .size_full()
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.refetch(cx);
                cx.notify();
            }))
            .bg(rgb(p().rail))
            .font_family(SANS)
            .text_color(rgb(p().ink))
            .child(top)
            .child(
                div().flex().flex_1().min_h_0().child(rail).child({
                    // History scrolls inside its own list, which needs the page's full height.
                    let own_scroll = self.page == Page::History;
                    div()
                        .id("page")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .mr_2()
                        .mb_2()
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgb(p().hair))
                        .bg(rgb(p().ground))
                        .pt(px(56.))
                        .px(px(64.))
                        .when(!own_scroll, |d| d.overflow_y_scroll().pb(px(64.)))
                        .child(div().w_full().min_w_0().max_w(px(920.)).mx_auto().when(own_scroll, |d| d.h_full()).child(page))
                }),
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
    /// A key to show after the words, as a keycap ("fn").
    pub key: Option<&'static str>,
}

const TOAST_HEIGHT: f32 = 30.;
/// Room around the pill for its shadow.
const TOAST_MARGIN: f32 = 36.;

/// `above`: how far above the dot's centre the pill sits.
pub fn open_toast(cx: &mut App, centre: (f64, f64), above: f32, toast: Toast) -> Option<gpui::WindowHandle<Toast>> {
    // Generous for the sans at this size; the pill itself sizes to its words.
    let chars = (toast.lead.chars().count() + toast.text.chars().count() + toast.key.map_or(0, |k| k.len() + 4)) as f32;
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
            })
            .children(self.key.map(|key| {
                div()
                    .px(px(6.))
                    .py(px(1.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(alpha(CREAM, 0.25))
                    .bg(alpha(CREAM, 0.06))
                    .font_family(MONO)
                    .text_size(px(11.))
                    .text_color(rgb(CREAM))
                    .child(key)
            }));
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

/// The words so far, above the orb while talking: the take written as it's heard.
/// One window for the whole run, faded in and out, so it never flickers into being.
pub struct Caption {
    pub text: String,
}

/// Fade a window in or out; the window server runs the animation.
#[allow(unexpected_cfgs)]
pub fn fade(window: &Window, visible: bool) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        unsafe {
            let view = appkit.ns_view.as_ptr() as *mut Object;
            let native: *mut Object = msg_send![view, window];
            let animator: *mut Object = msg_send![native, animator];
            let alpha: f64 = if visible { 1.0 } else { 0.0 };
            let _: () = msg_send![animator, setAlphaValue: alpha];
        }
    }
}

const CAPTION_WIDTH: f32 = 560.;
/// Room for the card: it sizes itself to its words (two lines at most), from the bottom.
const CAPTION_HEIGHT: f32 = 120.;

/// `above`: how far above the orb's centre the caption's middle sits.
pub fn open_caption(cx: &mut App, centre: (f64, f64), above: f32, caption: Caption) -> Option<gpui::WindowHandle<Caption>> {
    let (width, height) = (CAPTION_WIDTH + TOAST_MARGIN * 2., CAPTION_HEIGHT + TOAST_MARGIN * 2.);
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
        |_, cx| cx.new(|_| caption),
    )
    .ok()
    .inspect(|handle| {
        let _ = handle.update(cx, |_, window, _| {
            no_shadow(window);
            click_through(window);
        });
    })
}

impl Render for Caption {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // The end of what's been said: what fits in two lines, from a word's start.
        const SHOWN: usize = 112;
        let chars = self.text.chars().count();
        let tail: String = if chars > SHOWN {
            let cut: String = self.text.chars().skip(chars - SHOWN).collect();
            format!("…{}", cut.split_once(' ').map_or(cut.as_str(), |(_, rest)| rest))
        } else {
            self.text.clone()
        };
        // Sized to the words, up to two lines, growing up from the bottom. The width is
        // set from the length (the sans at 15 px is about 7.7 px a character): a text left
        // to find its own width is measured unwrapped and spills over the card.
        let width = (tail.chars().count() as f32 * 7.7 + 44.).clamp(120., CAPTION_WIDTH);
        let card = div()
            .w(px(width))
            .px(px(20.))
            .py(px(14.))
            .rounded(px(16.))
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(alpha(0x30323c, 0.96), 0.),
                gpui::linear_color_stop(alpha(GROUND, 0.96), 1.),
            ))
            .border_1()
            .border_color(alpha(CREAM, 0.07))
            .shadow(vec![gpui::BoxShadow {
                color: alpha(0x000000, 0.35),
                offset: point(px(0.), px(4.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
            }])
            .font_family(SANS)
            .text_size(px(15.))
            .line_height(px(24.))
            .text_color(rgb(CREAM))
            .child(tail);
        div().size_full().flex().items_end().justify_center().pb(px(TOAST_MARGIN)).child(card)
    }
}

/// The start of `text`, at most `chars` long, ended at a word with "…".
fn short(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let head: String = text.chars().take(chars).collect();
    let cut = head.rfind(' ').unwrap_or(head.len());
    format!("{}…", head[..cut].trim_end_matches([',', '.', ';', ':']))
}

/// A take that had nowhere to go: its words and a button to copy them.
pub struct Held {
    pub text: String,
    pub clicked: Rc<Cell<bool>>,
}

const HELD_WIDTH: f32 = 460.;
const HELD_HEIGHT: f32 = 64.;

pub fn open_held(cx: &mut App, centre: (f64, f64), above: f32, held: Held) -> Option<gpui::WindowHandle<Held>> {
    let (width, height) = (HELD_WIDTH + TOAST_MARGIN * 2., HELD_HEIGHT + TOAST_MARGIN * 2.);
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
        |_, cx| cx.new(|_| held),
    )
    .ok()
    .inspect(|handle| {
        let _ = handle.update(cx, |_, window, _| no_shadow(window));
    })
}

impl Render for Held {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let clicked = self.clicked.clone();
        let card = div()
            .id("held")
            .w(px(HELD_WIDTH))
            .h(px(HELD_HEIGHT))
            .pl(px(18.))
            .pr(px(12.))
            .rounded(px(14.))
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(alpha(0x30323c, 0.97), 0.),
                gpui::linear_color_stop(alpha(GROUND, 0.97), 1.),
            ))
            .border_1()
            .border_color(alpha(GOLD, 0.35))
            .shadow(vec![gpui::BoxShadow {
                color: alpha(0x000000, 0.35),
                offset: point(px(0.), px(4.)),
                blur_radius: px(14.),
                spread_radius: px(0.),
            }])
            .flex()
            .items_center()
            .gap_4()
            .cursor_pointer()
            .on_click(move |_, _, _| clicked.set(true))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(label("NOWHERE TO TYPE").text_color(rgb(GOLD)))
                    .child(div().w_full().font_family(SANS).text_size(px(14.)).text_color(rgb(CREAM)).truncate().child(short(&self.text, 52))),
            )
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py(px(7.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgb(GOLD))
                    .font_family(MONO)
                    .text_xs()
                    .text_color(rgb(GOLD))
                    .child("COPY"),
            );
        div().size_full().flex().items_center().justify_center().child(gpui::AnimationExt::with_animation(
            card,
            "held-in",
            gpui::Animation::new(Duration::from_millis(320)).with_easing(gpui::ease_out_quint()),
            |card, t| card.opacity(t).mt(px(8. * (1. - t))),
        ))
    }
}

/// Show the pointing hand (or the arrow again) while Heyra isn't the active app: the dot
/// is hovered over other apps' windows, where macOS would otherwise keep their cursor.
#[allow(unexpected_cfgs)]
pub fn hand_cursor(on: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe extern "C" {
        fn _CGSDefaultConnection() -> i32;
        fn CGSSetConnectionProperty(cid: i32, target: i32, key: core_foundation::string::CFStringRef, value: core_foundation::base::CFTypeRef) -> i32;
    }
    static ALLOWED: std::sync::Once = std::sync::Once::new();
    ALLOWED.call_once(|| unsafe {
        use core_foundation::base::TCFType;
        let key = core_foundation::string::CFString::from_static_string("SetsCursorInBackground");
        let cid = _CGSDefaultConnection();
        CGSSetConnectionProperty(cid, cid, key.as_concrete_TypeRef(), core_foundation::boolean::CFBoolean::true_value().as_CFTypeRef());
    });
    unsafe {
        let cursor: *mut Object = if on { msg_send![class!(NSCursor), pointingHandCursor] } else { msg_send![class!(NSCursor), arrowCursor] };
        let _: () = msg_send![cursor, set];
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

/// macOS draws a shadow around even a clear window, and a titled window's top edge;
/// the floating pieces must have neither.
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
            // GPUI makes these titled windows with the bar hidden; macOS still draws a
            // titled window's 1 px top edge, a white line on dark screens. Borderless (and
            // still a non-activating panel) has none.
            const NONACTIVATING_PANEL: u64 = 1 << 7;
            let _: () = msg_send![native, setStyleMask: NONACTIVATING_PANEL];
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
