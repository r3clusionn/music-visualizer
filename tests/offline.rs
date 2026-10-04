//! Rendering without a window, from the demo track.

use mviz::analysis::Settings;
use mviz::audio::{decode, Pcm};
use mviz::demo::track;
use mviz::offline::{png, render_at};
use mviz::render::{Effect, Style};

fn demo_pcm() -> Pcm {
    let t = track(124.0, 8, 48_000);
    let samples = t.left.iter().zip(&t.right).flat_map(|(l, r)| [*l, *r]).collect();
    Pcm { rate: 48_000, channels: 2, samples }
}

#[test]
fn a_rendered_frame_is_a_valid_png_and_repeatable() {
    let pcm = demo_pcm();
    let style = Style { panels: vec![(Effect::Spectrogram, 1.0), (Effect::Bars, 2.0)], ..Style::default() };
    let a = render_at(&pcm, &Settings::default(), &style, 60.0, 10.0, 320, 180);
    let b = render_at(&pcm, &Settings::default(), &style, 60.0, 10.0, 320, 180);
    assert_eq!(a, b, "the same frame twice");
    let bytes = png(&a);
    let img = lumen::png::decode(&bytes).unwrap();
    assert_eq!((img.width, img.height), (320, 180));
    // Music is playing at 10 s, so the picture is not just background.
    let distinct: std::collections::HashSet<u32> = a.px.iter().copied().collect();
    assert!(distinct.len() > 50, "only {} colours", distinct.len());
}

#[test]
fn the_wav_writer_and_the_decoder_agree() {
    let t = track(124.0, 2, 48_000);
    let dir = std::env::temp_dir().join(format!("mviz-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("demo.wav");
    std::fs::write(&p, t.wav()).unwrap();
    let pcm = decode(&p).unwrap();
    assert_eq!((pcm.rate, pcm.channels, pcm.frames()), (48_000, 2, t.left.len()));
    // 16-bit quantisation: within one step.
    let worst = pcm.samples.iter().step_by(2).zip(&t.left).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst <= 1.0 / 32768.0 + 1e-6, "{worst}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn resampling_keeps_a_tone_at_its_frequency() {
    // 1 kHz at 44.1 kHz, converted to 48 kHz: the strongest band stays the one holding 1 kHz.
    let n = 44_100;
    let samples: Vec<f32> =
        (0..n).map(|i| (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 44_100.0).sin() as f32 * 0.5).collect();
    let pcm = Pcm { rate: 44_100, channels: 1, samples };
    let up = mviz::audio::resample(&pcm, 48_000);
    assert_eq!(up.rate, 48_000);
    assert!((up.frames() as i64 - 48_000).abs() < 10);
    let mut a = mviz::analysis::Analyzer::new(Settings::default(), 48_000);
    let f = a.analyze(&up.samples[10_000..20_000], 0.016);
    let best = f.band_db.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap().0;
    let centre = a.band_centers()[best];
    assert!((centre / 1000.0).log2().abs() < 0.15, "strongest band at {centre} Hz");
}
