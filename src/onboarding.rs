//! The first run: a quiet window with the orb in it, one thing at a time.
//! Each step asks for one permission with a line on why, and steps that are
//! already done are skipped. Closing it at any point is fine; Home's setup
//! list covers whatever is left.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnimationExt, App, Bounds, Context, FontWeight, ImageSource, IntoElement, ParentElement, Render, RenderImage,
    Stateful, Styled, Window, WindowBounds, WindowOptions, div, img, point, px, rgb, size,
};
use gpui::prelude::*;

use crate::gpu_orb::{Gpu, Motion};
use crate::setup::{self, Mic};
use crate::ui::{CREAM, GOLD, GROUND, MONO, SANS, SLATE, alpha};
use crate::worker::{Cmd, Phase, Shared};
use std::sync::mpsc::Sender;

/// The orb's size in the window, in points.
const ORB: f32 = 200.;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Welcome,
    Microphone,
    Access,
    FnKey,
    Try,
    Ready,
}

pub struct Onboarding {
    state: Shared,
    cmds: Sender<Cmd>,
    step: Step,
    gpu: Option<Gpu>,
    motion: Motion,
    last: Instant,
    frame: Option<Arc<RenderImage>>,
    /// Takes in History when "Try it" began; one more means it worked.
    takes_before: usize,
}

pub fn open(cx: &mut App, state: Shared, cmds: Sender<Cmd>) {
    let bounds = Bounds::centered(None, size(px(460.), px(580.)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Welcome to Heyra".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(14.))),
            }),
            is_resizable: false,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|_| Onboarding {
                state,
                cmds,
                step: Step::Welcome,
                gpu: Gpu::new().ok(),
                // It begins as the resting dot and grows into the orb.
                motion: Motion::new(0.12),
                last: Instant::now(),
                frame: None,
                takes_before: 0,
            })
        },
    )
    .ok();
    cx.activate(true);
}

impl Onboarding {
    /// Is this step already done, so there's nothing to ask?
    fn done(&self, step: Step) -> bool {
        let checks = self.state.lock().unwrap().checks;
        match step {
            Step::Microphone => checks.mic == Mic::Allowed,
            Step::Access => checks.accessibility,
            Step::FnKey => checks.fn_free,
            Step::Welcome | Step::Try | Step::Ready => false,
        }
    }

    fn advance(&mut self) {
        const ORDER: [Step; 6] = [Step::Welcome, Step::Microphone, Step::Access, Step::FnKey, Step::Try, Step::Ready];
        let at = ORDER.iter().position(|&s| s == self.step).unwrap_or(0);
        self.step = ORDER[at + 1..].iter().copied().find(|&s| !self.done(s)).unwrap_or(Step::Ready);
        if self.step == Step::Try {
            self.takes_before = self.state.lock().unwrap().history.len();
        }
    }

    /// The orb, drawn by the same shader as the overlay, one frame per render.
    fn orb(&mut self, window: &mut Window, listening: bool, writing: bool, level: f32) -> Option<Arc<RenderImage>> {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.motion.step(dt, listening, writing, false, level, 1.0);
        let gpu = self.gpu.as_ref()?;
        let pixels = (ORB * window.scale_factor()) as u64;
        let mut uniforms = self.motion.uniforms();
        uniforms.res = [pixels as f32, pixels as f32];
        let bgra = gpu.snapshot(pixels, &uniforms);
        let buffer = image::RgbaImage::from_raw(pixels as u32, pixels as u32, bgra)?;
        let frame = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        if let Some(old) = self.frame.replace(frame.clone()) {
            let _ = window.drop_image(old);
        }
        Some(frame)
    }
}

/// The globe printed on the fn key, in thin lines: an outline, a meridian, the equator.
fn globe() -> gpui::Div {
    let line = alpha(CREAM, 0.75);
    div()
        .relative()
        .flex_none()
        .size(px(10.))
        .rounded_full()
        .border_1()
        .border_color(line)
        .child(div().absolute().top(px(-1.)).left(px(2.5)).w(px(3.)).h(px(10.)).rounded_full().border_1().border_color(line))
        .child(div().absolute().top(px(3.5)).left(px(0.)).w(px(8.)).h(px(1.)).bg(line))
}

/// The fn key, drawn as a key: 🌐 fn.
fn keycap() -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.))
        .h(px(20.))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(px(5.))
        .border_1()
        .border_b_2()
        .border_color(alpha(CREAM, 0.22))
        .bg(alpha(CREAM, 0.06))
        .font_family(MONO)
        .text_size(px(11.5))
        .text_color(rgb(CREAM))
        .child(globe())
        .child("fn")
}

/// One line of text, with "fn" shown as the key.
fn line(text: &str) -> gpui::Div {
    let mut row = div().flex().flex_wrap().justify_center().items_center().gap(px(5.));
    let mut run = String::new();
    for word in text.split(' ') {
        let rest = word.strip_prefix("fn").filter(|r| r.chars().all(|c| !c.is_alphanumeric()));
        let Some(rest) = rest else {
            if !run.is_empty() {
                run.push(' ');
            }
            run.push_str(word);
            continue;
        };
        if !run.is_empty() {
            row = row.child(div().child(std::mem::take(&mut run)));
        }
        // Punctuation stays against the key: "fn,".
        row = row.child(div().flex().items_center().child(keycap()).children((!rest.is_empty()).then(|| rest.to_string())));
    }
    if !run.is_empty() {
        row = row.child(div().child(run));
    }
    row
}

/// A soft rounded button; the main one is lit.
fn pill(id: &'static str, text: impl Into<gpui::SharedString>, main: bool) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(22.))
        .h(px(36.))
        .flex()
        .items_center()
        .rounded_full()
        .border_1()
        .border_color(alpha(if main { GOLD } else { CREAM }, if main { 0.55 } else { 0.12 }))
        .bg(alpha(if main { GOLD } else { CREAM }, if main { 0.10 } else { 0.0 }))
        .text_size(px(13.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(if main { CREAM } else { SLATE }))
        .cursor_pointer()
        .hover(move |d| d.bg(alpha(if main { GOLD } else { CREAM }, if main { 0.18 } else { 0.05 })).text_color(rgb(CREAM)))
        .child(text.into())
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (phase, level, progress, history_len, last_text) = {
            let s = self.state.lock().unwrap();
            (s.phase, s.level, s.progress, s.history.len(), s.history.last().map(|e| e.text.clone()))
        };
        // Steps that finish outside this window (System Settings) move on by themselves.
        if matches!(self.step, Step::Access | Step::FnKey) && self.done(self.step) {
            self.advance();
        }
        let mic = self.state.lock().unwrap().checks.mic;
        let listening = mic == Mic::Allowed && self.step != Step::Welcome && phase != Phase::Transcribing;
        let frame = self.orb(window, listening, phase == Phase::Transcribing, level);
        let tried = self.step == Step::Try && history_len > self.takes_before;
        let loading = phase == Phase::Loading;

        // (title, words, main button, other button)
        let (title, words, main, other): (String, String, Option<&'static str>, Option<&'static str>) = match self.step {
            Step::Welcome => (
                "Heyra".into(),
                "Hold fn, talk, let go.\nYour words appear where you're typing. Nothing leaves this Mac.".into(),
                Some("Begin"),
                None,
            ),
            Step::Microphone => match mic {
                Mic::Allowed => ("Say something".into(), "The orb follows your voice.".into(), Some("Continue"), None),
                Mic::Denied => (
                    "Microphone is off".into(),
                    "Turn Heyra on in Privacy & Security → Microphone.".into(),
                    Some("Open Settings"),
                    Some("Skip"),
                ),
                Mic::Unknown => (
                    "Microphone".into(),
                    "Heyra only listens while you hold fn.".into(),
                    Some("Allow microphone"),
                    None,
                ),
            },
            Step::Access => (
                "Accessibility".into(),
                "So Heyra can hear fn and type for you.\nTurn Heyra on in the list that opens.".into(),
                Some("Open Settings"),
                Some("Skip"),
            ),
            Step::FnKey => (
                "The fn key".into(),
                "Keyboard → Press 🌐 key to → Do Nothing,\nso fn doesn't also open emoji.".into(),
                Some("Open Keyboard Settings"),
                Some("Skip"),
            ),
            Step::Try if tried => (String::new(), "That's Heyra.".into(), Some("Continue"), None),
            Step::Try if loading => (
                "Almost ready".into(),
                format!(
                    "Downloading the voice model, once{}",
                    progress.map(|p| format!(" · {}%", (p * 100.) as u32)).unwrap_or_default()
                ),
                None,
                Some("Done"),
            ),
            Step::Try => ("Try it".into(), "Hold fn and say something.\nLet go when you're done.".into(), None, Some("Skip")),
            Step::Ready => (
                "You're ready".into(),
                "Click into any text box, hold fn and talk.\nDouble-tap fn for hands-free.\nYou can change the key in Settings.\nEvery take is saved in History, on this Mac.\nHeyra lives in the dot at the bottom of your screen.".into(),
                Some("Start"),
                Some("Open Heyra"),
            ),
        };

        let step = self.step;
        let mic_now = mic;
        let act = cx.listener(move |this, _, window, cx| {
            match step {
                Step::Welcome => this.advance(),
                Step::Microphone => match mic_now {
                    Mic::Allowed => this.advance(),
                    Mic::Denied => setup::open_settings(setup::PANE_MICROPHONE),
                    // macOS asks now; the answer moves this step on.
                    Mic::Unknown => {
                        let state = this.state.clone();
                        setup::request_mic(move |granted| {
                            let mut s = state.lock().unwrap();
                            s.checks.mic = if granted { Mic::Allowed } else { Mic::Denied };
                            s.mic_go = true;
                        });
                    }
                },
                Step::Access => {
                    crate::hotkey::trusted(true);
                }
                Step::FnKey => setup::open_settings(setup::PANE_KEYBOARD),
                Step::Try => this.advance(),
                // Off it goes, to the dot it lives in.
                Step::Ready => {
                    this.state.lock().unwrap().toast =
                        Some(("Ready".into(), "Hold fn anywhere".into(), Instant::now()));
                    window.remove_window();
                }
            }
            cx.notify();
        });
        let skip = cx.listener(move |this, _, window, cx| {
            if step == Step::Ready {
                window.remove_window();
                crate::ui::open_main(cx, this.state.clone(), this.cmds.clone());
            } else {
                this.advance();
            }
            cx.notify();
        });

        let content = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.))
            .when(tried, |d| {
                d.child(
                    div()
                        .max_w(px(360.))
                        .text_center()
                        .text_size(px(19.))
                        .line_height(px(27.))
                        .text_color(rgb(CREAM))
                        .child(format!("“{}”", last_text.clone().unwrap_or_default())),
                )
            })
            .when(!title.is_empty(), |d| {
                d.child(div().text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).text_color(rgb(CREAM)).child(title))
            })
            .child(
                div()
                    .max_w(px(400.))
                    .text_center()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(rgb(SLATE))
                    .children(words.lines().map(line).collect::<Vec<_>>()),
            )
            .child(
                div()
                    .pt(px(16.))
                    .flex()
                    .gap(px(10.))
                    .children(main.map(|t| pill("main", t, true).on_click(act)))
                    .children(other.map(|t| pill("other", t, false).on_click(skip))),
            )
            .with_animation(
                gpui::ElementId::Name(format!("{step:?}{tried}").into()),
                gpui::Animation::new(Duration::from_millis(520)).with_easing(gpui::ease_out_quint()),
                |d, t| d.opacity(t).mt(px(10. * (1. - t))),
            );

        div()
            .size_full()
            .font_family(SANS)
            .bg(gpui::linear_gradient(
                180.,
                gpui::linear_color_stop(rgb(0x23252c), 0.),
                gpui::linear_color_stop(rgb(GROUND), 1.),
            ))
            .flex()
            .flex_col()
            .items_center()
            .pt(px(56.))
            .child(div().size(px(ORB)).children(frame.map(|f| img(ImageSource::Render(f)).size(px(ORB)))))
            .child(div().pt(px(20.)).child(content))
            // The model downloads behind every step; a hairline shows how far.
            .when_some(progress.filter(|_| loading && step != Step::Try), |d, p| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(26.))
                        .left_0()
                        .w_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .font_family(MONO)
                                .text_size(px(10.5))
                                .text_color(alpha(SLATE, 0.8))
                                .child(format!("VOICE MODEL · {}%", (p * 100.) as u32)),
                        )
                        .child(
                            div()
                                .w(px(160.))
                                .h(px(2.))
                                .rounded_full()
                                .bg(alpha(CREAM, 0.08))
                                .child(div().h_full().w(px(160. * p)).rounded_full().bg(alpha(GOLD, 0.7))),
                        ),
                )
            })
    }
}
