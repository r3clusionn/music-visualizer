//! Rendering without a window: one frame of a file as a PNG, or the whole file as a video
//! (frames piped to ffmpeg, which also copies in the audio).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::analysis::{Analyzer, Frame, Settings};
use crate::audio::Pcm;
use crate::render::{Canvas, Renderer, Style};

/// Analyses `mono` (at `rate`) frame by frame at `fps` from `from` to `to` seconds, calling `each`
/// with every frame, as the live loop would see it.
pub fn walk(mono: &[f32], rate: u32, settings: &Settings, fps: f64, from: f64, to: f64, mut each: impl FnMut(&Frame, usize)) {
    let mut a = Analyzer::new(settings.clone(), rate);
    let dt = 1.0 / fps;
    let n = settings.fft_size.max(4096);
    let first = (from * fps).round() as usize;
    let last = (to * fps).round() as usize;
    // The analyser's clock reads the time of the newest sample it is given.
    a.set_time(first as f64 * dt);
    for i in first + 1..=last {
        let end = ((i as f64 * dt * rate as f64) as usize).min(mono.len());
        let f = a.analyze(&mono[end.saturating_sub(n)..end], dt);
        each(&f, i);
    }
}

/// The canvas as an 8-bit RGB PNG (through the lumen image library).
pub fn png(c: &Canvas) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(c.w * c.h * 3);
    for &p in &c.px {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    let img = lumen::image::Image::new8(c.w as u32, c.h as u32, lumen::image::Color::Rgb, rgb).expect("buffer matches size");
    lumen::png::encode(&img, 6)
}

/// Draws the frame at `at` seconds, replaying enough history before it that smoothing, peaks,
/// the beat tracker and a full-width spectrogram look as they would live.
pub fn render_at(pcm: &Pcm, settings: &Settings, style: &Style, fps: f64, at: f64, w: usize, h: usize) -> Canvas {
    let mono = pcm.mono();
    let mut r = Renderer::new(style.clone());
    let mut c = Canvas::new(w, h);
    let history = (w as f64 / style.scroll.max(1.0) as f64 + 0.5).max(3.0);
    walk(&mono, pcm.rate, settings, fps, (at - history).max(0.0), at, |f, _| r.draw(&mut c, f, (1.0 / fps) as f32));
    c
}

/// Renders the whole file to a video with ffmpeg (which must be on the PATH).
#[allow(clippy::too_many_arguments)]
pub fn export(
    input: &Path,
    pcm: &Pcm,
    settings: &Settings,
    style: &Style,
    fps: f64,
    w: usize,
    h: usize,
    out: &Path,
) -> Result<(), String> {
    let mut ff = Command::new("ffmpeg")
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "bgr0",
            "-s",
            &format!("{w}x{h}"),
            "-r",
            &fps.to_string(),
            "-i",
            "-",
        ])
        .arg("-i")
        .arg(input)
        .args([
            "-map",
            "0:v",
            "-map",
            "1:a",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
        ])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("starting ffmpeg: {e} (is it installed and on the PATH?)"))?;
    let mut stdin = ff.stdin.take().unwrap();
    let mono = pcm.mono();
    let mut r = Renderer::new(style.clone());
    let mut c = Canvas::new(w, h);
    let mut bytes = vec![0u8; w * h * 4];
    let mut err = None;
    let total = pcm.duration();
    walk(&mono, pcm.rate, settings, fps, 0.0, total, |f, i| {
        if err.is_some() {
            return;
        }
        r.draw(&mut c, f, (1.0 / fps) as f32);
        for (d, p) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(&c.px) {
            d.copy_from_slice(&p.to_le_bytes());
        }
        if let Err(e) = stdin.write_all(&bytes) {
            err = Some(format!("writing to ffmpeg: {e}"));
        }
        if i % (fps as usize * 10).max(1) == 0 {
            eprint!("\r{:.0} of {:.0} s", i as f64 / fps, total);
        }
    });
    drop(stdin);
    eprintln!();
    let st = ff.wait().map_err(|e| e.to_string())?;
    if let Some(e) = err {
        return Err(e);
    }
    if !st.success() {
        return Err(format!("ffmpeg exited with {st}"));
    }
    Ok(())
}
