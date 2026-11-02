//! Heyra: hold fn (or squeeze a TING) and talk; let go and the words are pasted
//! where your cursor is. Speech is turned into text on this machine with
//! the speech model of the person's choosing. Nothing leaves it.

mod engine;
mod hotkey;
mod login;
mod mics;
mod onboarding;
mod model;
mod gpu_orb;
mod paste;
mod record;
mod setup;
mod sound;
mod store;
mod ting;
mod ting_setup;
mod tray;
mod ui;
mod update;
mod worker;

use std::borrow::Cow;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use gpui::{App, Application};

use engine::Transducer;
use worker::{Cmd, Phase};

/// `heyra --file clip.wav` prints a transcript: a quick check of the engine.
/// `heyra --live take.wav`: the take played in at real speed, transcribed as it would be
/// while talking. Prints the wait after the end and the text, then the one-pass text.
fn transcribe_live(path: &str) {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    assert_eq!(spec.channels, 1, "mono wav");
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().map(|s| s.unwrap()).collect(),
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.unwrap() as f32 / scale).collect()
        }
    };
    let dir = model::find(&model::Choice::saved()).expect("no model yet: open Heyra once to download it");
    let engine: engine::Shared = std::sync::Arc::new(std::sync::Mutex::new(Box::new(Transducer::load(&dir).expect("load model"))));
    let rate = spec.sample_rate;
    let step = (rate as f32 * 0.03) as usize;
    let mut ahead = engine::Ahead::new(engine.clone(), rate, true);
    let mut len = 0;
    let mut shown = String::new();
    eprintln!("start");
    while len < samples.len() {
        std::thread::sleep(Duration::from_millis(30));
        len = (len + step).min(samples.len());
        let before = ahead.cut();
        ahead.feed(len, |from| samples[from.min(len)..len].to_vec());
        if ahead.cut() != before {
            eprintln!("cut {:.2}", ahead.cut() as f32 / rate as f32);
        }
        let so_far = ahead.so_far();
        if so_far != shown {
            eprintln!("draft {}", serde_json::to_string(&so_far).unwrap_or_default());
            shown = so_far;
        }
    }
    let started = Instant::now();
    let live = ahead.finish(&samples);
    eprintln!("final {}", serde_json::to_string(&live).unwrap_or_default());
    eprintln!("{:.1}s take, {:.2}s after the end", samples.len() as f32 / rate as f32, started.elapsed().as_secs_f32());
    println!("live:     {live}");
    let started = Instant::now();
    let whole = engine::transcribe_long(engine.lock().unwrap().as_mut(), rate, &samples);
    eprintln!("one pass: {:.2}s", started.elapsed().as_secs_f32());
    println!("one pass: {whole}");
}

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
    let dir = model::find(&model::Choice::saved()).expect("no model yet: open Heyra once to download it");
    let mut engine = Transducer::load(&dir).expect("load model");
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
    motion: gpu_orb::Motion,
    /// The resting dot never changes, so it's drawn once and left alone.
    rested: bool,
    /// Where the overlay is: under the menu bar, or above the Dock.
    top: bool,
}

/// The app bundle this is running from, if a different version has been put in its place.
fn replaced_bundle() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app"))?.to_path_buf();
    let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).ok()?;
    let key = "<key>CFBundleShortVersionString</key>";
    let after = &plist[plist.find(key)? + key.len()..];
    let version = after.split("<string>").nth(1)?.split("</string>").next()?.trim();
    (version != env!("CARGO_PKG_VERSION")).then_some(app)
}

/// How long the pointer rests on the dot before "Dictate fn" shows.
const HOVER_TIP_AFTER: Duration = Duration::from_millis(250);

/// How long a take with nowhere to go is offered to copy.
const HELD_FOR: Duration = Duration::from_secs(12);

/// How long a toast stays above the dot.
const TOAST_FOR: Duration = Duration::from_secs(3);

/// Size of the resting dot, as a fraction of the full orb.
const IDLE_SCALE: f32 = 0.12;
/// Size of the dot under the pointer.
const HOVER_SCALE: f32 = 0.3;

impl OrbDriver {
    fn new() -> Self {
        gpu_orb::ORB_TOP.store(store::load_settings().orb_top, std::sync::atomic::Ordering::Relaxed);
        let overlay = gpu_orb::Overlay::new().map_err(|e| store::log(&format!("orb: {e}"))).ok();
        Self {
            overlay,
            last: std::time::Instant::now(),
            motion: gpu_orb::Motion::new(IDLE_SCALE),
            rested: false,
            top: gpu_orb::ORB_TOP.load(std::sync::atomic::Ordering::Relaxed),
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
        let top = gpu_orb::ORB_TOP.load(std::sync::atomic::Ordering::Relaxed);
        if top != self.top {
            self.top = top;
            overlay.place(top);
            self.rested = false;
        }
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

        // Hovering the resting dot: it grows a little and spins gold.
        let size = if active { 1.0 } else if hovering { HOVER_SCALE } else { IDLE_SCALE };
        self.motion.step(dt, listening, writing, hovering, level, size);

        let settled = !active && !hovering && (self.motion.scale - IDLE_SCALE).abs() < 0.002;
        if settled && self.rested {
            return;
        }
        self.rested = settled;
        overlay.draw(self.motion.uniforms());
    }
}

/// A menu bar app with no window open is still working: macOS mustn't close it as idle.
#[allow(unexpected_cfgs)]
fn keep_running() {
    use cocoa::base::id;
    use cocoa::foundation::NSString;
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let info: id = msg_send![class!(NSProcessInfo), processInfo];
        let reason = NSString::alloc(cocoa::base::nil).init_str("Heyra listens for push-to-talk");
        let _: () = msg_send![info, disableAutomaticTermination: reason];
        let _: () = msg_send![info, disableSuddenTermination];
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
    if args.len() == 3 && args[1] == "--live" {
        return transcribe_live(&args[2]);
    }
    // Load a model folder and transcribe a second of silence: run as a separate process
    // by `model::check_folder`, so a model the engine can't run can't take Heyra down.
    if args.len() == 3 && args[1] == "--check-model" {
        let mut engine = Transducer::load(std::path::Path::new(&args[2])).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        });
        let _ = engine::Engine::transcribe(&mut engine, 16_000, &vec![0.0; 16_000]);
        println!("ok");
        return;
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
        let model = model::default();
        let result = model.find().map(Ok).unwrap_or_else(|| {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            model
                .download(&cancel, |p| match p {
                    model::Progress::Bytes(done, total) => eprint!("\r{} / {} MB   ", done / 1_000_000, total / 1_000_000),
                    model::Progress::Retrying { reason, .. } => eprintln!("\n{reason} Trying again…"),
                })
                .map_err(|e| e.message())
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
    // `heyra --preview-words`: the app as usual, with the words box up, two lines' worth.
    if args.iter().any(|a| a == "--preview-words") {
        let state = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            let mut s = state.lock().unwrap();
            s.draft = "So the plan for Thursday is that we meet the venue people at ten, walk the space, and then decide on the layout before lunch with Neil and Emily.".into();
            s.draft_until = Some(Instant::now() + Duration::from_secs(20));
        });
    }
    // `heyra --preview-held`: the app as usual, with a take offered to copy.
    if args.iter().any(|a| a == "--preview-held") {
        let state = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            let text = "Send the deck to Neil and Emily before Thursday";
            state.lock().unwrap().held = Some((text.into(), Instant::now()));
        });
    }
    // `heyra --preview-toast`: the app as usual, with a toast up for design work.
    if args.iter().any(|a| a == "--preview-toast") {
        let state = state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            state.lock().unwrap().toast = Some(("Using".into(), "CUBILUX HLMS-C4 Line IN".into(), Instant::now()));
        });
    }
    if args.len() == 2 && args[1] == "--preview-orb" {
        return preview_orb(state);
    }
    let (tx, rx) = mpsc::channel::<Cmd>();
    let settings = store::load_settings();
    // First run: the welcome window explains, then asks; `--onboarding` shows it again.
    let welcome = !settings.onboarded || args.iter().any(|a| a == "--onboarding");
    // Set up before there was a model choice: offer the new models once.
    let new_models = !welcome && settings.model.is_none();
    // Updated since the last run: say so above the dot.
    let version = env!("CARGO_PKG_VERSION");
    if settings.last_version.as_deref().is_some_and(|v| v != version) {
        state.lock().unwrap().toast = Some(("Updated to".into(), version.into(), Instant::now()));
    }
    if settings.last_version.as_deref() != Some(version) {
        store::save_settings(&store::Settings { last_version: Some(version.into()), ..store::load_settings() });
    }
    update::start(state.clone());
    // Write the model down now, so the default a first run starts with is still the
    // model on the next launch (once onboarded, an unsaved choice reads as Parakeet).
    if settings.model.is_none() {
        model::Choice::saved().save();
    }

    {
        // Watch the keyboard. Without Accessibility, ask once, then retry until it's granted.
        let state = state.clone();
        let tx = tx.clone();
        std::thread::spawn(move || {
            // On the first run the welcome window asks, after saying why.
            let mut asked = welcome;
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
        // Keep the setup list on Home current as permissions change, and notice a
        // TING plugged in over USB-C.
        let state = state.clone();
        std::thread::spawn(move || loop {
            let checks = setup::check();
            let disk = ting_setup::disk().map(|d| ting_setup::is_set_up(&d));
            {
                let mut s = state.lock().unwrap();
                s.checks = checks;
                if disk.is_some() && s.ting_disk.is_none() {
                    s.ting_note = None;
                    s.toast = Some(match disk {
                        Some(false) => ("FX mic plugged in".into(), "Set it up in Settings".into(), Instant::now()),
                        _ => ("FX mic".into(), "Ready: unplug it and squeeze".into(), Instant::now()),
                    });
                }
                s.ting_disk = disk;
            }
            std::thread::sleep(Duration::from_secs(1));
        });
    }

    // First run from the app bundle: open at login, which Settings can turn off.
    let in_bundle = std::env::current_exe().is_ok_and(|p| p.to_string_lossy().contains(".app/Contents/MacOS"));
    sound::set_enabled(settings.sounds);
    if in_bundle && !settings.login_offered {
        login::set(true);
        store::save_settings(&store::Settings { login_offered: true, ..store::load_settings() });
    }
    let app = Application::new().with_assets(ui::Icons);
    ui::APPEARANCE.store(
        match settings.appearance.as_deref() {
            Some("light") => 1,
            Some("auto") => 2,
            _ => 0,
        },
        std::sync::atomic::Ordering::Relaxed,
    );
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
        keep_running();
        let _ = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Regular.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Medium.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/SpaceGrotesk-Bold.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/MartianMono-StdRg.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/MartianMono-StdMd.ttf").as_slice()),
            Cow::Borrowed(include_bytes!("../assets/fonts/Outland-Bold.otf").as_slice()),
        ]);
        if welcome {
            onboarding::open(cx, state.clone(), tx.clone());
            store::save_settings(&store::Settings { onboarded: true, ..store::load_settings() });
        } else if new_models {
            onboarding::open_models(cx, state.clone(), tx.clone());
        } else {
            ui::open_main(cx, state.clone(), tx.clone());
        }
        let tray = tray::Tray::new();
        let mut shown_model: Option<Option<&'static str>> = None;
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
        // Over the resting dot: since when, and the "Dictate fn" pill once it's been a moment.
        let mut hover_since: Option<Instant> = None;
        let mut hover_tip: Option<gpui::WindowHandle<ui::Toast>> = None;
        // The caption window: opened the first time it's needed, then kept, faded in and out.
        let mut caption: Option<(gpui::WindowHandle<ui::Caption>, String)> = None;
        let mut caption_shown = false;
        // The words box's ×: hide it now, and stop showing words while talking.
        let words_closed = std::rc::Rc::new(std::cell::Cell::new(false));
        // A take with nowhere to go, offered to copy: its window, when it was held, the click.
        let mut held: Option<(gpui::WindowHandle<ui::Held>, Instant)> = None;
        let held_clicked = std::rc::Rc::new(std::cell::Cell::new(false));
        // Where the orb was last placed: when it moves, everything around it reopens there.
        let mut placed_top = gpu_orb::ORB_TOP.load(std::sync::atomic::Ordering::Relaxed);
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                frame = frame.wrapping_add(1);
                let offered = state.lock().unwrap().held.clone();
                let (phase, level, hands_free, note, draft) = {
                    let s = state.lock().unwrap();
                    let talking = matches!(s.phase, Phase::Listening | Phase::Transcribing)
                        || s.draft_until.is_some_and(|t| t > Instant::now());
                    (s.phase, s.level, s.hands_free, s.toast.clone(), if talking { s.draft.clone() } else { String::new() })
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
                    let top = gpu_orb::ORB_TOP.load(std::sync::atomic::Ordering::Relaxed);
                    if top != placed_top {
                        placed_top = top;
                        let close = |h: gpui::AnyWindowHandle, cx: &mut App| {
                            let _ = h.update(cx, |_, window, _| window.remove_window());
                        };
                        if let Some((h, _)) = caption.take() {
                            close(h.into(), cx);
                            caption_shown = false;
                        }
                        if let Some((h, _)) = toast.take() {
                            close(h.into(), cx);
                        }
                        if let Some(h) = hover_tip.take() {
                            close(h.into(), cx);
                        }
                        if let Some((h, _)) = held.take() {
                            close(h.into(), cx);
                        }
                        if let Some(h) = hotspot.take() {
                            close(h.into(), cx);
                        }
                        for h in button_windows.drain(..) {
                            close(h.into(), cx);
                        }
                    }
                    // Other apps put their own cursor back as the mouse moves: keep the hand.
                    if hovering {
                        ui::hand_cursor(true);
                    }
                    match (hovering, hover_since) {
                        (true, None) => hover_since = Some(Instant::now()),
                        (false, Some(_)) => {
                            hover_since = None;
                            ui::hand_cursor(false);
                            if let Some(h) = hover_tip.take() {
                                let _ = h.update(cx, |_, window, _| window.remove_window());
                            }
                        }
                        (true, Some(since)) if hover_tip.is_none() && toast.is_none() && since.elapsed() > HOVER_TIP_AFTER => {
                            if let Some(centre) = orb.centre() {
                                let view = ui::Toast { lead: "Dictate".into(), text: String::new(), key: Some("fn") };
                                hover_tip = ui::open_toast(cx, centre, 34., view);
                            }
                        }
                        _ => {}
                    }
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
                    let fresh = note.as_ref().filter(|(_, _, at)| at.elapsed() < TOAST_FOR);
                    let shown = toast.as_ref().map(|(_, at)| *at);
                    if fresh.map(|(_, _, at)| *at) != shown {
                        if let Some((h, _)) = toast.take() {
                            let _ = h.update(cx, |_, window, _| window.remove_window());
                        }
                        if let (Some((lead, text, at)), Some(centre)) = (fresh, orb.centre()) {
                            // Just above the resting dot; above the whole orb while it's awake.
                            let above = if phase == Phase::Ready { 28. } else { 74. };
                            let view = ui::Toast { lead: lead.clone(), text: text.clone(), key: None };
                            toast = ui::open_toast(cx, centre, above, view).map(|h| (h, *at));
                        }
                    }
                    // Updated on disk (brew upgrade swaps the app, not the running copy):
                    // at rest, start the new version. Looked at every five seconds.
                    // Not while the welcome window is up: that's someone in the middle of something.
                    let welcoming = cx.windows().iter().any(|w| w.downcast::<onboarding::Onboarding>().is_some());
                    if frame.is_multiple_of(300) && phase == Phase::Ready && !welcoming && let Some(app) = replaced_bundle() {
                        store::log(&format!("updated on disk: restarting into {}", app.display()));
                        let _ = std::process::Command::new("/bin/sh")
                            .arg("-c")
                            .arg("sleep 1; /usr/bin/open \"$0\"")
                            .arg(&app)
                            .spawn();
                        cx.quit();
                    }
                    if words_closed.replace(false) {
                        worker::SHOW_WORDS.store(false, std::sync::atomic::Ordering::Relaxed);
                        store::save_settings(&store::Settings { show_words: false, ..store::load_settings() });
                        let mut s = state.lock().unwrap();
                        s.draft.clear();
                        s.draft_until = None;
                        s.toast = Some(("Live words off".into(), "Settings turns them back on".into(), Instant::now()));
                    }
                    // A held take: shown until copied, a new take, or HELD_FOR.
                    let fresh = offered.as_ref().filter(|(_, at)| at.elapsed() < HELD_FOR && phase != Phase::Listening);
                    let copy = held_clicked.replace(false);
                    if held.as_ref().map(|(_, at)| *at) != fresh.map(|(_, at)| *at) || copy {
                        if let Some((h, _)) = held.take() {
                            let _ = h.update(cx, |_, window, _| window.remove_window());
                        }
                        if copy {
                            if let Some((text, _)) = &offered
                                && let Ok(mut clipboard) = arboard::Clipboard::new()
                            {
                                let _ = clipboard.set_text(text.clone());
                            }
                            let mut s = state.lock().unwrap();
                            s.held = None;
                            s.toast = Some(("Copied".into(), String::new(), Instant::now()));
                        } else if let (Some((text, at)), Some(centre)) = (fresh, orb.centre()) {
                            let view = ui::Held { text: text.clone(), clicked: held_clicked.clone() };
                            held = ui::open_held(cx, centre, 70., view).map(|h| (h, *at));
                        }
                    }
                    // The words so far, above the orb; redrawn only when they change.
                    if caption.is_none() && !draft.is_empty() && let Some(centre) = orb.centre() {
                        let view = ui::Caption { text: String::new(), closed: words_closed.clone() };
                        caption = ui::open_caption(cx, centre, 140., view).map(|h| (h, String::new()));
                        if let Some((h, _)) = &caption {
                            let _ = h.update(cx, |_, window, _| ui::fade(window, false));
                        }
                    }
                    if let Some((h, shown)) = &mut caption {
                        if !draft.is_empty() && *shown != draft {
                            let text = draft.clone();
                            let _ = h.update(cx, |c, window, _| {
                                c.text = text;
                                window.refresh();
                            });
                            *shown = draft.clone();
                        }
                        if caption_shown == draft.is_empty() {
                            caption_shown = !draft.is_empty();
                            let _ = h.update(cx, |_, window, _| ui::fade(window, caption_shown));
                        }
                    }
                    for ((_, _, cmd), clicked) in buttons.iter().zip(&pressed) {
                        if clicked.replace(false) {
                            let _ = main_tx.send(cmd());
                        }
                    }
                    // Welcome closed before the microphone step: open it anyway.
                    if cx.windows().iter().all(|w| w.downcast::<onboarding::Onboarding>().is_none()) {
                        let mut s = state.lock().unwrap();
                        s.mic_go = true;
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
                        Some(tray::Action::Settings) => {
                            ui::open_settings(cx, main_state.clone(), main_tx.clone());
                            cx.activate(true);
                        }
                        Some(tray::Action::PasteLast) => {
                            let last = main_state.lock().unwrap().history.last().map(|e| e.text.clone());
                            if let Some(text) = last {
                                // Once the menu has closed and the app you were in has focus again.
                                std::thread::spawn(move || {
                                    std::thread::sleep(Duration::from_millis(250));
                                    let _ = paste::paste(&text);
                                });
                            }
                        }
                        Some(tray::Action::UseModel(id)) => {
                            let _ = main_tx.send(Cmd::UseModel(model::Choice::Listed(id)));
                        }
                        Some(tray::Action::Quit) => cx.quit(),
                        None => {}
                    }
                    // The tick follows the model in use, however it was chosen.
                    if let Some(t) = &tray {
                        let current = match &main_state.lock().unwrap().model {
                            model::Choice::Listed(id) => Some(*id),
                            model::Choice::Folder(_) => None,
                        };
                        if shown_model.as_ref() != Some(&current) {
                            t.show_model(current);
                            shown_model = Some(current);
                        }
                    }
                    // Only the welcome window's orb needs a steady frame. The main window
                    // redraws itself on change; toasts animate themselves; the dot's
                    // target and the orb's buttons are still.
                    if frame.is_multiple_of(2) {
                        for w in cx.windows().into_iter().filter_map(|w| w.downcast::<onboarding::Onboarding>()) {
                            let _ = w.update(cx, |_, window, _| window.refresh());
                        }
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
