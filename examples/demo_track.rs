//! Writes the synthesized demo track as a 16-bit stereo WAV.
//!
//!     cargo run --release --example demo_track -- demo.wav [BPM] [BARS]

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let out = a.first().map(String::as_str).unwrap_or("demo.wav");
    let bpm: f64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(124.0);
    let bars: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(24);
    let t = mviz::demo::track(bpm, bars, 48_000);
    std::fs::write(out, t.wav()).expect("write");
    eprintln!("{out}: {:.1} s at {bpm} BPM, {} kicks", t.left.len() as f64 / 48_000.0, t.kicks.len());
}
