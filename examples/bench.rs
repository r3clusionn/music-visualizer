//! Time per frame for the analysis and for each effect, on the demo track.
//!
//!     cargo run --release --example bench -- [WIDTHxHEIGHT]

use std::time::Instant;

use mviz::analysis::{Analyzer, Settings};
use mviz::demo::track;
use mviz::render::{Canvas, Effect, Renderer, Style};

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let size = std::env::args().nth(1).unwrap_or_else(|| "1920x1080".into());
    let (w, h) = size.split_once('x').map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap())).unwrap();
    let t = track(124.0, 16, 48_000);
    let mono = t.mono();
    let frames = 1200; // 20 s at 60 fps
    let dt = 1.0 / 60.0;
    // Analysis: the newest 4096 samples per frame, as the live loop does.
    let mut a = Analyzer::new(Settings::default(), 48_000);
    let mut times = Vec::new();
    let mut out = Vec::new();
    for i in 1..=frames {
        let end = (i as f64 * dt * 48_000.0) as usize;
        let s = &mono[end.saturating_sub(4096)..end];
        let t0 = Instant::now();
        out.push(a.analyze(s, dt));
        times.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    println!("analysis: median {:.0} us per frame ({frames} frames)", median(times));
    for e in Effect::ALL {
        let mut r = Renderer::new(Style { panels: vec![(e, 1.0)], ..Style::default() });
        let mut c = Canvas::new(w, h);
        let mut times = Vec::new();
        for f in &out {
            let t0 = Instant::now();
            r.draw(&mut c, f, dt as f32);
            times.push(t0.elapsed().as_secs_f64() * 1e3);
        }
        println!("{:<12} {w}x{h}: median {:.2} ms per frame", e.name(), median(times));
    }
}
