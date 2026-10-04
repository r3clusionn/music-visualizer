//! Scores beat detection on the demo track for a range of settings (development aid).
use mviz::analysis::Settings;
use mviz::demo::track;
use mviz::offline::walk;

fn main() {
    for (lo, hi) in [(40.0, 120.0)] {
        for c in [10.0] {
            for k in [2.0, 2.5] {
                for frac in [0.3, 0.4, 0.5, 0.6] {
                    let s = Settings {
                        beat_low_hz: lo,
                        beat_high_hz: hi,
                        onset_compression: c,
                        beat_sensitivity: k,
                        beat_peak_fraction: frac,
                        ..Settings::default()
                    };
                    let mut line = format!("{lo:>3}-{hi:<3} c={c:<6} k={k} f={frac}:");
                    for bpm in [90.0, 124.0, 140.0] {
                        let t = track(bpm, 16, 48_000);
                        let mono = t.mono();
                        let mut beats = Vec::new();
                        let mut fl = Vec::new();
                        walk(&mono, 48_000, &s, 60.0, 0.0, mono.len() as f64 / 48_000.0 - 1.0, |f, _| {
                            fl.push(f.flux);
                            if f.beat {
                                beats.push(f.time)
                            }
                        });
                        fl.sort_by(f32::total_cmp);
                        if bpm == 124.0 {
                            line += &format!(" [flux median {:.3} p99 {:.3}]", fl[fl.len() / 2], fl[fl.len() * 99 / 100]);
                        }
                        let kicks: Vec<f64> =
                            t.kicks.iter().copied().filter(|&k| k > 1.0 && k < 16.0 * 240.0 / bpm - 1.0).collect();
                        let beats: Vec<f64> = beats.into_iter().filter(|&b| b > 1.0).collect();
                        let tp = kicks.iter().filter(|&&k| beats.iter().any(|&b| (b - k).abs() <= 0.05)).count();
                        line += &format!("  {bpm}: {tp}/{} +{}", kicks.len(), beats.len().saturating_sub(tp));
                    }
                    println!("{line}");
                }
            }
        }
    }
}
