//! Heyra: hold fn (or squeeze a TING) and talk; let go and the words are pasted
//! where your cursor is. Speech is turned into text on this machine with
//! Parakeet, the model Heyra uses. Nothing leaves it.

mod engine;
mod hotkey;
mod paste;
mod record;
mod store;
mod ui;
mod worker;

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
    let mut engine = Parakeet::load(&Parakeet::default_dir()).expect("load model");
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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 3 && args[1] == "--file" {
        return transcribe_file(&args[2]);
    }

    let state = worker::new_state();
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
        ui::open_main(cx, state.clone(), tx.clone());

        // Redraw ~30 times a second, and show the pill while listening or writing.
        let state = state.clone();
        cx.spawn(async move |cx| {
            let mut pill: Option<WindowHandle<ui::Pill>> = None;
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let phase = state.lock().unwrap().phase;
                let busy = matches!(phase, Phase::Listening | Phase::Transcribing);
                let ok = cx.update(|cx| {
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
