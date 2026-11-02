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
    ground: 0xf3f3f6,
    panel: 0xffffff,
    rail: 0xe9e9ee,
    hair: 0xe0e0e6,
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
fn tip_pill(text: SharedString, side: Side) -> impl IntoElement {
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
        .font_family(SANS)
        .text_size(px(12.))
        .line_height(px(14.))
        .text_color(rgb(p().ground))
        .whitespace_nowrap()
        .child(text.clone());
    // In with a little spring: it slides the last few points from its target, passes
    // its place a touch and settles (ease-out-back), fading in as it goes.
    let back = |t: f32| {
        let (c1, c3) = (1.9_f32, 2.9_f32);
        1. + c3 * (t - 1.).powi(3) + c1 * (t - 1.).powi(2)
    };
    gpui::AnimationExt::with_animation(
        pill,
        ElementId::Name(text),
        gpui::Animation::new(Duration::from_millis(240)).with_easing(back),
        move |pill, t| {
            let gap = 6. * (1. - t);
            let pill = pill.opacity((t * 1.6).min(1.));
            match side {
                Side::Right => pill.ml(px(-gap)),
                Side::Above => pill.mt(px(gap)),
            }
        },
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
        Side::Right => div().ml(px(10.)).mt(px(-1.)).child(tip_pill(text, side)),
        Side::Above => div().ml(px(-width / 2.)).mt(px(-32.)).child(tip_pill(text, side)),
    }))
    .with_priority(1);
    match side {
        Side::Right => div().absolute().left_full().top_0().h_full().flex().items_center().child(pill),
        Side::Above => div().absolute().bottom_full().left_0().w_full().flex().justify_center().child(pill),
    }
}

/// A small label in sentence case.
fn label(text: impl Into<SharedString>) -> Div {
    div().font_family(SANS).text_size(px(12.5)).text_color(rgb(p().slate)).child(text.into())
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
        .font_family(SANS)
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

/// iOS's inset grouped list: a title above, rows in a rounded card split by inset
/// hairlines, and a footnote below.
fn group(name: Option<&'static str>, items: Vec<AnyElement>, footer: Option<SharedString>) -> Div {
    // The card has a little padding; each row's highlight is a rounded pill inside it,
    // and the hairlines stop short of both edges, as macOS lists do.
    let mut card = div().flex().flex_col().p(px(6.)).rounded(px(14.)).bg(rgb(p().panel)).border_1().border_color(rgb(p().hair));
    for (i, item) in items.into_iter().enumerate() {
        if i > 0 {
            card = card.child(divider());
        }
        card = card.child(div().rounded(px(9.)).overflow_hidden().child(item));
    }
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(name.map(|n| label(n).px(px(18.)).pb_1()))
        .child(card)
        .children(footer.map(|f| hint(f).px(px(18.)).pt_1()))
}

/// The hairline between rows in a group, inset from both edges.
fn divider() -> Div {
    div().h(px(1.)).mx(px(12.)).bg(rgb(p().hair))
}

/// A row in a group: comfortable to hit, lit faintly under the pointer when it does something.
fn row(id: impl Into<gpui::ElementId>) -> Stateful<Div> {
    div().id(id).flex().items_center().gap_3().min_h(px(46.)).px(px(12.)).py(px(11.))
}

/// A setting that's on or off: an iOS toggle at the row's end. `set` gets the new value.
fn switch(id: &'static str, text: &'static str, on: bool, set: impl Fn(bool) + 'static) -> AnyElement {
    let track = div()
        .flex_none()
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .p(px(2.))
        .flex()
        .when(on, |d| d.justify_end())
        .bg(if on { rgb(p().gold).into() } else { alpha(p().ink, 0.14) })
        .child(div().size(px(18.)).rounded_full().bg(rgb(0xffffff)).shadow(vec![gpui::BoxShadow {
            color: alpha(0x000000, 0.25),
            offset: point(px(0.), px(1.)),
            blur_radius: px(3.),
            spread_radius: px(0.),
        }]));
    row(id)
        .cursor_pointer()
        .on_click(move |_, _, _| set(!on))
        .child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child(text))
        .child(track)
        .into_any_element()
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
/// The orb in Home's card.
const ORB_CARD: f32 = 120.;
/// Between the orb's frames as it drifts at rest: 15 a second.
const ORB_DRIFT: Duration = Duration::from_millis(66);
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
            "icons/chevron.svg" => include_bytes!("../assets/icons/chevron.svg"),
            "icons/mic.svg" => include_bytes!("../assets/icons/mic.svg"),
            "icons/model.svg" => include_bytes!("../assets/icons/model.svg"),
            "icons/sun.svg" => include_bytes!("../assets/icons/sun.svg"),
            "icons/moon.svg" => include_bytes!("../assets/icons/moon.svg"),
            "icons/auto.svg" => include_bytes!("../assets/icons/auto.svg"),
            _ => return Ok(None),
        };
        Ok(Some(std::borrow::Cow::Borrowed(bytes)))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

/// One take: number and time, your words, duration. A click copies it.
/// `full`: every word (History); otherwise three lines at most (Home). `lit`: the
/// pointer's on it, so its highlight springs in.
fn take(index: usize, entry: &store::Entry, copied: bool, full: bool, lit: bool, main: gpui::WeakEntity<Main>) -> AnyElement {
    let text = entry.text.clone();
    let key: SharedString = format!("take-{index}").into();
    let hover_main = main.clone();
    div()
        .id(("take", index))
        .relative()
        .w_full()
        .flex()
        .gap_6()
        .py(px(14.))
        .px(px(12.))
        .cursor_pointer()
        .on_hover(move |hovered, _, cx| {
            let key = key.clone();
            let _ = hover_main.update(cx, |main, cx| {
                if *hovered {
                    main.hovered = Some(key);
                } else if main.hovered.as_ref() == Some(&key) {
                    main.hovered = None;
                }
                cx.notify();
            });
        })
        // The highlight: a rounded pill that grows in with a little overshoot.
        .when(lit, |d| {
            d.child(gpui::AnimationExt::with_animation(
                div().absolute().top_0().bottom_0().left_0().right_0().rounded(px(9.)).bg(alpha(p().ink, 0.055)),
                ElementId::Name(format!("lit-{index}").into()),
                gpui::Animation::new(Duration::from_millis(260)).with_easing(|t: f32| {
                    let (c1, c3) = (1.7_f32, 2.7_f32);
                    1. + c3 * (t - 1.).powi(3) + c1 * (t - 1.).powi(2)
                }),
                |pill, t| {
                    let inset = 5. * (1. - t);
                    pill.top(px(inset)).bottom(px(inset)).left(px(inset * 2.)).right(px(inset * 2.)).opacity((t * 1.5).min(1.))
                },
            ))
        })
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
                .relative()
                .flex_1()
                .min_w_0()
                .font_family(SANS)
                .text_size(px(15.))
                .line_height(px(23.))
                .text_color(rgb(p().ink))
                .when(!full, |d| d.line_clamp(3))
                .child(entry.text.clone())
                // On History: where it went, which model, how long it was, how long it took.
                .when(full, |d| {
                    let mut parts: Vec<String> = Vec::new();
                    parts.extend(entry.app.clone());
                    parts.extend(entry.model.clone());
                    parts.push(format!("{:.1} s audio", entry.secs));
                    if entry.took > 0.0 {
                        parts.push(format!("{:.2} s to text", entry.took));
                    }
                    d.child(div().pt(px(6.)).child(label(parts.join(" · ")).text_color(rgb(p().dim))))
                }),
        )
        // When, and how long, on the right.
        .child(
            div()
                .relative()
                .flex_none()
                .w(px(72.))
                .pt(px(3.))
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(label(clock(entry.at)))
                .child(if copied {
                    label("Copied").text_color(rgb(p().gold))
                } else {
                    label(format!("{:.1} s", entry.secs)).text_color(rgb(p().dim))
                }),
        )
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Home,
    Settings,
    History,
    Dictionary,
}

pub struct Main {
    state: Shared,
    cmds: Sender<Cmd>,
    page: Page,
    copied: Option<usize>,
    born: Instant,
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
    /// The dropdown that's open, if any.
    menu: Option<&'static str>,
    /// The menu last closed by a click outside it, and when.
    menu_closed: Option<(&'static str, Instant)>,
    /// The orb in Home's card: its shader, motion, last frame, and when that was drawn.
    orb: Option<crate::gpu_orb::Gpu>,
    orb_motion: crate::gpu_orb::Motion,
    orb_last: Instant,
    orb_image: Option<std::sync::Arc<gpui::RenderImage>>,
    orb_phase: Option<Phase>,
    orb_drawn: Instant,
    /// Home's orb drifts at rest while the window is in use.
    orb_drifting: bool,
    /// The width the latest slide started from.
    rail_from: f32,
    /// History, newest first: only the rows on screen are laid out.
    takes: gpui::ListState,
    /// Typing goes to the dictionary's boxes while one is being edited.
    focus: gpui::FocusHandle,
    dict: Option<DictEdit>,
}

/// The dictionary while it's being edited: every rule, a blank one at the end for
/// the next, and the box the typing goes into (rule, and 0 for "when I say", 1 for "write").
struct DictEdit {
    rows: Vec<(String, String)>,
    at: (usize, usize),
}

impl DictEdit {
    fn cell(&mut self) -> &mut String {
        let (i, side) = self.at;
        let row = &mut self.rows[i];
        if side == 0 { &mut row.0 } else { &mut row.1 }
    }

    /// Always one blank rule at the end to type the next into.
    fn keep_blank(&mut self) {
        if self.rows.last().is_none_or(|(h, m)| !h.is_empty() || !m.is_empty()) {
            self.rows.push((String::new(), String::new()));
        }
    }

    fn save(&self) {
        store::save_dictionary(&self.rows);
    }
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
                    menu_closed: None,
                    menu: std::env::var("HEYRA_MENU").ok().and_then(|m| ["mic", "model", "home-mic", "home-model", "look"].into_iter().find(|k| *k == m)),
                    orb: crate::gpu_orb::Gpu::new().ok(),
                    orb_motion: crate::gpu_orb::Motion::new(1.0),
                    orb_last: Instant::now(),
                    orb_image: None,
                    orb_phase: None,
                    orb_drawn: Instant::now(),
                    orb_drifting: false,
                    takes: gpui::ListState::new(0, gpui::ListAlignment::Top, px(600.)),
                    focus: cx.focus_handle(),
                    dict: None,
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

    /// Home's orb. A new frame only while it moves (talking, writing) or when the state
    /// changes; otherwise the last one, so scrolling never waits on the GPU.
    fn orb_frame(&mut self, window: &mut Window, phase: Phase, level: f32) -> Option<std::sync::Arc<gpui::RenderImage>> {
        let moving = matches!(phase, Phase::Listening | Phase::Transcribing);
        let settling = self.orb_last.elapsed() < Duration::from_millis(900);
        // At rest it drifts, slowly, but only while this window is the one in use.
        // Its frames are asked for by `poll` at 15 a second: no redraw in between.
        self.orb_drifting = window.is_window_active();
        if self.orb_image.is_some() && !moving && !settling && self.orb_phase == Some(phase) && (!self.orb_drifting || self.orb_drawn.elapsed() < ORB_DRIFT) {
            return self.orb_image.clone();
        }
        if self.orb_phase != Some(phase) {
            self.orb_phase = Some(phase);
            self.orb_last = Instant::now();
        }
        let dt = self.orb_drawn.elapsed().as_secs_f32().min(0.1);
        self.orb_drawn = Instant::now();
        self.orb_motion.step(dt, phase == Phase::Listening, phase == Phase::Transcribing, false, level, 1.0);
        let gpu = self.orb.as_ref()?;
        let pixels = (ORB_CARD * window.scale_factor()) as u64;
        let mut uniforms = self.orb_motion.uniforms();
        uniforms.res = [pixels as f32, pixels as f32];
        let bgra = gpu.snapshot(pixels, &uniforms);
        let buffer = image::RgbaImage::from_raw(pixels as u32, pixels as u32, bgra)?;
        let frame = std::sync::Arc::new(gpui::RenderImage::new([image::Frame::new(buffer)]));
        if let Some(old) = self.orb_image.replace(frame.clone()) {
            let _ = window.drop_image(old);
        }
        if moving || settling {
            window.request_animation_frame();
        }
        if moving {
            self.orb_last = Instant::now();
        }
        Some(frame)
    }

    /// Open or close a menu from its trigger. A click on the trigger of an open menu
    /// first lands outside the menu, which closes it; that same click mustn't reopen it.
    fn toggle_menu(&mut self, id: &'static str) {
        let just_closed = self.menu_closed.is_some_and(|(m, at)| m == id && at.elapsed() < Duration::from_millis(400));
        self.menu = if self.menu == Some(id) || just_closed { None } else { Some(id) };
        self.menu_closed = None;
    }

    /// A floating menu of `items` on the top layer, its `anchor` corner at the spot it's
    /// placed; it closes on a pick or a click elsewhere.
    fn menu_panel(&self, id: &'static str, items: Vec<AnyElement>, anchor: gpui::Corner, cx: &Context<Self>) -> impl IntoElement {
        let panel = div()
            .id(ElementId::Name(format!("menu-{id}").into()))
            .w(px(420.))
            .max_h(px(380.))
            .overflow_y_scroll()
            .p(px(6.))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .bg(rgb(if is_light() { 0xffffff } else { 0x2a2c36 }))
            .border_1()
            .border_color(rgb(p().hair))
            .shadow(vec![gpui::BoxShadow {
                color: alpha(0x000000, 0.35),
                offset: point(px(0.), px(12.)),
                blur_radius: px(32.),
                spread_radius: px(0.),
            }])
            // A pick closes it, except the theme menu: it stays to try each look.
            .on_click(cx.listener(move |this, _, _, cx| {
                if id != "look" {
                    this.menu = None;
                }
                cx.notify();
            }))
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| {
                // Remembered, so a click on this menu's own trigger leaves it closed.
                this.menu = None;
                this.menu_closed = Some((id, Instant::now()));
                cx.notify();
            }))
            .children(items.into_iter().map(|item| div().rounded(px(8.)).overflow_hidden().child(item)));
        gpui::deferred(gpui::anchored().anchor(anchor).child(div().mt(px(6.)).child(gpui::AnimationExt::with_animation(
            panel,
            ("menu-in", id.len()),
            gpui::Animation::new(Duration::from_millis(140)).with_easing(gpui::ease_out_quint()),
            |panel, t| panel.opacity(t).mt(px(-6. * (1. - t))),
        ))))
        .with_priority(2)
    }

    /// A glassy pill on Home's dark card: an icon, the choice and a chevron; it opens
    /// the same menu as Settings, below it.
    fn chip(&self, id: &'static str, icon: &'static str, text: String, status: Option<(String, u32)>, items: Vec<AnyElement>, cx: &Context<Self>) -> impl IntoElement {
        let open = self.menu == Some(id);
        div()
            .id(ElementId::Name(format!("chip-{id}").into()))
            .relative()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(32.))
            .px(px(12.))
            .rounded_full()
            .bg(alpha(CREAM, if open { 0.12 } else { 0.06 }))
            .border_1()
            .border_color(alpha(if open { GOLD } else { CREAM }, if open { 0.5 } else { 0.10 }))
            .cursor_pointer()
            .hover(|s| s.bg(alpha(CREAM, 0.10)).border_color(alpha(CREAM, 0.2)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_menu(id);
                cx.notify();
            }))
            .child(gpui::svg().path(icon).size(px(14.)).text_color(alpha(GOLD, 0.9)))
            .child(div().font_family(SANS).text_size(px(13.)).text_color(alpha(CREAM, 0.88)).whitespace_nowrap().child(short(&text, 30)))
            .children(status.map(|(t, _)| div().font_family(SANS).text_size(px(10.)).text_color(rgb(GOLD)).child(t)))
            .child(gpui::svg().path("icons/chevron.svg").size(px(13.)).text_color(alpha(CREAM, 0.5)))
            .when(open, |d| d.child(div().absolute().top_full().left_0().child(self.menu_panel(id, items, gpui::Corner::TopLeft, cx))))
    }

    /// A row that opens a menu: its name, what's chosen and a chevron; the menu floats
    /// below its right edge on the top layer, and closes on a pick or a click elsewhere.
    fn dropdown(&self, id: &'static str, name: &'static str, current: String, status: Option<(String, u32)>, items: Vec<AnyElement>, cx: &Context<Self>) -> AnyElement {
        let open = self.menu == Some(id);
        let menu = self.menu_panel(id, items, gpui::Corner::TopRight, cx);
        row(id)
            .cursor_pointer()
            .hover(|s| s.bg(alpha(p().ink, 0.035)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_menu(id);
                cx.notify();
            }))
            .child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child(name))
            .children(status.map(|(text, color)| div().font_family(SANS).text_xs().text_color(rgb(color)).child(text)))
            .child(div().text_size(px(15.)).text_color(rgb(p().slate)).child(short(&current, 34)))
            .child(gpui::svg().path("icons/chevron.svg").size(px(15.)).text_color(rgb(if open { p().gold } else { p().slate })))
            .child(div().relative().w_0().h_full().when(open, |d| d.child(div().absolute().top_full().right_0().child(menu))))
            .into_any_element()
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
        let orb_due = self.page == Page::Home && self.orb_drifting && self.orb_drawn.elapsed() >= ORB_DRIFT;
        if seen != self.seen || orb_due {
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
            .font_family(SANS)
            .text_size(px(13.5))
            .text_color(rgb(color))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page = page;
                this.copied = None;
                cx.notify();
            }))
            .child(gpui::svg().path(icon).size(px(17.)).flex_none().text_color(rgb(if active { p().gold } else { color })))
            .child(div().whitespace_nowrap().opacity(shown).child(text))
    }

    fn home(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let count = s.history.len();
        let level = s.level;
        drop(s);

        let frame = self.orb_frame(window, phase, level);
        let state_word = match phase {
            Phase::Listening => "LISTENING",
            Phase::Transcribing => "WRITING",
            Phase::Loading => "LOADING",
            Phase::Ready => "READY",
            Phase::Error => "CHECK",
        };

        // A viewport into space: the orb itself, live. Always dark, whatever the theme:
        // it's a window onto the dot's world.
        let live = matches!(phase, Phase::Listening | Phase::Transcribing);
        let (current_mic, mic_items) = self.mic_menu(cx);
        let mic_chip = self.chip("home-mic", "icons/mic.svg", current_mic, None, mic_items, cx);
        let (current_model, model_status, model_items) = self.model_menu();
        let model_chip = self.chip("home-model", "icons/model.svg", current_model, model_status, model_items, cx);
        let deck = div()
            .relative()
            .overflow_hidden()
            .rounded(px(16.))
            .border_1()
            .border_color(alpha(CREAM, 0.07))
            .bg(gpui::linear_gradient(
                135.,
                gpui::linear_color_stop(rgb(0x07080c), 0.),
                gpui::linear_color_stop(rgb(0x151824), 1.),
            ))
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(28.))
                    .px(px(28.))
                    .py(px(22.))
                    .child(div().flex_none().size(px(ORB_CARD)).children(frame.map(|f| gpui::img(gpui::ImageSource::Render(f)).size(px(ORB_CARD)))))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                div()
                                    .font_family(DISPLAY)
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(px(24.))
                                    .text_color(rgb(if live || phase == Phase::Error { GOLD } else { CREAM }))
                                    .child(tracked(state_word)),
                            )
                            .child(div().font_family(SANS).text_size(px(16.)).line_height(px(23.)).text_color(alpha(CREAM, 0.72)).child(message))
                            .child(div().pt(px(8.)).flex().gap(px(8.)).child(mic_chip).child(model_chip)),
                    ),
            );

        let card = || div().flex().flex_col().p_7().rounded(px(14.)).bg(rgb(p().panel)).border_1().border_color(rgb(p().hair));
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
                    .child(div().flex().flex_col().gap_2().child(label("Today")).child(big(format!("{} words", grouped(today)))))
                    .child(label(format!("{} all time", grouped(words))).text_color(rgb(p().dim))),
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
            .child(card().flex_1().gap_2().child(label("Time saved")).child(big(saved)))
            .child(
                card()
                    .flex_1()
                    .gap_2()
                    .child(label("Streak"))
                    .child(big(format!("{streak} {}", if streak == 1 { "day" } else { "days" })))
                    .child(div().pt_2().child(week)),
            )
            .child(
                card()
                    .flex_1()
                    .gap_2()
                    .child(label("Pace"))
                    .child(big(format!("{pace} wpm")))
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(pace_bar("You", pace, rgb(p().gold).into()))
                            .child(pace_bar("Typing", 40, alpha(p().ink, 0.16))),
                    ),
            );
        let strip = div().flex().flex_col().gap_4().child(words_card).child(small);


        let me = cx.entity().downgrade();
        let recent: Vec<AnyElement> = {
            let s = self.state.lock().unwrap();
            (0..count).rev().take(6).map(|i| take(i, &s.history[i], self.copied == Some(i), false, self.hovered.as_ref().map(|h| h.as_ref()) == Some(format!("take-{i}").as_str()), me.clone())).collect()
        };
        div()
            .flex()
            .flex_col()
            .gap_10()
            .child(title("Home"))
            .children(self.setup(cx))
            .child(deck)
            .child(strip)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(if recent.is_empty() {
                        group(Some("Recent"), vec![row("none").child(hint("Hold fn anywhere and talk.")).into_any_element()], None)
                    } else {
                        group(Some("Recent"), recent, None)
                    }),
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
        let (state, copied, me, hovered) = (self.state.clone(), self.copied, cx.entity().downgrade(), self.hovered.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_6()
            .child(title("History"))
            .child(hint("Click one to copy it."))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .mb(px(28.))
                    .p(px(6.))
                    .rounded(px(14.))
                    .bg(rgb(p().panel))
                    .border_1()
                    .border_color(rgb(p().hair))
                    .overflow_hidden()
                    .child(
                        gpui::list(self.takes.clone(), move |n, _, _| {
                            let s = state.lock().unwrap();
                            let i = s.history.len().saturating_sub(n + 1);
                            let Some(entry) = s.history.get(i) else { return div().into_any_element() };
                            // List items don't stretch by themselves: full width, so times line up.
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .when(n > 0, |d| d.child(divider()))
                                .child(take(i, entry, copied == Some(i), true, hovered.as_ref().map(|h| h.as_ref()) == Some(format!("take-{i}").as_str()), me.clone()))
                                .into_any_element()
                        })
                        .size_full(),
                    ),
            )
    }

    /// The dictionary as boxes: when I say this, write that. Click a box to type; Tab
    /// moves on, Enter saves and starts the next rule, Esc saves and stops.
    fn dictionary(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editing = self.dict.as_ref().map(|d| d.at);
        let rows: Vec<(String, String)> = match &self.dict {
            Some(d) => d.rows.clone(),
            None => {
                let mut r = self.slow.dictionary.clone();
                r.push((String::new(), String::new()));
                r
            }
        };
        if editing.is_some() && !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let count = rows.len();
        let cell = |i: usize, side: usize, text: String, cx: &mut Context<Self>| {
            let on = editing == Some((i, side));
            let blank = text.is_empty();
            let placeholder = if side == 0 { "When I say" } else { "Write" };
            div()
                .id(("dict-cell", i * 2 + side))
                .flex_1()
                .min_w_0()
                .h(px(34.))
                .px(px(10.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .border_1()
                .border_color(if on { alpha(p().gold, 0.7) } else { rgb(p().hair).into() })
                .bg(alpha(p().ink, if on { 0.05 } else { 0.02 }))
                .cursor_text()
                .font_family(MONO)
                .text_sm()
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.dict.is_none() {
                        let mut rows = this.slow.dictionary.clone();
                        rows.push((String::new(), String::new()));
                        this.dict = Some(DictEdit { rows, at: (i, side) });
                    } else if let Some(d) = &mut this.dict {
                        d.at = (i, side);
                    }
                    window.focus(&this.focus);
                    cx.notify();
                }))
                .child(if blank && !on {
                    div().text_color(rgb(p().dim)).child(placeholder)
                } else {
                    div().text_color(rgb(if side == 0 { p().slate } else { p().ink })).child(text)
                })
                .when(on, |d| d.child(div().ml(px(1.)).w(px(1.5)).h(px(16.)).bg(rgb(p().gold))))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        items.push(
            row("dict-head")
                .child(div().flex_1().child(label("When I say")))
                .child(div().w(px(18.)))
                .child(div().flex_1().child(label("Write")))
                .child(div().w(px(26.)))
                .into_any_element(),
        );
        for (i, (heard, meant)) in rows.into_iter().enumerate() {
            let last = i + 1 == count;
            let remove = (!last).then(|| {
                div()
                    .id(("dict-remove", i))
                    .w(px(26.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .text_color(rgb(p().dim))
                    .hover(|d| d.bg(alpha(p().ink, 0.06)).text_color(rgb(p().ink)))
                    .child("×")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut rows = match this.dict.take() {
                            Some(d) => d.rows,
                            None => this.slow.dictionary.clone(),
                        };
                        if i < rows.len() {
                            rows.remove(i);
                        }
                        store::save_dictionary(&rows);
                        this.slow.dictionary = store::load_dictionary();
                        cx.notify();
                    }))
            });
            items.push(
                row(("rule", i))
                    .gap_2()
                    .child(cell(i, 0, heard, cx))
                    .child(div().w(px(18.)).flex().justify_center().text_color(rgb(p().dim)).child("→"))
                    .child(cell(i, 1, meant, cx))
                    .child(match remove {
                        Some(r) => r.into_any_element(),
                        None => div().w(px(26.)).into_any_element(),
                    })
                    .into_any_element(),
            );
        }
        div()
            .id("dictionary")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                let Some(d) = &mut this.dict else { return };
                let key = event.keystroke.key.as_str();
                let shift = event.keystroke.modifiers.shift;
                match key {
                    "backspace" => {
                        d.cell().pop();
                    }
                    "tab" => {
                        let (i, side) = d.at;
                        d.at = if shift {
                            if side == 1 { (i, 0) } else { (i.saturating_sub(1), if i == 0 { 0 } else { 1 }) }
                        } else if side == 0 {
                            (i, 1)
                        } else {
                            ((i + 1).min(d.rows.len() - 1), 0)
                        };
                    }
                    "enter" => {
                        d.keep_blank();
                        d.save();
                        d.at = (d.rows.len() - 1, 0);
                    }
                    "escape" => {
                        d.save();
                        this.dict = None;
                        this.slow.dictionary = store::load_dictionary();
                    }
                    "v" if event.keystroke.modifiers.platform => {
                        if let Ok(text) = arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
                            let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
                            d.cell().push_str(&line);
                            d.keep_blank();
                        }
                    }
                    _ => {
                        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
                            return;
                        }
                        if let Some(text) = &event.keystroke.key_char {
                            d.cell().push_str(text);
                            d.keep_blank();
                        }
                    }
                }
                if let Some(d) = &this.dict {
                    d.save();
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .flex()
            .flex_col()
            .gap_6()
            .child(title("Dictionary"))
            .child(group(None, items, Some("Case-insensitive. Saves as you type.".into())))
    }

    /// First-run setup, on General until every step is done.
    fn setup(&self, _cx: &mut Context<Self>) -> Option<Div> {
        let (model_ready, progress, checks, count) = {
            let s = self.state.lock().unwrap();
            (matches!(s.phase, Phase::Ready | Phase::Listening | Phase::Transcribing), s.progress, s.checks, s.history.len())
        };
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
            .child(label(format!("Setup · {done} of {} done", steps.len())).pb_3())
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
                (checks.mic == Mic::Denied).then_some(("Open Settings", setup::PANE_MICROPHONE)),
            ))
            .child(step(
                "s-ax",
                steps[2].0,
                steps[2].1,
                "Turn on Heyra in Privacy & Security.".into(),
                Some(("Open Settings", setup::PANE_ACCESSIBILITY)),
            ))
            .child(step(
                "s-fn",
                steps[3].0,
                steps[3].1,
                "Set Press 🌐 key to Do nothing.".into(),
                Some(("Keyboard", setup::PANE_KEYBOARD)),
            ))
            .child(step("s-take", steps[4].0, steps[4].1, "Hold fn in any text box and talk.".into(), None));
        // In a card, like the groups on Settings.
        (done < steps.len()).then(|| {
            div().px(px(18.)).pt(px(16.)).pb(px(6.)).rounded(px(14.)).bg(rgb(p().panel)).border_1().border_color(rgb(p().hair)).child(setup_list)
        })
    }

    fn settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ting_heard = self.state.lock().unwrap().ting_heard;
        let (current_mic, rows) = self.mic_menu(cx);
        let fn_free = self.state.lock().unwrap().checks.fn_free;
        let (disk, note) = {
            let s = self.state.lock().unwrap();
            (s.ting_disk, s.ting_note.clone())
        };
        let ink_row = |id: &'static str, text: String| row(id).child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child(text));


        // Push to talk.
        let recording = crate::hotkey::recording();
        let chosen = crate::hotkey::button();
        // One row: fn always works; another key or mouse button can work too.
        let tap_ok = self.state.lock().unwrap().checks.accessibility;
        let key_text = if recording && !tap_ok {
            "Allow Accessibility first".to_string()
        } else if recording {
            "Press a key or mouse button…".to_string()
        } else {
            chosen.map_or("fn".into(), |b| format!("fn or {}", crate::hotkey::name(b)))
        };
        let key_row = row("push-to-talk")
            .child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child("Push to talk"))
            .child(div().text_size(px(15.)).text_color(rgb(if recording { p().gold } else { p().slate })).child(key_text))
            .when(chosen.is_some() && !recording, |d| {
                d.child(button("clear", "Clear").on_click(|_, _, _| {
                    crate::hotkey::set_button(None);
                    let settings = store::load_settings();
                    store::save_settings(&store::Settings { button: None, ..settings });
                }))
            })
            .when(!recording, |d| {
                d.child(button("choose", if chosen.is_some() { "Change" } else { "Add a key" }).on_click(|_, _, _| crate::hotkey::record(true)))
            });
        let talk_footer = if fn_free {
            "Double-tap: hands-free. Esc: cancel."
        } else {
            "fn opens emoji? Set Press 🌐 key to Do nothing."
        };
        // The things people change, in one card at the top: how to talk, which model,
        // which mic, and opening at login.
        let main = group(
            None,
            vec![
                key_row.into_any_element(),
                self.dropdown("mic", "Microphone", current_mic, None, rows, cx),
                switch("login", "Open at login", self.slow.login, crate::login::set),
            ],
            Some(talk_footer.into()),
        );
        let dictation = group(
            Some("Dictation"),
            vec![
                switch("ready", "Keep mic open", KEEP_MIC_READY.load(Ordering::Relaxed), |on| {
                    KEEP_MIC_READY.store(on, Ordering::Relaxed);
                    store::save_settings(&store::Settings { keep_mic_ready: on, ..store::load_settings() });
                }),
                {
                    let now = crate::worker::WHILE_TALKING.load(Ordering::Relaxed);
                    let names = ["Do nothing", "Mute", "Lower volume"];
                    let items = self.choices("talking", &names, now as usize, |i| {
                        crate::worker::WHILE_TALKING.store(i as u8, Ordering::Relaxed);
                        let value = ["nothing", "mute", "lower"][i].to_string();
                        store::save_settings(&store::Settings { while_talking: Some(value), ..store::load_settings() });
                    }, cx);
                    self.dropdown("talking", "While dictating", names[now as usize].into(), None, items, cx)
                },
                {
                    let top = crate::gpu_orb::ORB_TOP.load(Ordering::Relaxed);
                    let names = ["Bottom", "Top"];
                    let items = self.choices("orb-spot", &names, top as usize, |i| {
                        crate::gpu_orb::ORB_TOP.store(i == 1, Ordering::Relaxed);
                        store::save_settings(&store::Settings { orb_top: i == 1, ..store::load_settings() });
                    }, cx);
                    self.dropdown("orb-spot", "Orb", names[top as usize].into(), None, items, cx)
                },
            ],
            None,
        );

        // FX mic, only when there's one to talk about.
        let fx = (ting_heard || disk.is_some() || note.is_some()).then(|| {
            let line = match disk {
                None if note.is_some() => None,
                None => Some("Squeeze to talk. Bottom button: Enter. Middle: undo."),
                Some(false) => Some("Adds a script and four tones. Backs up its files first."),
                Some(true) => Some("Set up. Unplug it, press the button above its USB port, then squeeze."),
            };
            let state = self.state.clone();
            let mut items: Vec<AnyElement> = Vec::new();
            if disk == Some(false) {
                items.push(
                    row("ting-setup")
                        .cursor_pointer()
                        .hover(|s| s.bg(alpha(p().ink, 0.035)))
                        .child(div().text_size(px(15.)).text_color(rgb(p().gold)).child("Set up this FX mic"))
                        .on_click(move |_, _, _| {
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
                        })
                        .into_any_element(),
                );
            } else {
                items.push(ink_row("ting", if ting_heard { "Heard on this mic".into() } else { "FX mic".into() }).into_any_element());
            }
            group(Some("FX mic"), items, note.map(Into::into).or(line.map(Into::into)))
        });

        // Appearance: three choices as rows, the one in use ticked.
        let now = APPEARANCE.load(Ordering::Relaxed);
        let look = |id: &'static str, text: &'static str, value: u8| {
            row(id)
                .cursor_pointer()
                .hover(|s| s.bg(alpha(p().ink, 0.035)))
                .on_click(move |_, _, _| {
                    APPEARANCE.store(value, Ordering::Relaxed);
                    let name = ["dark", "light", "auto"][value as usize];
                    store::save_settings(&store::Settings { appearance: Some(name.into()), ..store::load_settings() });
                })
                .child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child(text))
                .when(now == value, |d| d.child(div().text_size(px(15.)).text_color(rgb(p().gold)).child("✓")))
                .into_any_element()
        };
        let appearance = group(
            Some("Appearance"),
            vec![look("look-dark", "Dark", 0), look("look-light", "Light", 1), look("look-auto", "System", 2)],
            None,
        );

        let output = group(
            Some("Output"),
            vec![
                switch("words", "Live preview", SHOW_WORDS.load(Ordering::Relaxed), |on| {
                    SHOW_WORDS.store(on, Ordering::Relaxed);
                    store::save_settings(&store::Settings { show_words: on, ..store::load_settings() });
                }),
                switch("sounds", "Sounds", crate::sound::enabled(), |on| {
                    crate::sound::set_enabled(on);
                    store::save_settings(&store::Settings { sounds: on, ..store::load_settings() });
                }),
            ],
            None,
        );

        div()
            .flex()
            .flex_col()
            .gap(px(36.))
            .pb(px(24.))
            .child(title("Settings"))
            .child(main)
            .child(self.models(cx))
            .child(dictation)
            .child(output)
            .children(fx)
            .child(appearance)
    }

    /// A dropdown's options as menu rows, the current one marked; `pick` gets the index.
    fn choices(&self, id: &'static str, names: &[&'static str], current: usize, pick: fn(usize), cx: &mut Context<Self>) -> Vec<AnyElement> {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let selected = i == current;
                div()
                    .id((id, i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(46.))
                    .py(px(11.))
                    .px(px(12.))
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(p().ink, 0.035)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(i);
                        this.menu = None;
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
                    .child(div().flex_1().text_size(px(15.)).text_color(rgb(if selected { p().ink } else { p().slate })).child(*name))
                    .into_any_element()
            })
            .collect()
    }

    /// The mics to choose from, as menu rows, and the one chosen.
    fn mic_menu(&self, cx: &mut Context<Self>) -> (String, Vec<AnyElement>) {
        let (devices, chosen, in_use, ting_heard) = {
            let s = self.state.lock().unwrap();
            (s.devices.clone(), s.mic.clone(), s.mic_in_use.clone(), s.ting_heard)
        };
        let mac_default = &self.slow.mac_default;
        // Real mics first; the Mac's own choice; then phones and virtual devices, folded away.
        let (real, other): (Vec<_>, Vec<_>) = devices.into_iter().partition(|m| m.kind != crate::mics::Kind::Other);
        let mut options: Vec<(String, Option<&'static str>, Option<String>)> = real
            .into_iter()
            .map(|m| {
                let tag = match m.kind {
                    crate::mics::Kind::BuiltIn => Some("Built-in"),
                    crate::mics::Kind::Bluetooth => Some("Bluetooth"),
                    _ if ting_heard && m.name == in_use => Some("FX mic"),
                    _ => None,
                };
                (m.name.clone(), tag, Some(m.name))
            })
            .collect();
        options.push((format!("Same as the Mac ({mac_default})"), None, None));
        options.extend(other.into_iter().map(|m| (m.name.clone(), None, Some(m.name))));
        let current_mic = chosen.clone().unwrap_or_else(|| "Same as the Mac".into());
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
                    .min_h(px(46.))
                    .py(px(11.))
                    .px(px(12.))
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(p().ink, 0.035)))
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
                    .children(tag.map(|t| div().font_family(SANS).text_xs().text_color(rgb(if t == "FX mic" { p().gold } else { p().dim })).child(t)))
                    .into_any_element(),
            );
        }
        (current_mic, rows)
    }

    /// The speech models: Heyra's list, then a folder of your own. Clicking one
    /// downloads it if needed and switches; the current one works until then.
    fn models(&self, cx: &mut Context<Self>) -> Div {
        let (current, current_status, rows) = self.model_menu();
        let (error, retry) = {
            let s = self.state.lock().unwrap();
            (s.model_error.clone(), s.retry.clone())
        };
        let (cmds, state) = (self.cmds.clone(), self.state.clone());
        let choose = row("model-folder")
            .cursor_pointer()
            .hover(|s| s.bg(alpha(p().ink, 0.035)))
            .child(div().text_size(px(15.)).text_color(rgb(p().gold)).child("Use your own model…"))
            .on_click(cx.listener(move |_, _, _, cx| {
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
        let action = |id: &'static str, text: &'static str| {
            row(id).cursor_pointer().hover(|s| s.bg(alpha(p().ink, 0.035))).child(div().text_size(px(15.)).text_color(rgb(p().gold)).child(text))
        };
        let mut rows = vec![self.dropdown("model", "Voice model", current, current_status, rows, cx)];
        rows.push(choose.into_any_element());
        rows.push(
            action("data", "Open data folder")
                .on_click(|_, _, _| {
                    let _ = std::process::Command::new("open").arg(store::dir()).spawn();
                })
                .into_any_element(),
        );
        let footer = match (error, retry) {
            (Some((c, e)), _) => Some(format!("{}: {e}", c.name()).into()),
            (None, Some((why, _))) => Some(format!("{why} Heyra tries again by itself.").into()),
            _ => None,
        };
        group(Some("Voice model"), rows, footer)
    }

    /// The models to choose from, as menu rows; the one in use (or on its way) and how
    /// it's going.
    fn model_menu(&self) -> (String, Option<(String, u32)>, Vec<AnyElement>) {
        let (in_use, switching, progress, error, phase, retry) = {
            let s = self.state.lock().unwrap();
            (s.model.clone(), s.switching.clone(), s.progress, s.model_error.clone(), s.phase, s.retry.clone())
        };
        // One status for any model row: failed, downloading (or waiting to retry), or at rest.
        let status = |choice: &Choice, rest: (String, u32)| -> (String, u32) {
            let busy = switching.as_ref() == Some(choice) || (in_use == *choice && phase == Phase::Loading);
            if error.as_ref().is_some_and(|(c, _)| c == choice) && !busy {
                ("Try again".into(), p().rec)
            } else if busy {
                match (&retry, progress) {
                    (Some((_, at)), _) => {
                        let secs = at.saturating_duration_since(Instant::now()).as_secs() + 1;
                        (format!("Retrying in {secs} s"), p().gold)
                    }
                    // Downloaded: unpacking and test-loading it takes a few seconds more.
                    (None, Some(done)) if done >= 0.999 => ("Setting up".into(), p().gold),
                    (None, Some(done)) => (format!("Downloading {}%", (done * 100.) as u32), p().gold),
                    (None, None) => ("Loading".into(), p().gold),
                }
            } else {
                rest
            }
        };
        let model_row = |id: (&'static str, usize), name: String, note: String, selected: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_3()
                .min_h(px(46.))
                .py(px(11.))
                .px(px(12.))
                .cursor_pointer()
                .hover(|s| s.bg(alpha(p().ink, 0.035)))
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
                ("In use".into(), p().gold)
            } else if here {
                ("On this Mac".into(), p().dim)
            } else {
                (format!("{} MB", m.mb), p().dim)
            });
            let cmds = self.cmds.clone();
            rows.push(
                model_row(("model", i), m.name.into(), m.note.into(), switching.as_ref().map_or(using, |c| *c == choice))
                    .on_click(move |_, _, _| {
                        let _ = cmds.send(Cmd::UseModel(Choice::Listed(m.id)));
                    })
                    .child(div().font_family(SANS).text_xs().text_color(rgb(color)).child(status))
                    .when(here && !using && !busy, |d| {
                        d.child(button(("remove", i), "Remove").on_click(move |_, _, cx| {
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
            let (status, color) = status(choice, if using { ("In use".into(), p().gold) } else { ("Loading".into(), p().gold) });
            let picked = switching.as_ref().map_or(using, |c| c == choice);
            let (cmds, choice) = (self.cmds.clone(), choice.clone());
            rows.push(
                model_row(("model", 99), choice.name(), dir.display().to_string(), picked)
                    .on_click(move |_, _, _| {
                        let _ = cmds.send(Cmd::UseModel(choice.clone()));
                    })
                    .child(div().font_family(SANS).text_xs().text_color(rgb(color)).child(status))
                    .into_any_element(),
            );
        }
        // The one in use, or the one on its way with how it's going.
        let (current, current_status) = match &switching {
            Some(next) => (next.name(), Some(status(next, (String::new(), p().dim)))),
            None => (in_use.name(), (phase == Phase::Loading).then(|| status(&in_use, (String::new(), p().dim)))),
        };
        (current, current_status, rows)
    }
}

impl Render for Main {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, mic) = {
            let s = self.state.lock().unwrap();
            (if s.blocker.is_some() { Phase::Error } else { s.phase }, s.mic_in_use.clone())
        };
        let page = match self.page {
            Page::Home => self.home(window, cx).into_any_element(),
            Page::History => self.history(cx).into_any_element(),
            Page::Dictionary => self.dictionary(window, cx).into_any_element(),
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
                    .child(self.nav("nav-home", "icons/home.svg", "Home", Page::Home, shown, cx))
                    .child(self.nav("nav-settings", "icons/settings.svg", "Settings", Page::Settings, shown, cx))
                    .child(self.nav("nav-history", "icons/history.svg", "History", Page::History, shown, cx))
                    .child(self.nav("nav-dict", "icons/dictionary.svg", "Dictionary", Page::Dictionary, shown, cx)),
            )
            .child(
                div()
                    .px(px(27.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .whitespace_nowrap()
                    .child(div().flex().items_center().gap_2().child(light(phase)).child(label(match phase {
                        Phase::Listening => "Listening",
                        Phase::Transcribing => "Writing",
                        Phase::Loading => "Loading",
                        Phase::Ready => "Ready · fn",
                        Phase::Error => "Check Home",
                    }).opacity(shown)))
                    .child(label(short(&mic, 17)).text_color(rgb(p().dim)).whitespace_nowrap().opacity(shown))
                    // The version, and a way to check for a newer one now.
                    .child(
                        div()
                            .pt_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .opacity(shown)
                            .child(label(concat!("v", env!("CARGO_PKG_VERSION"))).text_color(rgb(p().dim)))
                            .child({
                                let state = self.state.clone();
                                div()
                                    .id("check-updates")
                                    .cursor_pointer()
                                    .child(label("Check for updates").text_color(rgb(p().slate)))
                                    .hover(|d| d.opacity(0.7))
                                    .on_click(move |_, _, _| crate::update::check_now(state.clone()))
                            }),
                    ),
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
        // Top right: the look, as a menu.
        let now = APPEARANCE.load(Ordering::Relaxed);
        let look_items: Vec<AnyElement> = [("look-dark", "icons/moon.svg", "Dark", 0u8), ("look-light", "icons/sun.svg", "Light", 1), ("look-auto", "icons/auto.svg", "System", 2)]
            .into_iter()
            .map(|(id, icon, text, value)| {
                row(id)
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(p().ink, 0.05)))
                    .on_click(move |_, _, _| {
                        APPEARANCE.store(value, Ordering::Relaxed);
                        let name = ["dark", "light", "auto"][value as usize];
                        store::save_settings(&store::Settings { appearance: Some(name.into()), ..store::load_settings() });
                    })
                    .child(gpui::svg().path(icon).size(px(16.)).text_color(rgb(if now == value { p().gold } else { p().slate })))
                    .child(div().flex_1().text_size(px(15.)).text_color(rgb(p().ink)).child(text))
                    .when(now == value, |d| d.child(div().text_size(px(15.)).text_color(rgb(p().gold)).child("✓")))
                    .into_any_element()
            })
            .collect();
        let look_open = self.menu == Some("look");
        let look_icon = ["icons/moon.svg", "icons/sun.svg", "icons/auto.svg"][now.min(2) as usize];
        let look = div()
            .id("look")
            .relative()
            .size(px(26.))
            .rounded(px(6.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(look_open, |d| d.bg(alpha(p().ink, 0.08)))
            .hover(|s| s.bg(alpha(p().ink, 0.06)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.toggle_menu("look");
                this.hovered = None;
                cx.notify();
            }))
            .child(gpui::svg().path(look_icon).size(px(16.)).text_color(rgb(if look_open { p().gold } else { p().slate })))
            .when(look_open, |d| d.child(div().absolute().top_full().right_0().child(self.menu_panel("look", look_items, gpui::Corner::TopRight, cx))));
        let top = div()
            .h(px(44.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .pl(px(84.))
            .pr(px(14.))
            .child(toggle)
            .child(look);
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
    open_popup(cx, (centre.0 + 33. * side, centre.1 - lift(33.) as f64), ORB_BUTTON, button)
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
            .on_click(move |_, window, _| {
                clicked.set(true);
                give_back_keyboard(window);
            })
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
/// Toasts, words and the copy card go above the orb, or below it when it's at the top.
fn lift(above: f32) -> f32 {
    if crate::gpu_orb::ORB_TOP.load(Ordering::Relaxed) { -above } else { above }
}

pub fn open_toast(cx: &mut App, centre: (f64, f64), above: f32, toast: Toast) -> Option<gpui::WindowHandle<Toast>> {
    // Generous for the sans at this size; the pill itself sizes to its words.
    let chars = (toast.lead.chars().count() + toast.text.chars().count() + toast.key.map_or(0, |k| k.len() + 4)) as f32;
    let width = chars * 7.4 + 70. + TOAST_MARGIN * 2.;
    let height = TOAST_HEIGHT + TOAST_MARGIN * 2.;
    let origin = point(px(centre.0 as f32 - width / 2.), px(centre.1 as f32 - lift(above) - height / 2.));
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
    /// Set when its × is clicked: hide the words, and stop showing them.
    pub closed: Rc<Cell<bool>>,
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
            // Faded out, it must not catch clicks meant for what's under it.
            let ignore: objc::runtime::BOOL = if visible { objc::runtime::NO } else { objc::runtime::YES };
            let _: () = msg_send![native, setIgnoresMouseEvents: ignore];
        }
    }
}

const CAPTION_WIDTH: f32 = 560.;
/// Room for the card: it sizes itself to its words (two lines at most), from the bottom.
const CAPTION_HEIGHT: f32 = 120.;

/// `above`: how far above the orb's centre the caption's middle sits.
pub fn open_caption(cx: &mut App, centre: (f64, f64), above: f32, caption: Caption) -> Option<gpui::WindowHandle<Caption>> {
    let (width, height) = (CAPTION_WIDTH + TOAST_MARGIN * 2., CAPTION_HEIGHT + TOAST_MARGIN * 2.);
    let origin = point(px(centre.0 as f32 - width / 2.), px(centre.1 as f32 - lift(above) - height / 2.));
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
        // Not click-through: the card takes clicks for its ×. Its clear surround still
        // lets them fall through, as macOS does for fully transparent pixels.
        let _ = handle.update(cx, |_, window, _| no_shadow(window));
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
        let width = (tail.chars().count() as f32 * 7.7 + 74.).clamp(150., CAPTION_WIDTH);
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
            .relative()
            .pr(px(30.))
            .child(tail)
            .child({
                let closed = self.closed.clone();
                div()
                    .id("close-words")
                    .absolute()
                    .top(px(8.))
                    .right(px(8.))
                    .size(px(18.))
                    .rounded_full()
                    .bg(alpha(CREAM, 0.08))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .line_height(px(11.))
                    .text_color(alpha(CREAM, 0.55))
                    .cursor_pointer()
                    .hover(|s| s.bg(alpha(CREAM, 0.16)).text_color(rgb(CREAM)))
                    .on_click(move |_, window, _| {
                        closed.set(true);
                        give_back_keyboard(window);
                    })
                    .child("✕")
            });
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
    let origin = point(px(centre.0 as f32 - width / 2.), px(centre.1 as f32 - lift(above) - height / 2.));
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
            .on_click(move |_, window, _| {
                clicked.set(true);
                give_back_keyboard(window);
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(label("Nowhere to type").text_color(rgb(GOLD)))
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
                    .font_family(SANS)
                    .text_xs()
                    .text_color(rgb(GOLD))
                    .child("Copy"),
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

/// Hand the keyboard back after a click on a floating panel. A click makes the panel
/// the key window, so ⌘V would land here and beep instead of in the app you're typing
/// in. A window ordered out loses key; ordered back without being made key, it stays shown.
#[allow(unexpected_cfgs)]
pub fn give_back_keyboard(window: &Window) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
        unsafe {
            let view = appkit.ns_view.as_ptr() as *mut Object;
            let native: *mut Object = msg_send![view, window];
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![native, orderOut: nil];
            let _: () = msg_send![native, orderFrontRegardless];
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
