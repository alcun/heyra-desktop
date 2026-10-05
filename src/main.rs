//! Heyra: hold fn (or squeeze a TING) and talk; let go and the words are
//! pasted where your cursor is. Speech is turned into text on this machine with
//! Parakeet, the model Heyra uses. Nothing leaves it.

mod engine;
mod hotkey;
mod paste;
mod record;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    App, Application, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};

use engine::{Engine, Parakeet};
use hotkey::Key;

#[derive(Clone)]
struct Status {
    line: SharedString,
    last: SharedString,
    listening: bool,
}

type Shared = Arc<Mutex<Status>>;

fn set(status: &Shared, line: impl Into<SharedString>) {
    status.lock().unwrap().line = line.into();
}

/// Loads the model, then turns key presses into recordings into pasted text.
fn run(status: Shared, keys: mpsc::Receiver<Key>) {
    set(&status, "Loading Parakeet…");
    let started = std::time::Instant::now();
    let mut engine: Box<dyn Engine> = match Parakeet::load(&Parakeet::default_dir()) {
        Ok(engine) => Box::new(engine),
        Err(e) => return set(&status, e),
    };
    let load_time = started.elapsed();
    let recorder = match record::Recorder::open() {
        Ok(recorder) => recorder,
        Err(e) => return set(&status, e),
    };
    let ready = format!(
        "Ready: hold fn and talk · {} · model loaded in {:.1}s",
        recorder.device_name,
        load_time.as_secs_f32()
    );
    set(&status, ready.clone());

    let mut down = false;
    for key in keys {
        match key {
            Key::Down if !down => {
                down = true;
                recorder.begin();
                let mut s = status.lock().unwrap();
                s.line = "Listening…".into();
                s.listening = true;
            }
            Key::Up if down => {
                down = false;
                let samples = recorder.end();
                status.lock().unwrap().listening = false;
                let seconds = samples.len() as f32 / recorder.sample_rate as f32;
                if seconds < 0.4 {
                    set(&status, ready.clone());
                    continue;
                }
                set(&status, "Transcribing…");
                let started = std::time::Instant::now();
                let text = engine.transcribe(recorder.sample_rate, &samples);
                let took = started.elapsed().as_secs_f32();
                if !text.is_empty() {
                    if let Err(e) = paste::paste(&text) {
                        eprintln!("paste: {e}");
                    }
                }
                let mut s = status.lock().unwrap();
                s.last = text.into();
                s.line = format!("{seconds:.1}s of speech → text in {took:.2}s").into();
            }
            _ => {}
        }
    }
}

struct Pill {
    status: Shared,
}

impl Render for Pill {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.status.lock().unwrap().clone();
        let dot = if s.listening { rgb(0xe5484d) } else { rgb(0x46a758) };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .size_full()
            .p_4()
            .bg(rgb(0x1c1c1f))
            .text_color(rgb(0xededef))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().size_3().rounded_full().bg(dot))
                    .child(div().text_sm().child(s.line)),
            )
            .child(div().text_color(rgb(0xa0a0a8)).child(if s.last.is_empty() {
                SharedString::from("Your last words show up here.")
            } else {
                s.last
            }))
    }
}

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
    let status: Shared = Arc::new(Mutex::new(Status {
        line: "Starting…".into(),
        last: SharedString::default(),
        listening: false,
    }));

    let (tx, rx) = mpsc::channel();
    {
        let status = status.clone();
        std::thread::spawn(move || {
            if let Err(e) = hotkey::listen(tx) {
                set(&status, e);
            }
        });
    }
    {
        let status = status.clone();
        std::thread::spawn(move || run(status, rx));
    }

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(460.), px(140.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|cx: &mut Context<Pill>| {
                    // Redraw a few times a second to pick up status from the worker threads.
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_millis(100)).await;
                            if this.update(cx, |_, cx| cx.notify()).is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                    Pill { status }
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
