//! A synthesized demo track (drums, bass, chords and an arpeggio), so there is music to test and
//! show without anyone's recording. It also returns when each kick drum starts, which is the
//! ground truth for beat detection.

use std::f64::consts::TAU;

pub struct Track {
    pub rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Start of every kick drum, in seconds.
    pub kicks: Vec<f64>,
}

struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

fn midi_hz(n: f64) -> f64 {
    440.0 * 2f64.powf((n - 69.0) / 12.0)
}

/// `bars` bars of 4/4 at `bpm`. The first two bars are drums only, so a detector has a quiet
/// lead-in; the kick plays on every beat.
pub fn track(bpm: f64, bars: usize, rate: u32) -> Track {
    let sr = rate as f64;
    let beat = 60.0 / bpm;
    let len = (bars as f64 * 4.0 * beat * sr) as usize + rate as usize;
    let mut l = vec![0.0f64; len];
    let mut r = vec![0.0f64; len];
    let mut noise = Noise(0x5eed);
    let mut kicks = Vec::new();
    // Chords (MIDI notes): A minor, F, C, G.
    let chords = [[57.0, 60.0, 64.0], [53.0, 57.0, 60.0], [48.0, 52.0, 55.0], [55.0, 59.0, 62.0]];
    let add = |buf_l: &mut Vec<f64>, buf_r: &mut Vec<f64>, start: f64, dur: f64, pan: f64, f: &mut dyn FnMut(f64) -> f64| {
        let s0 = (start * sr) as usize;
        let n = (dur * sr) as usize;
        for i in 0..n {
            let k = s0 + i;
            if k >= buf_l.len() {
                break;
            }
            let v = f(i as f64 / sr);
            buf_l[k] += v * (1.0 - pan);
            buf_r[k] += v * pan;
        }
    };
    for bar in 0..bars {
        let t_bar = bar as f64 * 4.0 * beat;
        let chord = chords[bar % 4];
        let full = bar >= 2;
        for b in 0..4 {
            let t = t_bar + b as f64 * beat;
            // Kick: a sine falling from 120 to 45 Hz with a fast decay.
            kicks.push(t);
            let mut phase = 0.0;
            add(&mut l, &mut r, t, 0.35, 0.5, &mut |x| {
                let f = 45.0 + 75.0 * (-x / 0.03).exp();
                phase += TAU * f / sr;
                0.9 * phase.sin() * (-x / 0.12).exp()
            });
            // Snare on 2 and 4: noise and a 190 Hz body.
            if b % 2 == 1 {
                add(&mut l, &mut r, t, 0.2, 0.45, &mut |x| {
                    (0.35 * noise.next() + 0.25 * (TAU * 190.0 * x).sin()) * (-x / 0.05).exp()
                });
            }
            // Hi-hats on eighths: noise with its low end removed (first difference).
            for e in 0..2 {
                let mut prev = 0.0;
                add(&mut l, &mut r, t + e as f64 * beat / 2.0, 0.06, 0.65, &mut |x| {
                    let n = noise.next();
                    let v = n - prev;
                    prev = n;
                    0.12 * v * (-x / 0.015).exp()
                });
            }
            if !full {
                continue;
            }
            // Bass on eighths: the chord root two octaves down, a few harmonics.
            for e in 0..2 {
                let f0 = midi_hz(chord[0] - 24.0);
                add(&mut l, &mut r, t + e as f64 * beat / 2.0, beat / 2.0 * 0.9, 0.5, &mut |x| {
                    let s: f64 = (1..=5).map(|h| (TAU * f0 * h as f64 * x).sin() / h as f64).sum();
                    0.22 * s * (1.0 - (-x / 0.005).exp()) * (-x / 0.25).exp()
                });
            }
            // Arpeggio on sixteenths, an octave up.
            for s in 0..4 {
                let note = chord[(b * 4 + s) % 3] + 12.0;
                let f0 = midi_hz(note);
                let pan = if s % 2 == 0 { 0.3 } else { 0.7 };
                add(&mut l, &mut r, t + s as f64 * beat / 4.0, beat / 4.0 * 0.8, pan, &mut |x| {
                    0.06 * ((TAU * f0 * x).sin() + 0.3 * (TAU * 2.0 * f0 * x).sin()) * (-x / 0.08).exp()
                });
            }
        }
        // Pad: the chord held for the bar, fading in.
        if full {
            for (i, &n) in chord.iter().enumerate() {
                let f0 = midi_hz(n);
                add(&mut l, &mut r, t_bar, 4.0 * beat, 0.35 + 0.15 * i as f64, &mut |x| {
                    0.05 * (TAU * f0 * x).sin() * (1.0 - (-x / 0.3).exp()) * (1.0 - x / (4.0 * beat)).max(0.0)
                });
            }
        }
    }
    // Normalise to -1 dBFS peak.
    let peak = l.iter().chain(&r).fold(0.0f64, |m, v| m.max(v.abs())).max(1e-9);
    let g = 0.89 / peak;
    Track { rate, left: l.iter().map(|v| (v * g) as f32).collect(), right: r.iter().map(|v| (v * g) as f32).collect(), kicks }
}

impl Track {
    pub fn mono(&self) -> Vec<f32> {
        self.left.iter().zip(&self.right).map(|(a, b)| (a + b) / 2.0).collect()
    }

    pub fn wav(&self) -> Vec<u8> {
        let audio = adsp::wav::Audio {
            sample_rate: self.rate,
            channels: vec![self.left.iter().map(|&v| v as f64).collect(), self.right.iter().map(|&v| v as f64).collect()],
        };
        adsp::wav::encode(&audio, adsp::wav::Format::Pcm16)
    }
}
