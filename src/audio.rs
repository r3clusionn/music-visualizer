//! Where the audio comes from: what the computer is playing (WASAPI loopback on Windows), a
//! microphone, or a file that is played and shown in step with what is heard.

use std::collections::VecDeque;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

/// Decoded audio: interleaved samples.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: usize,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1)
    }

    pub fn duration(&self) -> f64 {
        self.frames() as f64 / self.rate as f64
    }

    /// The channels averaged into one.
    pub fn mono(&self) -> Vec<f32> {
        let c = self.channels.max(1);
        self.samples.chunks_exact(c).map(|f| f.iter().sum::<f32>() / c as f32).collect()
    }
}

/// Decodes any format symphonia knows (MP3, AAC/M4A, FLAC, Ogg Vorbis, WAV).
pub fn decode(path: &Path) -> Result<Pcm, String> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| format!("{}: no audio track", path.display()))?;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio parameters")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let id = track.id;
    let mut out = Pcm { rate: 0, channels: 0, samples: Vec::new() };
    let mut chunk: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if packet.track_id != id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                out.rate = buf.spec().rate();
                out.channels = buf.spec().channels().count();
                chunk.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut chunk);
                out.samples.extend_from_slice(&chunk);
            }
            // A damaged packet is skipped, as players do.
            Err(Error::DecodeError(_)) => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    if out.channels == 0 || out.samples.is_empty() {
        return Err(format!("{}: no audio decoded", path.display()));
    }
    Ok(out)
}

/// Resamples every channel with adsp's polyphase resampler.
pub fn resample(p: &Pcm, to: u32) -> Pcm {
    if p.rate == to {
        return p.clone();
    }
    let c = p.channels;
    let chans: Vec<Vec<f64>> = (0..c).map(|ch| p.samples.iter().skip(ch).step_by(c).map(|&v| v as f64).collect()).collect();
    let outs: Vec<Vec<f64>> =
        chans.iter().map(|x| adsp::resample::Resampler::between(p.rate as usize, to as usize).run(x)).collect();
    let n = outs.iter().map(Vec::len).min().unwrap_or(0);
    let mut samples = Vec::with_capacity(n * c);
    for i in 0..n {
        for o in &outs {
            samples.push(o[i] as f32);
        }
    }
    Pcm { rate: to, channels: c, samples }
}

/// The newest mono samples of a live input, about two seconds' worth.
pub struct Ring {
    buf: VecDeque<f32>,
    cap: usize,
}

impl Ring {
    fn push_interleaved(&mut self, data: &[f32], channels: usize) {
        for f in data.chunks_exact(channels.max(1)) {
            if self.buf.len() == self.cap {
                self.buf.pop_front();
            }
            self.buf.push_back(f.iter().sum::<f32>() / f.len() as f32);
        }
    }
}

pub enum Source {
    Live { _stream: Stream, ring: Arc<Mutex<Ring>>, rate: u32, name: String },
    File(FileSource),
}

pub struct FileSource {
    pub name: String,
    pub mono: Arc<Vec<f32>>,
    pub rate: u32,
    /// Frames handed to the output device so far.
    pos: Arc<AtomicU64>,
    /// Device latency in frames: what is heard now was handed over this long ago.
    latency: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    _stream: Option<Stream>,
    /// Without playback, time advances with the frames shown.
    clock: Option<Mutex<f64>>,
}

fn device_named(devices: impl Iterator<Item = cpal::Device>, want: &str) -> Option<cpal::Device> {
    let want = want.to_lowercase();
    devices.into_iter().find(|d| d.to_string().to_lowercase().contains(&want))
}

/// Device names, for `--list-devices`.
pub fn list_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let mut out = Vec::new();
    for d in host.output_devices().map_err(|e| e.to_string())? {
        out.push(format!("output (loopback): {d}"));
    }
    for d in host.input_devices().map_err(|e| e.to_string())? {
        out.push(format!("input:             {d}"));
    }
    Ok(out)
}

impl Source {
    /// What an output device is playing. On Windows an input stream on an output device is a
    /// WASAPI loopback capture; other systems need a monitor input (PulseAudio) or a virtual
    /// device instead.
    pub fn loopback(device: Option<&str>) -> Result<Source, String> {
        let host = cpal::default_host();
        let dev = match device {
            Some(n) => device_named(host.output_devices().map_err(|e| e.to_string())?, n)
                .ok_or_else(|| format!("no output device matching '{n}'"))?,
            None => host.default_output_device().ok_or("no default output device")?,
        };
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        Self::capture(dev, cfg)
    }

    pub fn microphone(device: Option<&str>) -> Result<Source, String> {
        let host = cpal::default_host();
        let dev = match device {
            Some(n) => device_named(host.input_devices().map_err(|e| e.to_string())?, n)
                .ok_or_else(|| format!("no input device matching '{n}'"))?,
            None => host.default_input_device().ok_or("no default input device")?,
        };
        let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
        Self::capture(dev, cfg)
    }

    fn capture(dev: cpal::Device, cfg: cpal::SupportedStreamConfig) -> Result<Source, String> {
        let rate = cfg.sample_rate();
        let channels = cfg.channels() as usize;
        let ring = Arc::new(Mutex::new(Ring { buf: VecDeque::with_capacity(rate as usize * 2), cap: rate as usize * 2 }));
        let config: StreamConfig = cfg.config();
        let err = |e| eprintln!("audio: {e}");
        let r = Arc::clone(&ring);
        let stream = match cfg.sample_format() {
            SampleFormat::F32 => dev.build_input_stream::<f32, _, _>(
                config,
                move |d, _| r.lock().unwrap().push_interleaved(d, channels),
                err,
                None,
            ),
            SampleFormat::I16 => dev.build_input_stream::<i16, _, _>(
                config,
                move |d, _| {
                    let f: Vec<f32> = d.iter().map(|&s| s as f32 / 32768.0).collect();
                    r.lock().unwrap().push_interleaved(&f, channels)
                },
                err,
                None,
            ),
            SampleFormat::I32 => dev.build_input_stream::<i32, _, _>(
                config,
                move |d, _| {
                    let f: Vec<f32> = d.iter().map(|&s| s as f32 / 2_147_483_648.0).collect();
                    r.lock().unwrap().push_interleaved(&f, channels)
                },
                err,
                None,
            ),
            f => return Err(format!("sample format {f:?} is not supported")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Source::Live { _stream: stream, ring, rate, name: dev.to_string() })
    }

    /// A file, played on the default output device if `play`, else only shown (time advances by
    /// the frames drawn).
    pub fn file(path: &Path, play: bool) -> Result<Source, String> {
        let pcm = decode(path)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let pos = Arc::new(AtomicU64::new(0));
        let latency = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(false));
        if !play {
            return Ok(Source::File(FileSource {
                name,
                rate: pcm.rate,
                mono: Arc::new(pcm.mono()),
                pos,
                latency,
                paused,
                _stream: None,
                clock: Some(Mutex::new(0.0)),
            }));
        }
        let host = cpal::default_host();
        let dev = host.default_output_device().ok_or("no default output device")?;
        let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
        if cfg.sample_format() != SampleFormat::F32 {
            return Err(format!("the output device wants {:?} samples; only f32 output is supported", cfg.sample_format()));
        }
        // Shared-mode devices run at their own rate: convert the file to it once, up front.
        let out_rate = cfg.sample_rate();
        let pcm = resample(&pcm, out_rate);
        let mono = Arc::new(pcm.mono());
        let dev_ch = cfg.channels() as usize;
        let data = Arc::new(pcm);
        let (p, l, z, d) = (Arc::clone(&pos), Arc::clone(&latency), Arc::clone(&paused), Arc::clone(&data));
        let stream = dev
            .build_output_stream::<f32, _, _>(
                cfg.config(),
                move |out, info| {
                    let ts = info.timestamp();
                    let lat = ts.playback.duration_since(ts.callback);
                    l.store((lat.as_secs_f64() * out_rate as f64) as u64, Ordering::Relaxed);
                    if z.load(Ordering::Relaxed) {
                        out.fill(0.0);
                        return;
                    }
                    let mut frame = p.load(Ordering::Relaxed) as usize;
                    let fc = d.channels;
                    for o in out.chunks_exact_mut(dev_ch) {
                        if frame < d.frames() {
                            for (c, s) in o.iter_mut().enumerate() {
                                // Mono files go to every speaker; extra file channels are dropped.
                                *s = d.samples[frame * fc + c % fc];
                            }
                            frame += 1;
                        } else {
                            o.fill(0.0);
                        }
                    }
                    p.store(frame as u64, Ordering::Relaxed);
                },
                |e| eprintln!("audio: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Source::File(FileSource { name, rate: out_rate, mono, pos, latency, paused, _stream: Some(stream), clock: None }))
    }

    pub fn rate(&self) -> u32 {
        match self {
            Source::Live { rate, .. } => *rate,
            Source::File(f) => f.rate,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Source::Live { name, .. } => name,
            Source::File(f) => &f.name,
        }
    }

    /// The newest `n` mono samples: for a file, ending at what is being heard right now.
    /// `dt` advances the clock of a file that is shown without playing.
    pub fn latest(&self, n: usize, dt: f64, out: &mut Vec<f32>) {
        out.clear();
        match self {
            Source::Live { ring, .. } => {
                let r = ring.lock().unwrap();
                let skip = r.buf.len().saturating_sub(n);
                out.extend(r.buf.iter().skip(skip));
            }
            Source::File(f) => {
                let end = f.heard(dt);
                out.extend_from_slice(&f.mono[end.saturating_sub(n)..end]);
            }
        }
    }

    /// For a file: (position, length) in seconds.
    pub fn position(&self) -> Option<(f64, f64)> {
        match self {
            Source::File(f) => Some((f.heard(0.0) as f64 / f.rate as f64, f.mono.len() as f64 / f.rate as f64)),
            _ => None,
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self, Source::File(f) if f.heard(0.0) >= f.mono.len())
    }

    pub fn toggle_pause(&self) {
        if let Source::File(f) = self {
            f.paused.fetch_xor(true, Ordering::Relaxed);
        }
    }

    /// Moves a file's position by `secs` (negative goes back).
    pub fn seek(&self, secs: f64) {
        if let Source::File(f) = self {
            let delta = (secs * f.rate as f64) as i64;
            match &f.clock {
                Some(c) => {
                    let mut t = c.lock().unwrap();
                    *t = (*t + secs).max(0.0);
                }
                None => {
                    let cur = f.pos.load(Ordering::Relaxed) as i64;
                    f.pos.store((cur + delta).clamp(0, f.mono.len() as i64) as u64, Ordering::Relaxed);
                }
            }
        }
    }
}

impl FileSource {
    /// The frame being heard now: handed-over frames minus the device latency.
    fn heard(&self, dt: f64) -> usize {
        match &self.clock {
            Some(c) => {
                let mut t = c.lock().unwrap();
                if !self.paused.load(Ordering::Relaxed) {
                    *t += dt;
                }
                ((*t * self.rate as f64) as usize).min(self.mono.len())
            }
            None => {
                let p = self.pos.load(Ordering::Relaxed);
                p.saturating_sub(self.latency.load(Ordering::Relaxed)).min(self.mono.len() as u64) as usize
            }
        }
    }
}

/// Waits briefly so a live source has some audio before the first frame.
pub fn warm_up() {
    std::thread::sleep(Duration::from_millis(100));
}
