//! The background worker: loads the model, owns the microphone, and turns
//! push-to-talk presses into pasted text. The UI only reads `State`.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::atomic::{AtomicBool, Ordering};
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
    pub devices: Vec<crate::mics::Mic>,
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
    /// The microphone may be opened. On the first run it waits for the welcome
    /// window, so macOS's question comes after Heyra has said why.
    pub mic_go: bool,
    /// A TING's disk is mounted (plugged in over USB-C): whether it's set up for Heyra.
    pub ting_disk: Option<bool>,
    /// How the last TING setup went, for Settings.
    pub ting_note: Option<String>,
    /// A short note shown above the dot: a quiet lead word, the news, and when.
    pub toast: Option<(String, String, Instant)>,
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
        devices: crate::mics::list(),
        mic: store::load_settings().mic,
        mic_in_use: String::new(),
        sample_rate: 0,
        progress: None,
        checks: crate::setup::check(),
        blocker: None,
        hands_free: false,
        ting_heard: false,
        mic_go: store::load_settings().onboarded || crate::setup::check().mic != crate::setup::Mic::Unknown,
        toast: None,
        ting_disk: None,
        ting_note: None,
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
/// How often to look for microphones plugged in or taken out.
const MIC_SCAN: Duration = Duration::from_secs(2);
/// A press shorter than this is a tap, not push-to-talk.
const TAP: Duration = Duration::from_millis(300);
/// Two taps this close together start hands-free.
const DOUBLE_TAP: Duration = Duration::from_millis(500);

/// The Settings switch for muting the Mac while a take records.
pub static MUTE_WHILE_TALKING: AtomicBool = AtomicBool::new(false);

/// The Mac's sound, muted for a take and put back as it was.
#[derive(Default)]
struct Mute {
    on: bool,
    /// It was already muted, so leave it muted after.
    was_muted: bool,
}

impl Mute {
    fn engage(&mut self) {
        if !self.on {
            self.on = true;
            self.was_muted = crate::mics::mute_output(true).unwrap_or(true);
        }
    }

    fn restore(&mut self) {
        if std::mem::take(&mut self.on) && !self.was_muted {
            crate::mics::mute_output(false);
        }
    }
}

/// Stop recording, transcribe the take and keep it in History; paste it unless `paste` is off.
fn finish(state: &Shared, engine: &mut dyn Engine, recorder: &Recorder, paste: bool, mute: &mut Mute) {
    let samples = recorder.end();
    let secs = samples.len() as f32 / recorder.sample_rate as f32;
    let peak = samples.iter().fold(0f32, |m, x| m.max(x.abs()));
    store::log(&format!("clip {secs:.1}s peak {peak:.3}"));
    if store::load_settings().keep_last_clip {
        store::save_clip(recorder.sample_rate, &samples);
    }
    mute.restore(); // before the sound, so it's heard
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
    let (text, enter) = match store::press_enter(&text) {
        Some(before) => (before, paste),
        None => (text, false),
    };
    let took = started.elapsed().as_secs_f32();
    store::log(&format!("transcribed in {took:.2}s, {} chars", text.len()));
    if text.is_empty() && !enter {
        state.lock().unwrap().toast = Some(("Didn't catch that".into(), String::new(), Instant::now()));
    }
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
    if enter {
        let _ = crate::paste::tap(crate::paste::RETURN, false);
    }
}

pub fn run(state: Shared, cmds: Receiver<Cmd>) {
    {
        let mut s = state.lock().unwrap();
        s.message = "Loading the speech model…".into();
    }
    // The model downloads and loads on its own thread, so the microphone can open
    // (and the welcome window's orb follow your voice) as soon as it's allowed.
    let loader = {
        let state = state.clone();
        std::thread::spawn(move || -> Result<Box<dyn Engine>, String> {
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
                    got?
                }
            };
            state.lock().unwrap().message = "Loading the speech model…".into();
            Ok(Box::new(Parakeet::load(&dir)?))
        })
    };
    let mut recorder: Option<Recorder> = None;
    while recorder.is_none() || !loader.is_finished() {
        if recorder.is_none() && state.lock().unwrap().mic_go {
            let mic = state.lock().unwrap().mic.clone();
            recorder = Some(match open_mic(&state, mic.as_deref()) {
                Some(r) => r,
                None => return,
            });
        }
        if let Some(r) = &recorder {
            state.lock().unwrap().level = r.level();
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut recorder = recorder.unwrap();
    let mut engine = match loader.join() {
        Ok(Ok(engine)) => engine,
        Ok(Err(e)) => return fail(&state, e),
        Err(_) => return fail(&state, "the speech model crashed while loading".into()),
    };
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
    let mut mute = Mute::default();
    MUTE_WHILE_TALKING.store(store::load_settings().mute_while_talking, Ordering::Relaxed);
    // Microphones come and go: a new one is switched to, as other dictation apps do.
    let mut known: Vec<String> = state.lock().unwrap().devices.iter().map(|m| m.name.clone()).collect();
    let mut scanned = Instant::now();
    // A switch Heyra made by itself is said above the dot once it has happened.
    let mut announce = false;
    loop {
        match cmds.recv_timeout(Duration::from_millis(30)) {
            Ok(cmd) => queue.push(cmd),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !recording && scanned.elapsed() > MIC_SCAN {
            scanned = Instant::now();
            let mics = crate::mics::list();
            let names: Vec<String> = mics.iter().map(|m| m.name.clone()).collect();
            if names != known {
                // Only a mic plugged in by cable takes over; a phone or headphones coming
                // into range don't. An adapter with both: the line-in is where a TING is.
                let added: Vec<&crate::mics::Mic> = mics
                    .iter()
                    .filter(|m| !known.contains(&m.name) && m.kind == crate::mics::Kind::Wired)
                    .collect();
                let pick = added.iter().find(|m| m.name.to_lowercase().contains("line in")).or(added.first());
                if let Some(mic) = pick {
                    store::log(&format!("new microphone: {}", mic.name));
                    announce = true;
                    queue.push(Cmd::SetMic(Some(mic.name.clone())));
                } else if !names.contains(&recorder.device_name) {
                    store::log(&format!("microphone gone: {}", recorder.device_name));
                    announce = true;
                    queue.push(Cmd::SetMic(None));
                }
                known = names;
                state.lock().unwrap().devices = mics;
            }
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
            let mut s = state.lock().unwrap();
            if !s.ting_heard {
                s.ting_heard = true;
                s.toast = Some(("Connected".into(), "FX mic".into(), Instant::now()));
            }
        }
        // The level shows in Settings too, so a mic can be checked without talking to an app.
        state.lock().unwrap().level = recorder.level();
        if recording {
            if recorder.seconds() > MAX_TAKE_SECS {
                recording = false;
                ignore_up = !hands_free;
                hands_free = false;
                finish(&state, engine.as_mut(), &recorder, true, &mut mute);
                ready(&state);
                continue;
            }
        }
        for cmd in std::mem::take(&mut queue) {
        match cmd {
            Cmd::Press(code, command) => {
                let _ = crate::paste::tap(code, command);
            }
            Cmd::Key(Key::Escape) if recording => {
                recording = false;
                hands_free = false;
                recorder.end();
                ready(&state);
            }
            Cmd::Key(Key::Cancel) if recording && !hands_free => {
                recording = false;
                recorder.end();
                ready(&state);
            }
            cmd @ (Cmd::Discard | Cmd::Finish) if hands_free => {
                recording = false;
                hands_free = false;
                finish(&state, engine.as_mut(), &recorder, matches!(cmd, Cmd::Finish), &mut mute);
                ready(&state);
            }
            Cmd::Key(Key::Down) if hands_free => {
                recording = false;
                hands_free = false;
                ignore_up = true;
                finish(&state, engine.as_mut(), &recorder, true, &mut mute);
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
                finish(&state, engine.as_mut(), &recorder, true, &mut mute);
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
                if std::mem::take(&mut announce) {
                    state.lock().unwrap().toast = Some(("Using".into(), recorder.device_name.clone(), Instant::now()));
                }
                ready(&state);
            }
            _ => {}
        }
        }
        // A tap never mutes; a take that's really talking does.
        if recording && MUTE_WHILE_TALKING.load(Ordering::Relaxed) && pressed_at.elapsed() > TAP {
            mute.engage();
        } else if !recording {
            mute.restore();
        }
        crate::hotkey::set_taking(recording);
    }
}
