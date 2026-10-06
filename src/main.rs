//! Heyra: hold fn (or squeeze a TING) and talk; let go and the words are pasted
//! where your cursor is. Speech is turned into text on this machine with
//! Parakeet, the model Heyra uses. Nothing leaves it.

mod engine;
mod hotkey;
mod login;
mod mics;
mod model;
mod gpu_orb;
mod paste;
mod record;
mod setup;
mod sound;
mod store;
mod ting;
mod tray;
mod ui;
mod worker;

use std::borrow::Cow;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{App, Application};

use engine::Parakeet;
use worker::{Cmd, Phase};

/// `heyra --file clip.wav` prints a transcript: a quick check of the engine.
fn transcribe_file(path: &str) {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let raw: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    };
    let mono: Vec<f32> = raw
        .chunks(channels)
        .map(|f| f.iter().sum::<f32>() / channels as f32)
        .collect();
    let started = std::time::Instant::now();
    let dir = model::find().expect("no model yet: open Heyra once to download it");
    let mut engine = Parakeet::load(&dir).expect("load model");
    let loaded = started.elapsed().as_secs_f32();
    let started = std::time::Instant::now();
    let text = engine::transcribe_long(&mut engine, spec.sample_rate, &mono);
    println!("{text}");
    eprintln!(
        "load {loaded:.2}s, {:.1}s audio in {:.2}s",
        mono.len() as f32 / spec.sample_rate as f32,
        started.elapsed().as_secs_f32()
    );
}

/// `heyra --preview-orb`: the orb alone with a fake voice, for design work.
fn preview_orb(state: worker::Shared) {
    {
        let state = state.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                let t = start.elapsed().as_secs_f32();
                let mut s = state.lock().unwrap();
                s.phase = match t % 10.0 {
                    x if x < 3.0 => Phase::Ready,
                    x if x < 8.0 => Phase::Listening,
                    _ => Phase::Transcribing,
                };
                s.level = ((t * 3.1).sin() * 0.5 + 0.5) * ((t * 0.7).sin() * 0.35 + 0.55);
                drop(s);
                std::thread::sleep(Duration::from_millis(30));
            }
        });
    }
    Application::new().run(move |cx: &mut App| {
        let mut orb = OrbDriver::new();
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let (phase, level) = {
                let s = state.lock().unwrap();
                (s.phase, s.level)
            };
            if cx.update(|_| orb.tick(phase, level, false)).is_err() {
                break;
            }
        })
        .detach();
    });
}

/// Eases the voice and integrates the orb's clocks, then draws a frame.
/// Talking winds the shells up; writing releases them outward.
struct OrbDriver {
    overlay: Option<gpu_orb::Overlay>,
    last: std::time::Instant,
    clock: f32,
    wave: f32,
    twist: f32,
    smooth: f32,
    writing: f32,
    scale: f32,
    /// The resting dot never changes, so it's drawn once and left alone.
    rested: bool,
}

/// How long a toast stays above the dot.
const TOAST_FOR: Duration = Duration::from_secs(3);

/// Size of the resting dot, as a fraction of the full orb.
const IDLE_SCALE: f32 = 0.12;
/// Size of the dot under the pointer.
const HOVER_SCALE: f32 = 0.3;

impl OrbDriver {
    fn new() -> Self {
        let overlay = gpu_orb::Overlay::new().map_err(|e| store::log(&format!("orb: {e}"))).ok();
        Self {
            overlay,
            last: std::time::Instant::now(),
            clock: 0.0,
            wave: 0.0,
            twist: 1.0,
            smooth: 0.0,
            writing: 0.0,
            scale: IDLE_SCALE,
            rested: false,
        }
    }

    /// Called ~60 times a second. At rest it's a tiny still dark circle, drawn
    /// once; talking grows it into the full orb.
    /// Centre of the orb on screen, once the overlay exists.
    fn centre(&self) -> Option<(f64, f64)> {
        self.overlay.as_ref().map(|o| o.centre)
    }

    fn tick(&mut self, phase: Phase, level: f32, hovering: bool) {
        let Some(overlay) = self.overlay.as_mut() else { return };
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.1);
        self.last = now;
        if matches!(phase, Phase::Loading | Phase::Error) {
            overlay.hide();
            self.rested = false;
            return;
        }
        overlay.show();
        let listening = phase == Phase::Listening;
        let writing = phase == Phase::Transcribing;
        let active = listening || writing;

        let target = if listening { (level * 1.25).min(1.0) } else { 0.0 };
        let rate = if target > self.smooth { 0.35 } else { 0.07 };
        self.smooth += (target - self.smooth) * rate;
        // Hovering the resting dot: it grows a little and spins gold.
        let gold = writing || hovering;
        self.writing += ((if gold { 1.0 } else { 0.0 }) - self.writing) * 0.12;
        let scale_target = if active { 1.0 } else if hovering { HOVER_SCALE } else { IDLE_SCALE };
        self.scale += (scale_target - self.scale) * if active { 0.22 } else { 0.1 };
        let twist_target = if writing { 0.35 } else { 1.0 + 1.6 * self.smooth };
        self.twist += (twist_target - self.twist) * 0.08;
        self.wave += dt * (if gold { 3.2 } else if listening { 0.6 + 2.0 * self.smooth } else { 0.25 });
        self.clock += dt * (if active || hovering { 0.8 + 1.2 * self.smooth } else { 0.3 });

        let settled = !active && !hovering && (self.scale - IDLE_SCALE).abs() < 0.002;
        if settled && self.rested {
            return;
        }
        self.rested = settled;
        overlay.draw(gpu_orb::Uniforms {
            time: self.clock,
            voice: self.smooth,
            wave: self.wave,
            twist: self.twist,
            writing: self.writing,
            scale: self.scale,
            ..Default::default()
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--mics" {
        for mic in mics::list() {
            println!("{:?}\t{}", mic.kind, mic.name);
        }
        println!("default: {:?}", mics::default_name());
        return;
    }
    if args.len() == 3 && args[1] == "--file" {
        return transcribe_file(&args[2]);
    }
    if args.len() == 4 && args[1] == "--orb-png" {
        // `heyra --orb-png out.png <voice 0..1 | writing>`: one GPU orb frame, timed.
        let gpu = gpu_orb::Gpu::new().expect("metal");
        let writing = args[3] == "writing";
        let voice: f32 = args[3].parse().unwrap_or(0.0);
        let size = 340u64;
        let u = gpu_orb::Uniforms {
            res: [size as f32, size as f32],
            time: 3.7,
            voice,
            wave: 2.1,
            twist: if writing { 0.4 } else { 1.0 + 1.5 * voice },
            writing: if writing { 1.0 } else { 0.0 },
            scale: if args[3] == "idle" { IDLE_SCALE } else { 1.0 },
        };
        let started = std::time::Instant::now();
        let mut px = gpu.snapshot(size, &u);
        eprintln!("frame in {:.1} ms", started.elapsed().as_secs_f32() * 1000.0);
        for p in px.chunks_mut(4) {
            p.swap(0, 2);
            let a = p[3] as f32 / 255.0;
            if a > 0.0 {
                for c in &mut p[..3] {
                    *c = ((*c as f32 / a).min(255.0)) as u8;
                }
            }
        }
        image::RgbaImage::from_raw(size as u32, size as u32, px).unwrap().save(&args[2]).unwrap();
        return;
    }
    if (args.len() == 4 || args.len() == 5) && args[1] == "--orb-frames" {
        // `heyra --orb-frames <dir> <count> [clear]`: one seamless loop of the orb for the
        // README, on the page's graphite or, with `clear`, on transparency. The shader
        // repeats every 2π/0.35 s of its clock.
        let clear = args.get(4).is_some_and(|a| a == "clear");
        let gpu = gpu_orb::Gpu::new().expect("metal");
        let dir = std::path::Path::new(&args[2]);
        std::fs::create_dir_all(dir).unwrap();
        let count: usize = args[3].parse().unwrap_or(96);
        let size = 360u64;
        let period = std::f32::consts::TAU / 0.35;
        let bg = [0x12u8 as f32, 0x13u8 as f32, 0x18u8 as f32];
        for i in 0..count {
            let phase = i as f32 / count as f32;
            let voice = 0.3 + 0.3 * (std::f32::consts::TAU * phase).sin();
            let u = gpu_orb::Uniforms {
                res: [size as f32, size as f32],
                time: period * phase,
                voice,
                wave: std::f32::consts::TAU * 2.0 * phase,
                twist: 1.0 + 1.6 * voice,
                writing: 0.0,
                scale: 1.0,
            };
            let px = gpu.snapshot(size, &u);
            if clear {
                // Premultiplied BGRA to straight RGBA.
                let mut rgba = Vec::with_capacity(px.len());
                for p in px.chunks(4) {
                    let a = p[3] as f32 / 255.0;
                    let un = |c: u8| if a > 0.0 { (c as f32 / a).min(255.0) as u8 } else { 0 };
                    rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), p[3]]);
                }
                image::RgbaImage::from_raw(size as u32, size as u32, rgba)
                    .unwrap()
                    .save(dir.join(format!("{i:03}.png")))
                    .unwrap();
                continue;
            }
            // Premultiplied BGRA over the page colour, to RGB.
            let mut rgb = Vec::with_capacity((size * size * 3) as usize);
            for p in px.chunks(4) {
                let a = p[3] as f32 / 255.0;
                for (c, b) in [p[2], p[1], p[0]].into_iter().zip(bg) {
                    rgb.push((c as f32 + b * (1.0 - a)).min(255.0) as u8);
                }
            }
            image::RgbImage::from_raw(size as u32, size as u32, rgb)
                .unwrap()
                .save(dir.join(format!("{i:03}.png")))
                .unwrap();
        }
        return;
    }
    if args.len() == 2 && args[1] == "--fetch-model" {
        // Exercise the first-run download without the window.
        let result = model::find().map(Ok).unwrap_or_else(|| {
            model::download(|done, total| eprint!("\r{} / {} MB   ", done / 1_000_000, total / 1_000_000))
        });
        eprintln!();
        match result {
            Ok(dir) => println!("model ready at {}", dir.display()),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let state = worker::new_state();
    if args.len() == 2 && args[1] == "--preview-orb" {
        return preview_orb(state);
    }
    let (tx, rx) = mpsc::channel::<Cmd>();

    {
        // Watch the keyboard. Without Accessibility, ask once, then retry until it's granted.
        let state = state.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut asked = false;
            while !hotkey::trusted(!asked) {
                asked = true;
                state.lock().unwrap().blocker = Some(
                    "Heyra needs Accessibility to hear fn: System Settings → Privacy & Security → Accessibility → turn on Heyra."
                        .into(),
                );
                std::thread::sleep(Duration::from_secs(2));
            }
            state.lock().unwrap().blocker = None;
            let tx = tx.clone();
            hotkey::set_button(store::load_settings().button);
            if let Err(e) = hotkey::listen(move |key| {
                let _ = tx.send(Cmd::Key(key));
            }) {
                store::log(&format!("error: {e}"));
                state.lock().unwrap().blocker = Some(e);
            }
        });
    }
    {
        let state = state.clone();
        std::thread::spawn(move || worker::run(state, rx));
    }
    {
        // Keep the setup list on Home current as permissions change.
        let state = state.clone();
        std::thread::spawn(move || loop {
            let checks = setup::check();
            state.lock().unwrap().checks = checks;
            std::thread::sleep(Duration::from_secs(1));
        });
    }

    // First run from the app bundle: open at login, which Settings can turn off.
    let in_bundle = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS"));
    let settings = store::load_settings();
    if in_bundle && !settings.login_offered {
        login::set(true);
        store::save_settings(&store::Settings { login_offered: true, ..settings });
    }
    let app = Application::new();
    {
        // Clicking the Dock icon brings the main window back after it was closed.
        let state = state.clone();
        let tx = tx.clone();
        app.on_reopen(move |cx| {
            if cx.windows().iter().all(|w| w.downcast::<ui::Main>().is_none()) {
                ui::open_main(cx, state.clone(), tx.clone());
            }
        });
    }
    app.run(move |cx: &mut App| {
        let _ = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf").as_slice()),
        ]);
        ui::open_main(cx, state.clone(), tx.clone());
        let tray = tray::Tray::new();
        let main_state = state.clone();
        let main_tx = tx.clone();

        // Redraw the window ~30 times a second; drive the orb at ~60.
        let state = state.clone();
        let mut orb = OrbDriver::new();
        let mut frame = 0u32;
        let clicked = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut hotspot: Option<gpui::WindowHandle<ui::Hotspot>> = None;
        // The X and the tick shown beside the orb during a hands-free take.
        let buttons: [(&'static str, f64, fn() -> Cmd); 2] = [("✕", -1.0, || Cmd::Discard), ("✓", 1.0, || Cmd::Finish)];
        let pressed = [std::rc::Rc::new(std::cell::Cell::new(false)), std::rc::Rc::new(std::cell::Cell::new(false))];
        let mut button_windows: Vec<gpui::WindowHandle<ui::OrbButton>> = Vec::new();
        let mut toast: Option<(gpui::WindowHandle<ui::Toast>, Instant)> = None;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                frame = frame.wrapping_add(1);
                let (phase, level, hands_free, note) = {
                    let s = state.lock().unwrap();
                    (s.phase, s.level, s.hands_free, s.toast.clone())
                };
                let ok = cx.update(|cx| {
                    // At rest, the pointer over the dot wakes it a little; a click opens Heyra.
                    let resting = phase == Phase::Ready;
                    let hovering = resting
                        && orb.centre().is_some_and(|(cx_, cy)| {
                            let (px_, py) = gpu_orb::pointer();
                            (px_ - cx_).hypot(py - cy) < 14.0
                        });
                    orb.tick(phase, level, hovering);
                    match (&hotspot, resting, orb.centre()) {
                        (None, true, Some(centre)) => {
                            hotspot = ui::open_hotspot(cx, centre, ui::Hotspot { clicked: clicked.clone() });
                        }
                        (Some(h), false, _) => {
                            let _ = h.update(cx, |_, window, _| window.remove_window());
                            hotspot = None;
                        }
                        _ => {}
                    }
                    match (button_windows.is_empty(), hands_free, orb.centre()) {
                        (true, true, Some(centre)) => {
                            for ((glyph, side, _), clicked) in buttons.iter().zip(&pressed) {
                                let button = ui::OrbButton { glyph, clicked: clicked.clone() };
                                button_windows.extend(ui::open_orb_button(cx, centre, *side, button));
                            }
                        }
                        (false, false, _) => {
                            for h in button_windows.drain(..) {
                                let _ = h.update(cx, |_, window, _| window.remove_window());
                            }
                        }
                        _ => {}
                    }
                    // A toast lasts three seconds; a newer one replaces it.
                    let fresh = note.as_ref().filter(|(_, at)| at.elapsed() < TOAST_FOR);
                    let shown = toast.as_ref().map(|(_, at)| *at);
                    if fresh.map(|(_, at)| *at) != shown {
                        if let Some((h, _)) = toast.take() {
                            let _ = h.update(cx, |_, window, _| window.remove_window());
                        }
                        if let (Some((text, at)), Some(centre)) = (fresh, orb.centre()) {
                            toast = ui::open_toast(cx, centre, text.clone()).map(|h| (h, *at));
                        }
                    }
                    for ((_, _, cmd), clicked) in buttons.iter().zip(&pressed) {
                        if clicked.replace(false) {
                            let _ = main_tx.send(cmd());
                        }
                    }
                    let open_from_dot = clicked.replace(false);
                    let action = tray.as_ref().and_then(|t| t.poll()).or(open_from_dot.then_some(tray::Action::Open));
                    match action {
                        Some(tray::Action::Open) => {
                            let existing = cx.windows().into_iter().find_map(|w| w.downcast::<ui::Main>());
                            match existing {
                                Some(w) => {
                                    let _ = w.update(cx, |_, window, _| window.activate_window());
                                }
                                None => ui::open_main(cx, main_state.clone(), main_tx.clone()),
                            }
                            cx.activate(true);
                        }
                        Some(tray::Action::Quit) => cx.quit(),
                        None => {}
                    }
                    if frame.is_multiple_of(2) {
                        cx.refresh_windows();
                    }
                });
                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.activate(true);
    });
}
