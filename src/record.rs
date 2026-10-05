//! Microphone capture. The stream stays open so a press starts instantly, and
//! the last moment before the press is kept so the first word isn't clipped.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat};

const PRE_ROLL_SECS: f32 = 0.3;

pub struct Recorder {
    _stream: cpal::Stream,
    shared: Arc<Shared>,
    pub sample_rate: u32,
    pub device_name: String,
}

struct Shared {
    recording: AtomicBool,
    /// Loudness of the latest audio, f32 bits, for the level meter.
    level: AtomicU32,
    buffer: Mutex<Vec<f32>>,
    pre_roll: usize,
}

impl Recorder {
    /// Names of the microphones this machine has.
    pub fn devices() -> Vec<String> {
        let host = cpal::default_host();
        host.input_devices()
            .map(|devices| devices.filter_map(|d| d.description().ok().map(|n| n.to_string())).collect())
            .unwrap_or_default()
    }

    /// Open the named microphone, or the system default.
    pub fn open(name: Option<&str>) -> Result<Self, String> {
        let host = cpal::default_host();
        let named = name.and_then(|want| {
            host.input_devices().ok()?.find(|d| {
                d.description().map(|n| n.to_string() == want).unwrap_or(false)
            })
        });
        let device = match named {
            Some(device) => device,
            None => host.default_input_device().ok_or("no microphone found")?,
        };
        let device_name = device
            .description()
            .map(|d| d.to_string())
            .unwrap_or_else(|_| "microphone".into());
        let config = device
            .default_input_config()
            .map_err(|e| format!("microphone config: {e}"))?;
        let sample_rate = config.sample_rate() as u32;
        let channels = config.channels() as usize;
        let shared = Arc::new(Shared {
            recording: AtomicBool::new(false),
            level: AtomicU32::new(0),
            buffer: Mutex::new(Vec::new()),
            pre_roll: (sample_rate as f32 * PRE_ROLL_SECS) as usize,
        });

        let err = |e| eprintln!("audio: {e}");
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config.into(), channels, shared.clone(), err),
            SampleFormat::I16 => build::<i16>(&device, config.into(), channels, shared.clone(), err),
            SampleFormat::I32 => build::<i32>(&device, config.into(), channels, shared.clone(), err),
            other => return Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|e| format!("start microphone: {e}"))?;
        Ok(Self { _stream: stream, shared, sample_rate, device_name })
    }

    /// 0..1, roughly how loud the microphone is right now.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    pub fn begin(&self) {
        self.shared.recording.store(true, Ordering::SeqCst);
    }

    /// Seconds recorded so far in this take.
    pub fn seconds(&self) -> f32 {
        if !self.shared.recording.load(Ordering::Relaxed) {
            return 0.0;
        }
        self.shared.buffer.lock().unwrap().len() as f32 / self.sample_rate as f32
    }

    /// Stop and hand back the clip (mono, at `sample_rate`).
    pub fn end(&self) -> Vec<f32> {
        self.shared.recording.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.shared.buffer.lock().unwrap())
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    shared: Arc<Shared>,
    err: impl FnMut(cpal::Error) + Send + 'static,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: FromSample<T>,
{
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &_| {
                let mut buffer = shared.buffer.lock().unwrap();
                let start = buffer.len();
                for frame in data.chunks(channels) {
                    let sum: f32 = frame.iter().map(|s| s.to_sample::<f32>()).sum();
                    buffer.push(sum / channels as f32);
                }
                let fresh = &buffer[start..];
                if !fresh.is_empty() {
                    let rms = (fresh.iter().map(|x| x * x).sum::<f32>() / fresh.len() as f32).sqrt();
                    shared.level.store((rms * 8.0).min(1.0).to_bits(), Ordering::Relaxed);
                }
                if !shared.recording.load(Ordering::Relaxed) && buffer.len() > shared.pre_roll {
                    let excess = buffer.len() - shared.pre_roll;
                    buffer.drain(..excess);
                }
            },
            err,
            None,
        )
        .map_err(|e| format!("open microphone: {e}"))
}
