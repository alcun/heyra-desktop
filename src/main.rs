//! Heyra: hold fn (or squeeze a TING) and talk; let go and the words are pasted
//! where your cursor is. Speech is turned into text on this machine with
//! Parakeet, the model Heyra uses. Nothing leaves it.

mod engine;
mod hotkey;
mod model;
mod orb;
mod paste;
mod record;
mod setup;
mod store;
mod tray;
mod ui;
mod worker;

use std::borrow::Cow;
use std::sync::mpsc;
use std::time::Duration;

use gpui::{App, Application, WindowHandle};

use engine::{Engine, Parakeet};
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
    let text = engine.transcribe(spec.sample_rate, &mono);
    println!("{text}");
    eprintln!(
        "load {loaded:.2}s, {:.1}s audio in {:.2}s",
        mono.len() as f32 / spec.sample_rate as f32,
        started.elapsed().as_secs_f32()
    );
}

/// `heyra --preview-gauge`: the edge gauge alone with a fake voice, for design work.
fn preview_gauge(state: worker::Shared) {
    {
        let state = state.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                let t = start.elapsed().as_secs_f32();
                let mut s = state.lock().unwrap();
                s.phase = if t % 8.0 < 6.0 { Phase::Listening } else { Phase::Transcribing };
                s.level = ((t * 3.1).sin() * 0.5 + 0.5) * ((t * 0.7).sin() * 0.35 + 0.55);
                drop(s);
                std::thread::sleep(Duration::from_millis(30));
            }
        });
    }
    Application::new().run(move |cx: &mut App| {
        ui::open_pill(cx, state.clone());
        cx.spawn(async move |cx| loop {
            cx.background_executor().timer(Duration::from_millis(33)).await;
            if cx.update(|cx| cx.refresh_windows()).is_err() {
                break;
            }
        })
        .detach();
    });
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--file" {
        return transcribe_file(&args[2]);
    }
    if args.len() == 4 && args[1] == "--orb-png" {
        // `heyra --orb-png out.png <voice 0..1 | writing>`: one orb frame, timed.
        let style = orb::Style {
            voice: args[3].parse().unwrap_or(0.0),
            writing: args[3] == "writing",
        };
        let size = 300;
        let started = std::time::Instant::now();
        let mut bgra = orb::render(size, 2.3, &style);
        eprintln!("frame in {:.1} ms", started.elapsed().as_secs_f32() * 1000.0);
        for px in bgra.chunks_mut(4) {
            px.swap(0, 2);
        }
        image::RgbaImage::from_raw(size as u32, size as u32, bgra).unwrap().save(&args[2]).unwrap();
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
    if args.len() == 2 && args[1] == "--preview-gauge" {
        return preview_gauge(state);
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

        // Redraw ~30 times a second, and show the pill while listening or writing.
        let state = state.clone();
        cx.spawn(async move |cx| {
            let mut pill: Option<WindowHandle<ui::Pill>> = None;
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let phase = state.lock().unwrap().phase;
                let busy = matches!(phase, Phase::Listening | Phase::Transcribing);
                let ok = cx.update(|cx| {
                    match tray.as_ref().and_then(|t| t.poll()) {
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
                    match (&pill, busy) {
                        (None, true) => pill = ui::open_pill(cx, state.clone()),
                        (Some(handle), false) => {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                            pill = None;
                        }
                        _ => {}
                    }
                    cx.refresh_windows();
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
