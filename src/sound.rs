//! The soft thock when a take starts and ends: two tiny sounds made here and
//! played with NSSound, so there are no sound files to ship.

#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use std::sync::OnceLock;

use cocoa::base::id;
use objc::{class, msg_send, sel, sel_impl};

const RATE: u32 = 44_100;
const VOLUME: f32 = 0.45;

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
        start: make(&thock(230.0, 1.06)),
        stop: make(&thock(245.0, 0.94)),
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

/// A felt mallet on hollow wood, kept small: a low note with a faint overtone
/// and a sub, a soft low-passed tap at the strike, about 50 ms of ring. The
/// pitch bends by `glide` over the first 50 ms, up to start and down to stop.
fn thock(base: f32, glide: f32) -> Vec<u8> {
    // (frequency ratio, loudness, decay in seconds)
    const MODES: [(f32, f32, f32); 3] = [(1.0, 1.0, 0.045), (2.31, 0.12, 0.018), (0.5, 0.15, 0.04)];
    let count = (RATE as f32 * 0.2) as usize;
    let mut phases = [0f32; 3];
    let mut noise = 0x2545_f491u32;
    let mut tap = 0f32;
    let raw: Vec<f32> = (0..count)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let pitch = base * (1.0 + (glide - 1.0) * (t / 0.05).min(1.0));
            let mut x = 0.0;
            for (phase, &(ratio, gain, decay)) in phases.iter_mut().zip(&MODES) {
                *phase += std::f32::consts::TAU * pitch * ratio / RATE as f32;
                x += phase.sin() * gain * (-t / decay).exp();
            }
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            tap += 0.05 * (noise as f32 / u32::MAX as f32 * 2.0 - 1.0 - tap);
            x += tap * 0.36 * (-t / 0.0015).exp();
            let attack = 0.5 - 0.5 * (std::f32::consts::PI * (t / 0.002).min(1.0)).cos();
            x * attack
        })
        .collect();
    let peak = raw.iter().fold(0f32, |m, x| m.max(x.abs()));
    let samples = raw.iter().map(|x| (x / peak * 0.9 * i16::MAX as f32) as i16);
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
