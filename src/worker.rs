//! The background worker: loads the model, owns the microphone, and turns
//! push-to-talk presses into pasted text. The UI only reads `State`.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{Engine, Parakeet};
use crate::hotkey::Key;
use crate::record::Recorder;
use crate::sound::Cue;
use crate::ting::Event as TingEvent;
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
    /// Model download progress, 0..1, while downloading.
    pub progress: Option<f32>,
    pub checks: crate::setup::Checks,
    /// Something the user must fix before push-to-talk works (e.g. a permission).
    pub blocker: Option<String>,
    /// A double-tap take is running; the orb shows an X and a tick.
    pub hands_free: bool,
    /// A TING has been heard on the microphone in use.
    pub ting_heard: bool,
}

pub type Shared = Arc<Mutex<State>>;

pub enum Cmd {
    Key(Key),
    SetMic(Option<String>),
    /// The orb's X: end the hands-free take and keep it in History, but don't paste it.
    Discard,
    /// The orb's tick: end the hands-free take and paste it, as a tap of fn does.
    Finish,
    /// Press a key (with cmd when set): the TING's buttons.
    Press(u16, bool),
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
        progress: None,
        checks: crate::setup::check(),
        blocker: None,
        hands_free: false,
        ting_heard: false,
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

/// Longest single take; a stuck key or a forgotten hands-free take ends here.
const MAX_TAKE_SECS: f32 = 30.0 * 60.0;
/// A press shorter than this is a tap, not push-to-talk.
const TAP: Duration = Duration::from_millis(300);
/// Two taps this close together start hands-free.
const DOUBLE_TAP: Duration = Duration::from_millis(500);

/// Stop recording, transcribe the take and keep it in History; paste it unless `paste` is off.
fn finish(state: &Shared, engine: &mut dyn Engine, recorder: &Recorder, paste: bool) {
    let samples = recorder.end();
    let secs = samples.len() as f32 / recorder.sample_rate as f32;
    let peak = samples.iter().fold(0f32, |m, x| m.max(x.abs()));
    store::log(&format!("clip {secs:.1}s peak {peak:.3}"));
    if store::load_settings().keep_last_clip {
        store::save_clip(recorder.sample_rate, &samples);
    }
    if secs < 0.4 {
        return;
    }
    crate::sound::play(Cue::Stop);
    {
        let mut s = state.lock().unwrap();
        s.phase = Phase::Transcribing;
        s.message = "Writing".into();
        s.level = 0.0;
    }
    let started = Instant::now();
    let raw = crate::engine::transcribe_long(engine, recorder.sample_rate, &samples);
    let text = store::apply_dictionary(&raw, &store::load_dictionary());
    let took = started.elapsed().as_secs_f32();
    store::log(&format!("transcribed in {took:.2}s, {} chars", text.len()));
    if !text.is_empty() {
        if paste {
            match crate::paste::paste(&text) {
                Ok(()) => store::log("pasted"),
                Err(e) => store::log(&format!("paste failed: {e}")),
            }
        }
        let entry = Entry { at: store::now(), text, secs, took };
        store::append_history(&entry);
        state.lock().unwrap().history.push(entry);
    }
}

pub fn run(state: Shared, cmds: Receiver<Cmd>) {
    {
        let mut s = state.lock().unwrap();
        s.message = "Loading the speech model…".into();
    }
    let dir = match crate::model::find() {
        Some(dir) => dir,
        None => {
            let progress_state = state.clone();
            let got = crate::model::download(move |done, total| {
                let mut s = progress_state.lock().unwrap();
                let mb = |b: u64| b / 1_000_000;
                s.progress = (total > 0).then(|| done as f32 / total as f32);
                s.message = if total > 0 {
                    format!("Downloading the speech model, once: {} of {} MB", mb(done), mb(total))
                } else {
                    format!("Downloading the speech model, once: {} MB", mb(done))
                };
            });
            state.lock().unwrap().progress = None;
            match got {
                Ok(dir) => dir,
                Err(e) => return fail(&state, e),
            }
        }
    };
    state.lock().unwrap().message = "Loading the speech model…".into();
    let mut engine: Box<dyn Engine> = match Parakeet::load(&dir) {
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
        s.hands_free = false;
    };
    ready(&state);

    let mut recording = false;
    let mut hands_free = false;
    // The key-up that follows the tap ending a hands-free take.
    let mut ignore_up = false;
    let mut pressed_at = Instant::now();
    let mut last_tap: Option<Instant> = None;
    let mut queue: Vec<Cmd> = Vec::new();
    loop {
        match cmds.recv_timeout(Duration::from_millis(30)) {
            Ok(cmd) => queue.push(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // A TING's squeeze is push-to-talk, like fn; its buttons press Enter and undo.
        for event in recorder.ting_events() {
            store::log(&format!("ting: {event:?}"));
            queue.push(match event {
                TingEvent::Squeeze => Cmd::Key(Key::Down),
                TingEvent::Release => Cmd::Key(Key::Up),
                TingEvent::Bottom => Cmd::Press(crate::paste::RETURN, false),
                TingEvent::Middle => Cmd::Press(crate::paste::Z, true),
            });
            state.lock().unwrap().ting_heard = true;
        }
        if recording {
            state.lock().unwrap().level = recorder.level();
            if recorder.seconds() > MAX_TAKE_SECS {
                recording = false;
                ignore_up = !hands_free;
                hands_free = false;
                finish(&state, engine.as_mut(), &recorder, true);
                ready(&state);
                continue;
            }
        }
        for cmd in std::mem::take(&mut queue) {
        match cmd {
            Cmd::Press(code, command) => {
                let _ = crate::paste::tap(code, command);
            }
            Cmd::Key(Key::Cancel) if recording && !hands_free => {
                recording = false;
                recorder.end();
                ready(&state);
            }
            cmd @ (Cmd::Discard | Cmd::Finish) if hands_free => {
                recording = false;
                hands_free = false;
                finish(&state, engine.as_mut(), &recorder, matches!(cmd, Cmd::Finish));
                ready(&state);
            }
            Cmd::Key(Key::Down) if hands_free => {
                recording = false;
                hands_free = false;
                ignore_up = true;
                finish(&state, engine.as_mut(), &recorder, true);
                ready(&state);
            }
            Cmd::Key(Key::Down) if !recording => {
                recording = true;
                pressed_at = Instant::now();
                recorder.begin();
                // The second tap of a double-tap already heard the first's sound.
                if !last_tap.is_some_and(|t| t.elapsed() < DOUBLE_TAP) {
                    crate::sound::play(Cue::Start);
                }
                let mut s = state.lock().unwrap();
                s.phase = Phase::Listening;
                s.message = "Recording".into();
            }
            Cmd::Key(Key::Up) if ignore_up => ignore_up = false,
            Cmd::Key(Key::Up) if recording && !hands_free => {
                if pressed_at.elapsed() < TAP {
                    if last_tap.take().is_some_and(|t| t.elapsed() < DOUBLE_TAP) {
                        hands_free = true;
                        let mut s = state.lock().unwrap();
                        s.hands_free = true;
                        s.message = "Hands-free: tap fn to stop".into();
                    } else {
                        last_tap = Some(Instant::now());
                        recording = false;
                        recorder.end();
                        ready(&state);
                    }
                    continue;
                }
                recording = false;
                finish(&state, engine.as_mut(), &recorder, true);
                ready(&state);
            }
            Cmd::SetMic(name) if !recording => {
                drop(recorder);
                store::save_settings(&store::Settings { mic: name.clone(), ..store::load_settings() });
                {
                    let mut s = state.lock().unwrap();
                    s.mic = name.clone();
                    s.ting_heard = false;
                }
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
}
