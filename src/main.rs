use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use minifb::{Key, KeyRepeat, Window, WindowOptions};
use mviz::analysis::Analyzer;
use mviz::audio::{self, Source};
use mviz::config::Config;
use mviz::offline;
use mviz::render::{Canvas, Effect, Palette, Renderer};

#[derive(Parser)]
#[command(name = "mviz", version, about = "Real-time music visualizer for system audio, a microphone or a file")]
struct Cli {
    /// Audio file to play and show (MP3, M4A/AAC, FLAC, Ogg Vorbis, WAV); default: what the computer is playing
    file: Option<PathBuf>,
    /// Use the default microphone (or --device) instead of what the computer is playing
    #[arg(long)]
    mic: bool,
    /// Device to capture from (part of its name; see --list-devices)
    #[arg(long)]
    device: Option<String>,
    #[arg(long)]
    list_devices: bool,
    /// Show a file without playing it
    #[arg(long)]
    silent: bool,
    /// Effects from top to bottom: bars, mirror, line, wave, spectrogram, radial (e.g. spectrogram,bars)
    #[arg(short, long, value_delimiter = ',')]
    effect: Vec<String>,
    /// Palette: fire, ice, neon, forest, mono, sunset
    #[arg(short, long)]
    palette: Option<String>,
    /// Configuration file (TOML; see the README)
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Size, as WIDTHxHEIGHT
    #[arg(long)]
    size: Option<String>,
    /// Write one frame of the file to this PNG instead of opening a window (with --at)
    #[arg(long)]
    render: Option<PathBuf>,
    /// Seconds into the file for --render
    #[arg(long, default_value_t = 30.0)]
    at: f64,
    /// Render the whole file to a video with ffmpeg (for example out.mp4)
    #[arg(long)]
    export: Option<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mviz: {e}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    if cli.list_devices {
        for d in audio::list_devices()? {
            println!("{d}");
        }
        return Ok(());
    }
    let mut cfg = match &cli.config {
        Some(p) => Config::parse(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)
            .map_err(|e| format!("{}: {e}", p.display()))?,
        None => Config::default(),
    };
    if !cli.effect.is_empty() {
        cfg.style.panels = cli
            .effect
            .iter()
            .map(|e| Effect::parse(e).map(|x| (x, 1.0)).ok_or_else(|| format!("unknown effect '{e}'")))
            .collect::<Result<_, _>>()?;
    }
    if let Some(p) = &cli.palette {
        cfg.style.palette = Palette::named(p).ok_or_else(|| format!("unknown palette '{p}'"))?;
    }
    if let Some(s) = &cli.size {
        let (w, h) = s
            .split_once('x')
            .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
            .ok_or("--size must look like 1280x720")?;
        cfg.width = w;
        cfg.height = h;
    }
    if cli.render.is_some() || cli.export.is_some() {
        let file = cli.file.as_ref().ok_or("--render and --export need a file")?;
        let pcm = audio::decode(file)?;
        if let Some(out) = &cli.render {
            let c =
                offline::render_at(&pcm, &cfg.analysis, &cfg.style, cfg.fps, cli.at.min(pcm.duration()), cfg.width, cfg.height);
            std::fs::write(out, offline::png(&c)).map_err(|e| format!("{}: {e}", out.display()))?;
        }
        if let Some(out) = &cli.export {
            offline::export(file, &pcm, &cfg.analysis, &cfg.style, cfg.fps, cfg.width, cfg.height, out)?;
        }
        return Ok(());
    }
    let source = match &cli.file {
        Some(f) => Source::file(f, !cli.silent)?,
        None if cli.mic => Source::microphone(cli.device.as_deref())?,
        None => Source::loopback(cli.device.as_deref())?,
    };
    audio::warm_up();
    window_loop(source, cfg)
}

fn window_loop(source: Source, cfg: Config) -> Result<(), String> {
    let title = format!("mviz - {}", source.name());
    let mut win = Window::new(&title, cfg.width, cfg.height, WindowOptions { resize: true, ..WindowOptions::default() })
        .map_err(|e| e.to_string())?;
    win.set_target_fps(cfg.fps.round() as usize);
    let mut analyzer = Analyzer::new(cfg.analysis.clone(), source.rate());
    let mut renderer = Renderer::new(cfg.style.clone());
    let mut canvas = Canvas::new(cfg.width, cfg.height);
    let mut samples = Vec::new();
    let mut last = Instant::now();
    let mut palette_i = Palette::names().iter().position(|n| Palette::named(n).as_ref() == Some(&cfg.style.palette)).unwrap_or(0);
    let mut floor = cfg.analysis.db_floor;
    while win.is_open() && !win.is_key_down(Key::Escape) && !win.is_key_down(Key::Q) {
        let now = Instant::now();
        let dt = now.duration_since(last).as_secs_f64().min(0.1);
        last = now;
        for k in win.get_keys_pressed(KeyRepeat::No) {
            match k {
                Key::Key1 | Key::Key2 | Key::Key3 | Key::Key4 | Key::Key5 | Key::Key6 => {
                    let i = k as usize - Key::Key1 as usize;
                    renderer.set_panels(vec![(Effect::ALL[i], 1.0)]);
                }
                Key::Key0 => renderer.set_panels(vec![(Effect::Spectrogram, 1.0), (Effect::Bars, 2.0)]),
                Key::P => {
                    palette_i = (palette_i + 1) % Palette::names().len();
                    renderer.style.palette = Palette::named(Palette::names()[palette_i]).unwrap();
                }
                Key::H => renderer.style.hud = !renderer.style.hud,
                Key::F => renderer.style.flash = !renderer.style.flash,
                Key::Space => source.toggle_pause(),
                Key::Left => source.seek(-5.0),
                Key::Right => source.seek(5.0),
                // Up and Down change the sensitivity: the level that counts as silence.
                Key::Up | Key::Down => {
                    floor = (floor + if k == Key::Up { 5.0 } else { -5.0 }).clamp(-120.0, -20.0);
                    let s = mviz::analysis::Settings { db_floor: floor, ..analyzer.settings().clone() };
                    analyzer = Analyzer::new(s, source.rate());
                }
                _ => {}
            }
        }
        let (w, h) = win.get_size();
        canvas.resize(w.max(1), h.max(1));
        source.latest(cfg.analysis.fft_size, dt, &mut samples);
        let frame = analyzer.analyze(&samples, dt);
        renderer.draw(&mut canvas, &frame, dt as f32);
        win.update_with_buffer(&canvas.px, canvas.w, canvas.h).map_err(|e| e.to_string())?;
        if source.finished() {
            std::thread::sleep(Duration::from_millis(500));
            break;
        }
    }
    Ok(())
}
