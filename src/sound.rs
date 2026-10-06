//! The soft click when a take starts and ends: two tiny tones made here and
//! played with NSSound, so there are no sound files to ship.

#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use std::sync::OnceLock;

use cocoa::base::id;
use objc::{class, msg_send, sel, sel_impl};

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.18;

#[derive(Clone, Copy)]
pub enum Cue {
    Start,
    Stop,
}

/// NSSound pointers, made once and kept for the life of the app.
struct Sounds {
    start: usize,
    stop: usize,
}

static SOUNDS: OnceLock<Sounds> = OnceLock::new();

pub fn play(cue: Cue) {
    let sounds = SOUNDS.get_or_init(|| Sounds {
        start: make(&tone(740.0, 990.0)),
        stop: make(&tone(990.0, 660.0)),
    });
    let sound = match cue {
        Cue::Start => sounds.start,
        Cue::Stop => sounds.stop,
    } as id;
    if sound.is_null() {
        return;
    }
    unsafe {
        let _: () = msg_send![sound, stop];
        let _: () = msg_send![sound, play];
    }
}

/// A 70 ms sine gliding from one pitch to another, softened at both ends.
fn tone(from: f32, to: f32) -> Vec<u8> {
    let count = (RATE as f32 * 0.07) as usize;
    let mut phase = 0f32;
    let samples = (0..count).map(|i| {
        let t = i as f32 / count as f32;
        phase += std::f32::consts::TAU * (from + (to - from) * t) / RATE as f32;
        let envelope = (t * 12.0).min(1.0) * (1.0 - t).powi(2);
        (phase.sin() * envelope * i16::MAX as f32) as i16
    });
    let spec = hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut wav = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut wav, spec).expect("wav header");
    for s in samples {
        writer.write_sample(s).expect("wav sample");
    }
    writer.finalize().expect("wav finish");
    wav.into_inner()
}

fn make(wav: &[u8]) -> usize {
    unsafe {
        let data: id = msg_send![class!(NSData), alloc];
        let data: id = msg_send![data, initWithBytes: wav.as_ptr() length: wav.len()];
        let sound: id = msg_send![class!(NSSound), alloc];
        let sound: id = msg_send![sound, initWithData: data];
        let _: () = msg_send![data, release];
        if !sound.is_null() {
            let _: () = msg_send![sound, setVolume: VOLUME];
        }
        sound as usize
    }
}
