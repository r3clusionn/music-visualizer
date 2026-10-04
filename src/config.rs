//! The configuration file: look (panels, palette, options) and analysis settings.

use serde::Deserialize;

use crate::analysis::Settings;
use crate::render::{Effect, Palette, Rgb, Style};

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct File {
    width: usize,
    height: usize,
    fps: f64,
    palette: PaletteSpec,
    background: String,
    panels: Vec<Panel>,
    flash: bool,
    peaks: bool,
    hud: bool,
    bar_gap: f32,
    scroll: f32,
    wave_gain: f32,
    analysis: Analysis,
}

impl Default for File {
    fn default() -> File {
        File {
            width: 1280,
            height: 720,
            fps: 60.0,
            palette: PaletteSpec::Name("neon".into()),
            background: "#080610".into(),
            panels: vec![Panel { effect: "bars".into(), weight: 1.0 }],
            flash: true,
            peaks: true,
            hud: true,
            bar_gap: 0.2,
            scroll: 120.0,
            wave_gain: 1.0,
            analysis: Analysis::default(),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PaletteSpec {
    Name(String),
    Stops(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Panel {
    effect: String,
    #[serde(default = "one")]
    weight: f32,
}

fn one() -> f32 {
    1.0
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Analysis {
    fft_size: usize,
    bands: usize,
    min_hz: f64,
    max_hz: f64,
    db_floor: f64,
    db_ceil: f64,
    attack: f64,
    release: f64,
    peak_hold: f64,
    peak_fall: f64,
    beat_sensitivity: f64,
}

impl Default for Analysis {
    fn default() -> Analysis {
        let s = Settings::default();
        Analysis {
            fft_size: s.fft_size,
            bands: s.bands,
            min_hz: s.min_hz,
            max_hz: s.max_hz,
            db_floor: s.db_floor,
            db_ceil: s.db_ceil,
            attack: s.attack,
            release: s.release,
            peak_hold: s.peak_hold,
            peak_fall: s.peak_fall,
            beat_sensitivity: s.beat_sensitivity,
        }
    }
}

#[derive(Debug)]
pub struct Config {
    pub width: usize,
    pub height: usize,
    pub fps: f64,
    pub style: Style,
    pub analysis: Settings,
}

impl Default for Config {
    fn default() -> Config {
        Config::parse("").unwrap()
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, String> {
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        let palette = match f.palette {
            PaletteSpec::Name(n) => {
                Palette::named(&n).ok_or_else(|| format!("palette: unknown '{n}' (one of {})", Palette::names().join(", ")))?
            }
            PaletteSpec::Stops(v) => {
                if v.len() < 2 {
                    return Err("palette: give at least two colours".into());
                }
                Palette(
                    v.iter()
                        .map(|s| Rgb::parse(s).ok_or_else(|| format!("palette: bad colour '{s}' (use #rrggbb)")))
                        .collect::<Result<_, _>>()?,
                )
            }
        };
        let panels = f
            .panels
            .iter()
            .map(|p| {
                Effect::parse(&p.effect).map(|e| (e, p.weight)).ok_or_else(|| format!("panels: unknown effect '{}'", p.effect))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if panels.is_empty() {
            return Err("panels: give at least one".into());
        }
        let a = f.analysis;
        if !a.fft_size.is_power_of_two() || !(512..=32768).contains(&a.fft_size) {
            return Err("analysis.fft_size: a power of two from 512 to 32768".into());
        }
        if a.bands == 0 || a.bands > 1024 || a.min_hz <= 0.0 || a.max_hz <= a.min_hz || a.db_ceil <= a.db_floor {
            return Err("analysis: bands 1 to 1024, 0 < min_hz < max_hz, db_floor < db_ceil".into());
        }
        if f.width == 0 || f.height == 0 || !(1.0..=500.0).contains(&f.fps) {
            return Err("width and height above 0, fps from 1 to 500".into());
        }
        Ok(Config {
            width: f.width,
            height: f.height,
            fps: f.fps,
            style: Style {
                panels,
                palette,
                background: Rgb::parse(&f.background).ok_or("background: use #rrggbb")?,
                flash: f.flash,
                bar_gap: f.bar_gap.clamp(0.0, 0.9),
                peaks: f.peaks,
                hud: f.hud,
                scroll: f.scroll.max(0.0),
                wave_gain: f.wave_gain,
            },
            analysis: Settings {
                fft_size: a.fft_size,
                bands: a.bands,
                min_hz: a.min_hz,
                max_hz: a.max_hz,
                db_floor: a.db_floor,
                db_ceil: a.db_ceil,
                attack: a.attack.max(1e-4),
                release: a.release.max(1e-4),
                peak_hold: a.peak_hold,
                peak_fall: a.peak_fall,
                beat_sensitivity: a.beat_sensitivity,
                ..Settings::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let c = Config::default();
        assert_eq!(c.style.panels, vec![(Effect::Bars, 1.0)]);
        let c = Config::parse("palette = [\"#000000\", \"#ffffff\"]\npanels = [{ effect = \"spectrogram\" }, { effect = \"bars\", weight = 2 }]\n[analysis]\nbands = 32\n").unwrap();
        assert_eq!(c.style.palette.0.len(), 2);
        assert_eq!(c.style.panels, vec![(Effect::Spectrogram, 1.0), (Effect::Bars, 2.0)]);
        assert_eq!(c.analysis.bands, 32);
    }

    #[test]
    fn errors_say_what_is_wrong() {
        assert!(Config::parse("palette = \"plaid\"").unwrap_err().contains("plaid"));
        assert!(Config::parse("panels = [{ effect = \"lasers\" }]").unwrap_err().contains("lasers"));
        assert!(Config::parse("[analysis]\nfft_size = 1000").unwrap_err().contains("power of two"));
        assert!(Config::parse("colour = 1").is_err());
    }
}
