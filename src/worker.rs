//! The background worker: loads the model, owns the microphone, and turns
//! push-to-talk presses into pasted text. The UI only reads `State`.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{Engine, Parakeet};
use crate::hotkey::Key;
use crate::record::Recorder;
use crate::store::{self, Entry};

#[derive(Clone, Copy, PartialEq)]
pub enum Phase {
    Loading,
    Ready,
    Listening,
    Transcribing,
    Error,
}

pub struct State {
    pub phase: Phase,
    pub message: String,
    pub level: f32,
    pub history: Vec<Entry>,
    pub devices: Vec<String>,
    pub mic: Option<String>,
    pub mic_in_use: String,
    pub sample_rate: u32,
    /// Something the user must fix before push-to-talk works (e.g. a permission).
    pub blocker: Option<String>,
}

pub type Shared = Arc<Mutex<State>>;

pub enum Cmd {
    Key(Key),
    SetMic(Option<String>),
}

pub fn new_state() -> Shared {
    Arc::new(Mutex::new(State {
        phase: Phase::Loading,
        message: "Starting…".into(),
        level: 0.0,
        history: store::load_history(),
        devices: Recorder::devices(),
        mic: store::load_settings().mic,
        mic_in_use: String::new(),
        sample_rate: 0,
        blocker: None,
    }))
}

fn fail(state: &Shared, message: String) {
    store::log(&format!("error: {message}"));
    let mut s = state.lock().unwrap();
    s.phase = Phase::Error;
    s.message = message;
}

fn open_mic(state: &Shared, name: Option<&str>) -> Option<Recorder> {
    match Recorder::open(name) {
        Ok(recorder) => {
            let mut s = state.lock().unwrap();
            s.mic_in_use = recorder.device_name.clone();
            s.sample_rate = recorder.sample_rate;
            Some(recorder)
        }
        Err(e) => {
            fail(state, e);
            None
        }
    }
}

pub fn run(state: Shared, cmds: Receiver<Cmd>) {
    {
        let mut s = state.lock().unwrap();
        s.message = "Loading the speech model…".into();
    }
    let mut engine: Box<dyn Engine> = match Parakeet::load(&Parakeet::default_dir()) {
        Ok(engine) => Box::new(engine),
        Err(e) => return fail(&state, e),
    };
    let mic = state.lock().unwrap().mic.clone();
    let Some(mut recorder) = open_mic(&state, mic.as_deref()) else { return };
    let ready = |state: &Shared| {
        let mut s = state.lock().unwrap();
        s.phase = Phase::Ready;
        s.message = "Hold fn and talk".into();
        s.level = 0.0;
    };
    ready(&state);

    let mut down = false;
    loop {
        let cmd = match cmds.recv_timeout(Duration::from_millis(30)) {
            Ok(cmd) => Some(cmd),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        if down {
            state.lock().unwrap().level = recorder.level();
        }
        if let Some(Cmd::Key(key)) = &cmd {
            store::log(&format!("key {key:?}"));
        }
        match cmd {
            Some(Cmd::Key(Key::Down)) if !down => {
                down = true;
                recorder.begin();
                let mut s = state.lock().unwrap();
                s.phase = Phase::Listening;
                s.message = "Recording".into();
            }
            Some(Cmd::Key(Key::Up)) if down => {
                down = false;
                let samples = recorder.end();
                let secs = samples.len() as f32 / recorder.sample_rate as f32;
                let peak = samples.iter().fold(0f32, |m, x| m.max(x.abs()));
                store::log(&format!("clip {secs:.1}s peak {peak:.3}"));
                if store::load_settings().keep_last_clip {
                    store::save_clip(recorder.sample_rate, &samples);
                }
                if secs < 0.4 {
                    ready(&state);
                    continue;
                }
                {
                    let mut s = state.lock().unwrap();
                    s.phase = Phase::Transcribing;
                    s.message = "Writing".into();
                    s.level = 0.0;
                }
                let started = Instant::now();
                let raw = engine.transcribe(recorder.sample_rate, &samples);
                let text = store::apply_dictionary(&raw, &store::load_dictionary());
                let took = started.elapsed().as_secs_f32();
                store::log(&format!("transcribed in {took:.2}s, {} chars", text.len()));
                if !text.is_empty() {
                    match crate::paste::paste(&text) {
                        Ok(()) => store::log("pasted"),
                        Err(e) => store::log(&format!("paste failed: {e}")),
                    }
                    let entry = Entry { at: store::now(), text, secs, took };
                    store::append_history(&entry);
                    state.lock().unwrap().history.push(entry);
                }
                ready(&state);
            }
            Some(Cmd::SetMic(name)) if !down => {
                drop(recorder);
                store::save_settings(&store::Settings { mic: name.clone(), ..store::load_settings() });
                state.lock().unwrap().mic = name.clone();
                recorder = match open_mic(&state, name.as_deref()) {
                    Some(r) => r,
                    None => match open_mic(&state, None) {
                        Some(r) => r,
                        None => return,
                    },
                };
                ready(&state);
            }
            _ => {}
        }
    }
}
