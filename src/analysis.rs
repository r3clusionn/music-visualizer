//! Turns the newest audio samples into what the effects draw: band levels on a logarithmic
//! frequency scale, smoothed and with falling peaks, the overall level, a stable oscilloscope trace,
//! and beats with a tempo estimate.
//!
//! Two transforms run per frame. A long one (4096 samples by default) gives the frequency
//! resolution the low bands need; a short one (1024 samples) over the newest audio gives the time
//! resolution beat detection needs, since a long window smears a drum hit over 85 ms.

use std::collections::VecDeque;

use adsp::fft::RealFft;
use adsp::window::Window;

#[derive(Clone, Debug)]
pub struct Settings {
    pub fft_size: usize,
    pub bands: usize,
    pub min_hz: f64,
    pub max_hz: f64,
    /// Levels map to 0..1 between these (dB relative to a full-scale sine).
    pub db_floor: f64,
    pub db_ceil: f64,
    /// Time constants of the smoothing, in seconds: how fast a band rises and falls.
    pub attack: f64,
    pub release: f64,
    /// Peaks stay this long, then fall at `peak_fall` (0..1 units per second).
    pub peak_hold: f64,
    pub peak_fall: f64,
    /// A beat needs the onset strength above mean + `beat_sensitivity` standard deviations of the
    /// last second.
    pub beat_sensitivity: f64,
    /// Frequency range whose rise in energy counts as an onset (where kick drums are).
    pub beat_low_hz: f64,
    pub beat_high_hz: f64,
    /// How strongly magnitudes are compressed before the flux is taken: ln(1 + c * magnitude).
    /// Small values keep loud hits dominant; large ones make quiet onsets count as much.
    pub onset_compression: f64,
    /// A beat must also reach this fraction of the strongest onset of the last few seconds,
    /// which keeps quieter low notes (a bass line) from counting while adapting to the volume.
    pub beat_peak_fraction: f64,
    /// Samples in the oscilloscope trace.
    pub wave_len: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            fft_size: 4096,
            bands: 64,
            min_hz: 30.0,
            max_hz: 16_000.0,
            db_floor: -70.0,
            db_ceil: 0.0,
            attack: 0.015,
            release: 0.18,
            peak_hold: 0.35,
            peak_fall: 0.9,
            beat_sensitivity: 2.0,
            beat_low_hz: 40.0,
            beat_high_hz: 120.0,
            onset_compression: 10.0,
            beat_peak_fraction: 0.6,
            wave_len: 1024,
        }
    }
}

/// One analysed moment.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    /// Smoothed band levels, 0..1, lowest frequency first.
    pub bands: Vec<f32>,
    /// Peaks of `bands`, held and then falling.
    pub peaks: Vec<f32>,
    /// Unsmoothed band levels in dB (relative to a full-scale sine).
    pub band_db: Vec<f32>,
    /// RMS of the last 50 ms in dB full scale (a full-scale sine reads -3 dB).
    pub rms_db: f32,
    /// Oscilloscope trace, starting at a rising zero crossing so it stands still for steady tones.
    pub wave: Vec<f32>,
    pub beat: bool,
    /// How far the onset strength cleared the threshold (1 = just at it).
    pub beat_strength: f32,
    /// Onset strength (spectral flux in the beat range).
    pub flux: f32,
    /// Tempo from the intervals between recent beats, in beats per minute.
    pub bpm: Option<f32>,
    /// Seconds since the analyser started (the sum of `dt`s).
    pub time: f64,
}

pub struct Analyzer {
    s: Settings,
    rate: f64,
    fft: RealFft,
    window: Vec<f64>,
    /// N * sum(w^2): turns a band's summed bin power into the amplitude of a sine.
    power_norm: f64,
    /// sum(w): turns one bin's magnitude into a sine amplitude.
    sum_w: f64,
    /// Per band: first and last bin (inclusive), or an interpolation point for narrow bands.
    edges: Vec<BandSpan>,
    onset_fft: RealFft,
    onset_window: Vec<f64>,
    onset_bins: (usize, usize),
    prev_onset: Vec<f64>,
    flux_hist: VecDeque<(f64, f64)>,
    last_beat: f64,
    /// The largest recent onset strength, decaying over a few seconds.
    flux_peak: f64,
    /// Samples that arrived but were too few for another onset step.
    onset_carry: usize,
    beats: VecDeque<f64>,
    smoothed: Vec<f32>,
    peaks: Vec<f32>,
    peak_age: Vec<f64>,
    time: f64,
}

#[derive(Clone, Copy, Debug)]
enum BandSpan {
    Bins(usize, usize),
    /// Narrower than a bin: the magnitude interpolated at this fractional bin.
    At(f64),
}

const ONSET_SIZE: usize = 1024;
/// The onset detector's step: 10.7 ms at 48 kHz.
const ONSET_HOP: usize = 512;

impl Analyzer {
    pub fn new(s: Settings, sample_rate: u32) -> Analyzer {
        let rate = sample_rate as f64;
        let n = s.fft_size;
        let window = Window::Hann.generate(n, true);
        let sum_w: f64 = window.iter().sum();
        let power_norm = n as f64 * window.iter().map(|w| w * w).sum::<f64>();
        let edges = band_spans(&s, rate);
        let bin = |hz: f64| (hz * ONSET_SIZE as f64 / rate).round() as usize;
        let onset_bins = (bin(s.beat_low_hz).max(1), bin(s.beat_high_hz).clamp(1, ONSET_SIZE / 2));
        Analyzer {
            rate,
            fft: RealFft::new(n),
            window,
            power_norm,
            sum_w,
            edges,
            onset_fft: RealFft::new(ONSET_SIZE),
            onset_window: Window::Hann.generate(ONSET_SIZE, true),
            onset_bins,
            prev_onset: vec![0.0; ONSET_SIZE / 2 + 1],
            flux_hist: VecDeque::new(),
            last_beat: f64::NEG_INFINITY,
            flux_peak: 0.0,
            onset_carry: 0,
            beats: VecDeque::new(),
            smoothed: vec![0.0; s.bands],
            peaks: vec![0.0; s.bands],
            peak_age: vec![0.0; s.bands],
            time: 0.0,
            s,
        }
    }

    /// Sets the clock (seconds), for analysis that starts part way into a file.
    pub fn set_time(&mut self, t: f64) {
        self.time = t;
    }

    pub fn settings(&self) -> &Settings {
        &self.s
    }

    /// The centre frequency of each band.
    pub fn band_centers(&self) -> Vec<f64> {
        band_edges_hz(&self.s).windows(2).map(|w| (w[0] * w[1]).sqrt()).collect()
    }

    /// Analyses the newest audio. `samples` is mono, newest last; shorter input is padded with
    /// silence in front. `dt` is the time since the previous call.
    pub fn analyze(&mut self, samples: &[f32], dt: f64) -> Frame {
        self.time += dt;
        let n = self.s.fft_size;
        let tail = |len: usize| -> Vec<f64> {
            let mut v = vec![0.0; len.saturating_sub(samples.len())];
            v.extend(samples[samples.len().saturating_sub(len)..].iter().map(|&x| x as f64));
            v
        };
        // Band levels.
        let x: Vec<f64> = tail(n).iter().zip(&self.window).map(|(a, w)| a * w).collect();
        let spec = self.fft.forward(&x);
        let power: Vec<f64> = spec.iter().map(|c| c.norm_sqr()).collect();
        let mut band_db = Vec::with_capacity(self.s.bands);
        for span in &self.edges {
            let amp = match *span {
                BandSpan::Bins(a, b) => 2.0 * (power[a..=b].iter().sum::<f64>() / self.power_norm).sqrt(),
                BandSpan::At(f) => {
                    let i = f.floor() as usize;
                    let frac = f - i as f64;
                    let m = power[i].sqrt() * (1.0 - frac) + power[(i + 1).min(power.len() - 1)].sqrt() * frac;
                    2.0 * m / self.sum_w
                }
            };
            band_db.push((20.0 * amp.max(1e-12).log10()) as f32);
        }
        let range = (self.s.db_ceil - self.s.db_floor) as f32;
        let a_up = 1.0 - (-dt / self.s.attack).exp() as f32;
        let a_down = 1.0 - (-dt / self.s.release).exp() as f32;
        for (i, &db) in band_db.iter().enumerate() {
            let v = ((db - self.s.db_floor as f32) / range).clamp(0.0, 1.0);
            let s = &mut self.smoothed[i];
            *s += (v - *s) * if v > *s { a_up } else { a_down };
            if *s >= self.peaks[i] {
                self.peaks[i] = *s;
                self.peak_age[i] = 0.0;
            } else {
                self.peak_age[i] += dt;
                if self.peak_age[i] > self.s.peak_hold {
                    self.peaks[i] = (self.peaks[i] - self.s.peak_fall as f32 * dt as f32).max(*s);
                }
            }
        }
        // Level over the last 50 ms.
        let lvl_len = ((self.rate * 0.05) as usize).min(samples.len()).max(1);
        let recent = &samples[samples.len().saturating_sub(lvl_len)..];
        let ms = recent.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / lvl_len as f64;
        let rms_db = (10.0 * ms.max(1e-24).log10()) as f32;
        // Onsets and beats.
        let (flux, beat, strength) = self.onset(samples, dt);
        let bpm = self.tempo();
        Frame {
            bands: self.smoothed.clone(),
            peaks: self.peaks.clone(),
            band_db,
            rms_db,
            wave: trigger(samples, self.s.wave_len),
            beat,
            beat_strength: strength,
            flux: flux as f32,
            bpm,
            time: self.time,
        }
    }

    /// Spectral flux over the beat range on a compressed (log) magnitude, against an adaptive
    /// threshold from the last second of flux values. The detector runs on its own hop of
    /// `ONSET_HOP` samples over all audio that arrived since the last frame, so what it finds
    /// does not depend on the frame rate. Returns the largest flux of the frame, whether a beat
    /// started in it, and how far it cleared the threshold.
    fn onset(&mut self, samples: &[f32], dt: f64) -> (f64, bool, f32) {
        let fresh = (dt * self.rate) as usize + self.onset_carry;
        let steps = fresh / ONSET_HOP;
        self.onset_carry = fresh % ONSET_HOP;
        let (mut best, mut beat, mut strength) = (0.0f64, false, 0.0f32);
        for s in (0..steps).rev() {
            // The window that ended `s` hops (plus the carry) before the newest sample.
            let end = samples.len().saturating_sub(s * ONSET_HOP + self.onset_carry);
            let mut x = vec![0.0; ONSET_SIZE.saturating_sub(end)];
            x.extend(samples[end.saturating_sub(ONSET_SIZE)..end].iter().map(|&v| v as f64));
            let t = self.time - (s * ONSET_HOP + self.onset_carry) as f64 / self.rate;
            let (flux, b, k) = self.onset_step(&x, t);
            best = best.max(flux);
            if b {
                beat = true;
                strength = strength.max(k);
            }
        }
        (best, beat, strength)
    }

    fn onset_step(&mut self, x: &[f64], now: f64) -> (f64, bool, f32) {
        let w: Vec<f64> = x.iter().zip(&self.onset_window).map(|(a, w)| a * w).collect();
        let spec = self.onset_fft.forward(&w);
        let mut flux = 0.0;
        let (lo, hi) = self.onset_bins;
        for (bin, prev) in spec[lo..=hi].iter().zip(&mut self.prev_onset[lo..=hi]) {
            let m = (1.0 + self.s.onset_compression * bin.abs() / ONSET_SIZE as f64).ln();
            flux += (m - *prev).max(0.0);
            *prev = m;
        }
        while self.flux_hist.front().is_some_and(|&(t, _)| now - t > 1.0) {
            self.flux_hist.pop_front();
        }
        let n = self.flux_hist.len().max(1) as f64;
        let mean = self.flux_hist.iter().map(|&(_, f)| f).sum::<f64>() / n;
        let var = self.flux_hist.iter().map(|&(_, f)| (f - mean) * (f - mean)).sum::<f64>() / n;
        let threshold = mean + self.s.beat_sensitivity * var.sqrt();
        self.flux_hist.push_back((now, flux));
        // A beat must also reach a fraction of the strongest recent onset (so it adapts to the
        // volume) and a tiny absolute floor (so hiss in near silence never counts); at most four
        // per second, and only after some history.
        let hop = ONSET_HOP as f64 / self.rate;
        self.flux_peak = flux.max(self.flux_peak * (-hop / 4.0).exp());
        let beat = self.flux_hist.len() > 20
            && flux > threshold
            && flux > self.s.beat_peak_fraction * self.flux_peak
            && flux > 0.02
            && now - self.last_beat > 0.25;
        if beat {
            self.last_beat = now;
            self.beats.push_back(now);
            while self.beats.len() > 24 || self.beats.front().is_some_and(|&t| now - t > 12.0) {
                self.beats.pop_front();
            }
        }
        (flux, beat, if beat { (flux / threshold.max(1e-9)) as f32 } else { 0.0 })
    }

    /// The median interval between recent beats, as beats per minute folded into 70..180.
    fn tempo(&self) -> Option<f32> {
        if self.beats.len() < 5 || self.time - self.beats.back().copied().unwrap_or(0.0) > 3.0 {
            return None;
        }
        let mut gaps: Vec<f64> = self.beats.iter().zip(self.beats.iter().skip(1)).map(|(a, b)| b - a).collect();
        gaps.sort_by(f64::total_cmp);
        let mut bpm = 60.0 / gaps[gaps.len() / 2];
        while bpm < 70.0 {
            bpm *= 2.0;
        }
        while bpm > 180.0 {
            bpm /= 2.0;
        }
        Some(bpm as f32)
    }
}

/// Band edges in Hz, spaced evenly on a log scale.
fn band_edges_hz(s: &Settings) -> Vec<f64> {
    let (lo, hi) = (s.min_hz.ln(), s.max_hz.ln());
    (0..=s.bands).map(|i| (lo + (hi - lo) * i as f64 / s.bands as f64).exp()).collect()
}

fn band_spans(s: &Settings, rate: f64) -> Vec<BandSpan> {
    let bin_hz = rate / s.fft_size as f64;
    let last = s.fft_size / 2;
    band_edges_hz(s)
        .windows(2)
        .map(|w| {
            let a = (w[0] / bin_hz).ceil() as usize;
            let b = ((w[1] / bin_hz).ceil() as usize).saturating_sub(1).min(last);
            if a <= b && a <= last {
                BandSpan::Bins(a, b)
            } else {
                BandSpan::At(((w[0] * w[1]).sqrt() / bin_hz).min(last as f64 - 1.0))
            }
        })
        .collect()
}

/// The last `len` samples' worth of trace, starting at the latest rising zero crossing that still
/// leaves `len` samples after it (so a steady tone draws in the same place every frame).
pub fn trigger(samples: &[f32], len: usize) -> Vec<f32> {
    if samples.len() <= len {
        return samples.to_vec();
    }
    let latest_start = samples.len() - len;
    let search_from = latest_start.saturating_sub(len);
    let start =
        (search_from + 1..=latest_start).rev().find(|&i| samples[i - 1] < 0.0 && samples[i] >= 0.0).unwrap_or(latest_start);
    samples[start..start + len].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f64, amp: f64, rate: f64, n: usize) -> Vec<f32> {
        (0..n).map(|i| (amp * (2.0 * std::f64::consts::PI * f * i as f64 / rate).sin()) as f32).collect()
    }

    #[test]
    fn bands_cover_the_range_in_order() {
        let s = Settings::default();
        let e = band_edges_hz(&s);
        assert_eq!(e.len(), 65);
        assert!((e[0] - 30.0).abs() < 1e-9 && (e[64] - 16_000.0).abs() < 1e-6);
        assert!(e.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn a_full_scale_sine_reads_zero_db_in_its_band() {
        let rate = 48_000.0;
        for f in [50.0, 440.0, 1000.0, 5000.0, 12_000.0] {
            let mut a = Analyzer::new(Settings::default(), 48_000);
            let x = sine(f, 1.0, rate, 8192);
            let fr = a.analyze(&x, 1.0 / 60.0);
            let (best, db) = fr.band_db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
            let centers = a.band_centers();
            let edges = band_edges_hz(a.settings());
            assert!(
                f >= edges[best] * 0.97 && f <= edges[best + 1] * 1.03,
                "{f} Hz landed in band {best} ({:.0} Hz)",
                centers[best]
            );
            assert!(db.abs() < 1.5, "{f} Hz full scale read {db} dB");
        }
    }

    #[test]
    fn levels_scale_with_amplitude() {
        let mut a = Analyzer::new(Settings::default(), 48_000);
        let fr = a.analyze(&sine(1000.0, 0.1, 48_000.0, 8192), 0.016);
        let max = fr.band_db.iter().cloned().fold(f32::MIN, f32::max);
        assert!((max + 20.0).abs() < 1.5, "-20 dB sine read {max}");
        assert!((fr.rms_db + 23.0).abs() < 0.2, "RMS of a 0.1 sine is -23 dBFS, read {}", fr.rms_db);
    }

    #[test]
    fn attack_is_fast_and_release_slow() {
        let mut a = Analyzer::new(Settings::default(), 48_000);
        let tone = sine(1000.0, 1.0, 48_000.0, 4096);
        let silence = vec![0.0f32; 4096];
        let band = |fr: &Frame| fr.bands.iter().cloned().fold(0.0, f32::max);
        let mut fr = Frame::default();
        for _ in 0..3 {
            fr = a.analyze(&tone, 1.0 / 60.0);
        }
        assert!(band(&fr) > 0.95, "after 50 ms of tone: {}", band(&fr));
        fr = a.analyze(&silence, 1.0 / 60.0);
        let after_one = band(&fr);
        assert!(after_one > 0.8, "one frame of silence should not drop it much: {after_one}");
        for _ in 0..60 {
            fr = a.analyze(&silence, 1.0 / 60.0);
        }
        assert!(band(&fr) < 0.01);
        assert!(fr.peaks.iter().all(|&p| p < 0.5), "peaks fall after the hold");
    }

    #[test]
    fn trigger_finds_a_rising_zero_crossing() {
        let x = sine(100.0, 1.0, 48_000.0, 4000);
        let w = trigger(&x, 1024);
        assert_eq!(w.len(), 1024);
        assert!(w[0] >= 0.0 && w[0] < 0.02 && w[1] > w[0], "{} {}", w[0], w[1]);
    }
}
