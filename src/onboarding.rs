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
use crate::worker::{Phase, Shared};

/// The orb's size in the window, in points.
const ORB: f32 = 200.;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    Welcome,
    Microphone,
    Access,
    FnKey,
    Try,
}

pub struct Onboarding {
    state: Shared,
    step: Step,
    gpu: Option<Gpu>,
    motion: Motion,
    last: Instant,
    frame: Option<Arc<RenderImage>>,
    /// Takes in History when "Try it" began; one more means it worked.
    takes_before: usize,
}

pub fn open(cx: &mut App, state: Shared) {
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
            Step::Welcome | Step::Try => false,
        }
    }

    fn advance(&mut self) {
        const ORDER: [Step; 5] = [Step::Welcome, Step::Microphone, Step::Access, Step::FnKey, Step::Try];
        let at = ORDER.iter().position(|&s| s == self.step).unwrap_or(0);
        self.step = ORDER[at + 1..].iter().copied().find(|&s| !self.done(s)).unwrap_or(Step::Try);
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
                "Hold fn, talk, let go.\nYour words appear wherever you type, written here on this Mac.".into(),
                Some("Begin"),
                None,
            ),
            Step::Microphone => match mic {
                Mic::Allowed => ("Say something".into(), "The orb follows your voice.".into(), Some("Continue"), None),
                Mic::Denied => (
                    "The microphone is off".into(),
                    "Turn on Heyra in Privacy & Security → Microphone.".into(),
                    Some("Open Settings"),
                    Some("Skip"),
                ),
                Mic::Unknown => (
                    "First, your microphone".into(),
                    "Heyra only listens while you hold the key.".into(),
                    Some("Allow microphone"),
                    None,
                ),
            },
            Step::Access => (
                "Then, typing".into(),
                "Accessibility lets Heyra hear fn and type your words.\nTurn on Heyra in the list.".into(),
                Some("Open Settings"),
                Some("Skip"),
            ),
            Step::FnKey => (
                "One small change".into(),
                "So fn doesn't open the emoji picker:\nKeyboard → Press 🌐 key to → Do Nothing.".into(),
                Some("Open Keyboard Settings"),
                Some("Skip"),
            ),
            Step::Try if tried => (
                String::new(),
                "That's Heyra. It lives in the dot at the bottom of your screen.".into(),
                Some("Done"),
                None,
            ),
            Step::Try if loading => (
                "Almost ready".into(),
                format!(
                    "Getting the voice model, just this once{}",
                    progress.map(|p| format!(" · {}%", (p * 100.) as u32)).unwrap_or_default()
                ),
                None,
                Some("Done"),
            ),
            Step::Try => ("Try it".into(), "Hold fn and say something.\nLet go when you're done.".into(), None, Some("Done")),
        };

        let step = self.step;
        let mic_now = mic;
        let act = cx.listener(move |this, _, window, cx| {
            match step {
                Step::Welcome => this.advance(),
                Step::Microphone => match mic_now {
                    Mic::Allowed => this.advance(),
                    Mic::Denied => setup::open_settings(setup::PANE_MICROPHONE),
                    // The worker opens the mic now, and macOS asks.
                    Mic::Unknown => this.state.lock().unwrap().mic_go = true,
                },
                Step::Access => {
                    crate::hotkey::trusted(true);
                }
                Step::FnKey => setup::open_settings(setup::PANE_KEYBOARD),
                Step::Try => window.remove_window(),
            }
            cx.notify();
        });
        let skip = cx.listener(move |this, _, window, cx| {
            if step == Step::Try {
                window.remove_window();
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
                    .max_w(px(340.))
                    .text_center()
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .text_color(rgb(SLATE))
                    .children(words.lines().map(|l| div().child(l.to_string())).collect::<Vec<_>>()),
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
