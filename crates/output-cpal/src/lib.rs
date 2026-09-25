//! The desktop's sound card for `nori-engine`, through cpal: PipeWire or ALSA on Linux, CoreAudio on
//! macOS, WASAPI on Windows. It only opens the device and, on the device's own thread, pulls each
//! buffer from the engine's [`Feed`]; everything else is the engine's.
//!
//! The device is asked for the stream's own rate and channels first, so nothing is resampled when it
//! can take them (PipeWire and CoreAudio take any rate); otherwise it plays at its own and the engine
//! converts. Paused, the stream is stopped, so the device and its thread can sleep.
//!
//! Which device the music goes to is told to the engine when the stream opens and whenever the system
//! moves a stream on the default device elsewhere (cpal reports that on PipeWire, CoreAudio and
//! WASAPI; ALSA cannot tell), with the kind cpal describes it as, so each device can have its own sound.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, DeviceType, ErrorKind, InterfaceType, SampleFormat, Stream, StreamConfig};
use nori_engine::{AudioOutput, DeviceWatch, Feed, OutputFormat, OutputKind};

pub struct CpalOutput {
    /// A device by name, or the system's default output.
    wanted: Option<String>,
    device: Option<Device>,
    config: Option<(StreamConfig, SampleFormat)>,
    stream: Option<Stream>,
    /// How far ahead of the ear the device's last pull was, µs, as the device reported it.
    latency_us: Arc<AtomicU64>,
    /// Told which device the music goes to.
    watch: Option<Arc<DeviceWatch>>,
    /// The listener's volume, a factor's bits (see [`Volume`]).
    volume: Volume,
}

/// The listener's volume on this output, 0 to 1, set from any thread and applied to every buffer on the
/// device's thread. At 1 (the default) the samples are handed on untouched; anything else is one
/// multiplication a sample, after the engine's chain, so ReplayGain, the limiter and the fades still see
/// the music at full scale.
#[derive(Clone)]
pub struct Volume(Arc<AtomicU32>);

impl Volume {
    pub fn set(&self, v: f32) {
        self.0.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
}

impl Default for Volume {
    fn default() -> Self {
        Volume(Arc::new(AtomicU32::new(1.0f32.to_bits())))
    }
}

/// What the engine is told of a device: its kind as the core ranks outputs, and its name.
fn described(device: &Device) -> Option<nori_engine::Device> {
    let d = device.description().ok()?;
    let kind = match (d.interface_type(), d.device_type()) {
        (InterfaceType::Usb, _) => OutputKind::Usb,
        (InterfaceType::Bluetooth, _) => OutputKind::Bluetooth,
        (InterfaceType::Hdmi | InterfaceType::DisplayPort | InterfaceType::Line | InterfaceType::Spdif, _) => OutputKind::Line,
        (_, DeviceType::Headphones | DeviceType::Headset) => OutputKind::Wired,
        (_, DeviceType::Speaker) | (InterfaceType::BuiltIn, _) => OutputKind::Speaker,
        (_, DeviceType::Dock) => OutputKind::Line,
        _ => OutputKind::Other,
    };
    Some(nori_engine::Device { kind, name: d.name().to_string() })
}

impl CpalOutput {
    /// The system's default output device.
    pub fn new() -> CpalOutput {
        CpalOutput { wanted: None, device: None, config: None, stream: None, latency_us: Arc::new(AtomicU64::new(0)), watch: None, volume: Volume::default() }
    }

    /// The handle the listener's volume is set through, for as long as the output lives.
    pub fn volume(&self) -> Volume {
        self.volume.clone()
    }

    /// The output device called `name` (as [`CpalOutput::devices`] lists them).
    pub fn with_device(name: &str) -> CpalOutput {
        CpalOutput { wanted: Some(name.to_string()), ..CpalOutput::new() }
    }

    /// The names of the output devices there are.
    pub fn devices() -> Vec<String> {
        let host = cpal::default_host();
        host.output_devices().map(|ds| ds.filter_map(|d| d.description().ok().map(|n| n.to_string())).collect()).unwrap_or_default()
    }

    fn pick(&self) -> Result<Device, String> {
        let host = cpal::default_host();
        match &self.wanted {
            None => host.default_output_device().ok_or_else(|| "no output device".to_string()),
            Some(name) => host
                .output_devices()
                .map_err(|e| e.to_string())?
                .find(|d| d.description().is_ok_and(|n| n.to_string() == *name))
                .ok_or_else(|| format!("no output device called {name}")),
        }
    }
}

impl Default for CpalOutput {
    fn default() -> Self {
        CpalOutput::new()
    }
}

/// How much the output can take of a format: float over 16-bit, and nothing else.
fn rank(f: SampleFormat) -> Option<u8> {
    match f {
        SampleFormat::F32 => Some(0),
        SampleFormat::I16 => Some(1),
        _ => None,
    }
}

/// The period asked of the device, ms: long enough that the sound path sleeps between callbacks, short
/// enough that a pause or a seek is heard at once.
const PERIOD_MS: u32 = 100;

impl AudioOutput for CpalOutput {
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        let device = self.pick()?;
        // The stream's own rate and channels when the device takes them, else the device's own.
        let exact = device
            .supported_output_configs()
            .map_err(|e| e.to_string())?
            .filter(|r| r.channels() as usize == want.channels && rank(r.sample_format()).is_some())
            .filter_map(|r| r.try_with_sample_rate(want.rate))
            .min_by_key(|c| rank(c.sample_format()));
        let chosen = match exact {
            Some(c) => c,
            None => device.default_output_config().map_err(|e| e.to_string())?,
        };
        let format = chosen.sample_format();
        if rank(format).is_none() {
            return Err(format!("the device only takes {format:?} samples"));
        }
        // A period of about [`PERIOD_MS`] where the device allows one: the sound server's default is a few ms, so
        // its thread and the callback wake hundreds of times a second for music the engine made seconds ago.
        let frames = chosen.sample_rate() * PERIOD_MS / 1000;
        let buffer_size = match *chosen.buffer_size() {
            cpal::SupportedBufferSize::Range { min, max } if max >= min => BufferSize::Fixed(frames.clamp(min, max)),
            _ => BufferSize::Default,
        };
        let config = StreamConfig { channels: chosen.channels(), sample_rate: chosen.sample_rate(), buffer_size };
        let got = OutputFormat { rate: config.sample_rate, channels: config.channels as usize, bits: 0 };
        if let (Some(w), Some(d)) = (&self.watch, described(&device)) {
            w(d);
        }
        self.device = Some(device);
        self.config = Some((config, format));
        Ok(got)
    }

    fn start(&mut self, mut feed: Feed) -> Result<(), String> {
        let (Some(device), Some((config, format))) = (&self.device, &self.config) else { return Err("not open".into()) };
        let latency = self.latency_us.clone();
        let (volume, volume_i16) = (self.volume.clone(), self.volume.clone());
        let note = move |info: &cpal::OutputCallbackInfo| {
            let t = info.timestamp();
            latency.store(t.playback.saturating_duration_since(t.callback).as_micros() as u64, Ordering::Relaxed);
        };
        // The system moved the stream to another default device: the engine hears which.
        let watch = self.watch.clone().filter(|_| self.wanted.is_none());
        let err = move |e: cpal::Error| {
            if e.kind() == ErrorKind::DeviceChanged {
                if let (Some(w), Some(d)) = (&watch, cpal::default_host().default_output_device().as_ref().and_then(described)) {
                    w(d);
                }
                return;
            }
            eprintln!("nori: the output stream failed: {e}");
        };
        let stream = match format {
            SampleFormat::F32 => device.build_output_stream(
                config.clone(),
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    feed.pull(data);
                    let v = volume.get();
                    if v != 1.0 {
                        data.iter_mut().for_each(|s| *s *= v);
                    }
                    note(info);
                },
                err,
                None,
            ),
            _ => device.build_output_stream(
                config.clone(),
                move |data: &mut [i16], info: &cpal::OutputCallbackInfo| {
                    feed.pull_i16(data);
                    let v = volume_i16.get();
                    if v != 1.0 {
                        data.iter_mut().for_each(|s| *s = (*s as f32 * v) as i16);
                    }
                    note(info);
                },
                err,
                None,
            ),
        }
        .map_err(|e| e.to_string())?;
        // cpal hands streams over paused; the engine resumes it when music is due.
        let _ = stream.pause();
        self.stream = Some(stream);
        Ok(())
    }

    fn pause(&mut self) {
        if let Some(s) = &self.stream {
            let _ = s.pause();
        }
    }

    fn resume(&mut self) {
        if let Some(s) = &self.stream {
            if let Err(e) = s.play() {
                eprintln!("nori: the output would not start: {e}");
            }
        }
    }

    fn watch(&mut self, changed: DeviceWatch) {
        self.watch = Some(Arc::new(changed));
    }

    fn latency_us(&self) -> u64 {
        self.latency_us.load(Ordering::Relaxed)
    }

    /// Whether the device plays float at all; PipeWire, CoreAudio and WASAPI do.
    fn takes_float(&mut self) -> bool {
        let Ok(device) = self.pick() else { return false };
        device.supported_output_configs().is_ok_and(|mut c| c.any(|r| r.sample_format() == SampleFormat::F32))
    }

    fn close(&mut self) {
        self.stream = None;
        self.device = None;
    }
}
