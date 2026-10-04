//! Software rendering into a 0xRRGGBB pixel buffer: a few primitives (rectangles, anti-aliased
//! lines, circles, a small bitmap font) and the effects built from them.

use crate::analysis::Frame;

#[derive(Clone, Debug, PartialEq)]
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(h, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    fn u32(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }

    fn lerp(self, o: Rgb, t: f32) -> Rgb {
        let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8;
        Rgb(m(self.0, o.0), m(self.1, o.1), m(self.2, o.2))
    }

    pub fn scale(self, k: f32) -> Rgb {
        let m = |a: u8| (a as f32 * k).round().clamp(0.0, 255.0) as u8;
        Rgb(m(self.0), m(self.1), m(self.2))
    }
}

/// A colour gradient: evenly spaced stops sampled from 0 to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette(pub Vec<Rgb>);

impl Palette {
    pub fn named(name: &str) -> Option<Palette> {
        let p = |v: &[&str]| Palette(v.iter().map(|s| Rgb::parse(s).unwrap()).collect());
        Some(match name {
            "fire" => p(&["#1a0500", "#a01e00", "#ff5a00", "#ffb300", "#fff5c0"]),
            "ice" => p(&["#020b1c", "#0b3d91", "#1f8fff", "#7fd8ff", "#eaffff"]),
            "neon" => p(&["#12002a", "#7a00ff", "#ff00c8", "#00e5ff", "#c8ff00"]),
            "forest" => p(&["#021204", "#0b5d1e", "#2fa84f", "#a7e163", "#f3ffbd"]),
            "mono" => p(&["#101010", "#606060", "#b0b0b0", "#ffffff"]),
            "sunset" => p(&["#0d0221", "#541388", "#d90368", "#f18805", "#ffd400"]),
            _ => return None,
        })
    }

    pub fn names() -> &'static [&'static str] {
        &["fire", "ice", "neon", "forest", "mono", "sunset"]
    }

    pub fn at(&self, t: f32) -> Rgb {
        let n = self.0.len();
        if n == 1 {
            return self.0[0];
        }
        let x = t.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = (x.floor() as usize).min(n - 2);
        self.0[i].lerp(self.0[i + 1], x - i as f32)
    }
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas { w, h, px: vec![0; w * h] }
    }

    pub fn resize(&mut self, w: usize, h: usize) {
        if (w, h) != (self.w, self.h) {
            *self = Canvas::new(w, h);
        }
    }

    pub fn clear(&mut self, c: Rgb) {
        self.px.fill(c.u32());
    }

    pub fn get(&self, x: usize, y: usize) -> Rgb {
        let v = self.px[y * self.w + x];
        Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }

    /// Blends `c` over the pixel with coverage `a` (0..1), in 8-bit fixed point.
    #[inline]
    fn blend(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let i = y as usize * self.w + x as usize;
        self.px[i] = mix(self.px[i], c, (a.min(1.0) * 256.0) as u32);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, c: Rgb) {
        let (x0, y0) = (x.round().max(0.0) as usize, y.round().max(0.0) as usize);
        let x1 = ((x + w).round().max(0.0) as usize).min(self.w);
        let y1 = ((y + h).round().max(0.0) as usize).min(self.h);
        let v = c.u32();
        for row in y0..y1 {
            if x0 < x1 {
                self.px[row * self.w + x0..row * self.w + x1].fill(v);
            }
        }
    }

    /// An anti-aliased line of the given width (Xiaolin Wu's algorithm, drawn several times side
    /// by side for widths above one pixel).
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, width: f32, c: Rgb) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len = (dx * dx + dy * dy).sqrt().max(1e-6);
        let (nx, ny) = (-dy / len, dx / len);
        let passes = width.max(1.0).round() as i32;
        for p in 0..passes {
            let off = p as f32 - (passes - 1) as f32 / 2.0;
            self.wu(x0 + nx * off, y0 + ny * off, x1 + nx * off, y1 + ny * off, c);
        }
    }

    fn wu(&mut self, mut x0: f32, mut y0: f32, mut x1: f32, mut y1: f32, c: Rgb) {
        let steep = (y1 - y0).abs() > (x1 - x0).abs();
        if steep {
            std::mem::swap(&mut x0, &mut y0);
            std::mem::swap(&mut x1, &mut y1);
        }
        if x0 > x1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
        }
        let grad = if x1 - x0 == 0.0 { 0.0 } else { (y1 - y0) / (x1 - x0) };
        let mut y = y0 + grad * (x0.round() - x0);
        let (xa, xb) = (x0.round() as i32, x1.round() as i32);
        // A hard cap so a wild coordinate cannot make a huge loop.
        if xb - xa > 20_000 {
            return;
        }
        for x in xa..=xb {
            let (fy, frac) = (y.floor(), y - y.floor());
            if steep {
                self.blend(fy as i32, x, c, 1.0 - frac);
                self.blend(fy as i32 + 1, x, c, frac);
            } else {
                self.blend(x, fy as i32, c, 1.0 - frac);
                self.blend(x, fy as i32 + 1, c, frac);
            }
            y += grad;
        }
    }

    /// A filled circle with an anti-aliased edge.
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgb) {
        let (x0, x1) = ((cx - r - 1.0).floor() as i32, (cx + r + 1.0).ceil() as i32);
        let (y0, y1) = ((cy - r - 1.0).floor() as i32, (cy + r + 1.0).ceil() as i32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                self.blend(x, y, c, (r - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// Text in a 5x7 font, scaled by `size` (pixels per font pixel). Unknown characters are blank.
    pub fn text(&mut self, x: f32, y: f32, s: &str, size: f32, c: Rgb) {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(rows) = glyph(ch.to_ascii_uppercase()) {
                for (ry, bits) in rows.iter().enumerate() {
                    for rx in 0..5 {
                        if bits & (0x10 >> rx) != 0 {
                            self.fill_rect(cx + rx as f32 * size, y + ry as f32 * size, size, size, c);
                        }
                    }
                }
            }
            cx += 6.0 * size;
        }
    }
}

/// `old` moved towards `c` by `a` / 256, per channel.
#[inline]
fn mix(old: u32, c: Rgb, a: u32) -> u32 {
    let ch = |o: u32, n: u8| -> u32 {
        let (o, n) = (o as i32, n as i32);
        (o + (((n - o) * a as i32) >> 8)) as u32
    };
    ch(old >> 16 & 0xff, c.0) << 16 | ch(old >> 8 & 0xff, c.1) << 8 | ch(old & 0xff, c.2)
}

/// 5x7 glyphs, one byte per row, the low five bits from left to right.
fn glyph(c: char) -> Option<[u8; 7]> {
    Some(match c {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1c, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1c],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0a, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '.' => [0, 0, 0, 0, 0, 0x0c, 0x0c],
        ':' => [0, 0x0c, 0x0c, 0, 0x0c, 0x0c, 0],
        '-' => [0, 0, 0, 0x1f, 0, 0, 0],
        '+' => [0, 0x04, 0x04, 0x1f, 0x04, 0x04, 0],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        ' ' => [0; 7],
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Bars,
    Mirror,
    Line,
    Wave,
    Spectrogram,
    Radial,
}

impl Effect {
    pub const ALL: [Effect; 6] = [Effect::Bars, Effect::Mirror, Effect::Line, Effect::Wave, Effect::Spectrogram, Effect::Radial];

    pub fn parse(s: &str) -> Option<Effect> {
        Some(match s {
            "bars" => Effect::Bars,
            "mirror" => Effect::Mirror,
            "line" => Effect::Line,
            "wave" | "scope" => Effect::Wave,
            "spectrogram" | "waterfall" => Effect::Spectrogram,
            "radial" | "circle" => Effect::Radial,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Effect::Bars => "bars",
            Effect::Mirror => "mirror",
            Effect::Line => "line",
            Effect::Wave => "wave",
            Effect::Spectrogram => "spectrogram",
            Effect::Radial => "radial",
        }
    }
}

/// What to draw and how it looks.
#[derive(Clone, Debug)]
pub struct Style {
    /// Panels from top to bottom, with relative heights.
    pub panels: Vec<(Effect, f32)>,
    pub palette: Palette,
    pub background: Rgb,
    /// Brighten the background on beats.
    pub flash: bool,
    /// Gap between bars, as a fraction of the bar pitch.
    pub bar_gap: f32,
    pub peaks: bool,
    /// Draw the tempo and level in the corner.
    pub hud: bool,
    /// Spectrogram scroll speed in pixels per second.
    pub scroll: f32,
    /// Oscilloscope gain.
    pub wave_gain: f32,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            panels: vec![(Effect::Bars, 1.0)],
            palette: Palette::named("neon").unwrap(),
            background: Rgb(8, 6, 16),
            flash: true,
            bar_gap: 0.2,
            peaks: true,
            hud: true,
            scroll: 120.0,
            wave_gain: 1.0,
        }
    }
}

/// Draws frames; keeps what effects remember between frames (the spectrogram's history, the
/// beat flash, the radial rotation).
pub struct Renderer {
    pub style: Style,
    flash: f32,
    spin: f32,
    history: Vec<Option<Canvas>>,
    scroll_carry: f32,
}

impl Renderer {
    pub fn new(style: Style) -> Renderer {
        let n = style.panels.len();
        Renderer { style, flash: 0.0, spin: 0.0, history: vec![None; n], scroll_carry: 0.0 }
    }

    pub fn set_panels(&mut self, panels: Vec<(Effect, f32)>) {
        self.history = vec![None; panels.len()];
        self.style.panels = panels;
    }

    pub fn draw(&mut self, c: &mut Canvas, f: &Frame, dt: f32) {
        if f.beat {
            self.flash = 1.0;
        } else {
            self.flash *= (-dt / 0.15).exp();
        }
        self.spin += dt * (0.15 + 0.6 * self.flash);
        let bg = if self.style.flash {
            self.style.background.lerp(self.style.palette.at(0.5), 0.12 * self.flash)
        } else {
            self.style.background
        };
        c.clear(bg);
        if c.w == 0 || c.h == 0 {
            return;
        }
        let total: f32 = self.style.panels.iter().map(|p| p.1.max(0.0)).sum::<f32>().max(1e-6);
        let mut y = 0.0;
        self.scroll_carry += self.style.scroll * dt;
        let scroll = self.scroll_carry.floor() as usize;
        self.scroll_carry -= scroll as f32;
        for i in 0..self.style.panels.len() {
            let (effect, weight) = self.style.panels[i];
            let h = c.h as f32 * weight.max(0.0) / total;
            let r = Rect { x: 0.0, y, w: c.w as f32, h };
            match effect {
                Effect::Bars => self.bars(c, f, r),
                Effect::Mirror => self.mirror(c, f, r),
                Effect::Line => self.line(c, f, r),
                Effect::Wave => self.wave(c, f, r),
                Effect::Spectrogram => self.spectrogram(c, f, r, i, scroll),
                Effect::Radial => self.radial(c, f, r),
            }
            y += h;
        }
        if self.style.hud && c.w >= 120 && c.h >= 40 {
            let size = (c.h as f32 / 270.0).clamp(1.0, 4.0).round();
            let mut s = format!("{:>5.1} DB", f.rms_db.max(-99.0));
            if let Some(b) = f.bpm {
                s = format!("{b:.0} BPM  {s}");
            }
            let tw = s.len() as f32 * 6.0 * size;
            c.text(c.w as f32 - tw - 8.0 * size, 6.0 * size, &s, size, self.style.palette.at(0.85));
        }
    }

    fn bars(&self, c: &mut Canvas, f: &Frame, r: Rect) {
        let n = f.bands.len().max(1) as f32;
        let pitch = r.w / n;
        let bw = (pitch * (1.0 - self.style.bar_gap)).max(1.0);
        for (i, &v) in f.bands.iter().enumerate() {
            let x = r.x + i as f32 * pitch + (pitch - bw) / 2.0;
            let top = r.y + r.h * (1.0 - v);
            // Colour by height: the gradient runs up the panel, so tall bars reach the bright end.
            let rows = (r.y + r.h).round() as i32 - top.round() as i32;
            for k in 0..rows.max(0) {
                let yy = top.round() + k as f32;
                let t = 1.0 - (yy - r.y) / r.h.max(1.0);
                c.fill_rect(x, yy, bw, 1.0, self.style.palette.at(0.15 + 0.85 * t));
            }
            if self.style.peaks && f.peaks.get(i).is_some_and(|&p| p > 0.01) {
                let py = r.y + r.h * (1.0 - f.peaks[i]);
                c.fill_rect(x, py - 2.0, bw, 2.0, self.style.palette.at(1.0));
            }
        }
    }

    fn mirror(&self, c: &mut Canvas, f: &Frame, r: Rect) {
        let n = f.bands.len().max(1) as f32;
        let pitch = r.w / n;
        let bw = (pitch * (1.0 - self.style.bar_gap)).max(1.0);
        let mid = r.y + r.h / 2.0;
        for (i, &v) in f.bands.iter().enumerate() {
            let x = r.x + i as f32 * pitch + (pitch - bw) / 2.0;
            let half = v * r.h / 2.0;
            let col = self.style.palette.at(0.25 + 0.75 * (i as f32 / n));
            c.fill_rect(x, mid - half, bw, half, col);
            c.fill_rect(x, mid, bw, half, col.scale(0.55));
        }
    }

    fn line(&self, c: &mut Canvas, f: &Frame, r: Rect) {
        let n = f.bands.len();
        if n < 2 {
            return;
        }
        // Catmull-Rom through the band values, sampled once per pixel column.
        let at = |x: f32| -> f32 {
            let p = x / r.w * (n - 1) as f32;
            let i = (p.floor() as usize).min(n - 2);
            let t = p - i as f32;
            let g = |k: isize| f.bands[(i as isize + k).clamp(0, n as isize - 1) as usize];
            let (p0, p1, p2, p3) = (g(-1), g(0), g(1), g(2));
            let v = 0.5
                * (2.0 * p1
                    + (-p0 + p2) * t
                    + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
                    + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t);
            v.clamp(0.0, 1.0)
        };
        let base = r.y + r.h;
        let mut prev: Option<(f32, f32)> = None;
        for xi in 0..r.w.round() as i32 {
            let x = r.x + xi as f32;
            let v = at(xi as f32);
            let y = base - v * r.h * 0.95;
            // Fill under the curve, fading towards the bottom (one colour per column).
            let rows = (base - y).round() as i32;
            let col = self.style.palette.at(0.2 + 0.6 * v);
            let (xi, y0) = (x as i32, y as i32);
            if xi >= 0 && (xi as usize) < c.w {
                let inv = 1.0 / rows.max(1) as f32;
                for k in 0..rows.max(0) {
                    let yy = y0 + k;
                    if yy < 0 || yy as usize >= c.h {
                        continue;
                    }
                    let t = 1.0 - k as f32 * inv;
                    let i = yy as usize * c.w + xi as usize;
                    c.px[i] = mix(c.px[i], col, ((0.15 + 0.55 * t * t) * 256.0) as u32);
                }
            }
            if let Some((px, py)) = prev {
                c.line(px, py, x, y, 2.0, self.style.palette.at(0.95));
            }
            prev = Some((x, y));
        }
    }

    fn wave(&self, c: &mut Canvas, f: &Frame, r: Rect) {
        let n = f.wave.len();
        if n < 2 {
            return;
        }
        let mid = r.y + r.h / 2.0;
        let level = ((f.rms_db + 40.0) / 40.0).clamp(0.0, 1.0);
        let col = self.style.palette.at(0.5 + 0.5 * level);
        c.line(r.x, mid, r.x + r.w, mid, 1.0, self.style.palette.at(0.2));
        // Scale the trace to its own peak (up to 20 times), so quiet music still fills the panel.
        let peak = f.wave.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let gain = self.style.wave_gain * (0.9 / peak.max(0.045)).min(20.0);
        let mut prev = None;
        for (i, &s) in f.wave.iter().enumerate() {
            let x = r.x + r.w * i as f32 / (n - 1) as f32;
            let y = mid - (s * gain).clamp(-1.0, 1.0) * r.h * 0.45;
            if let Some((px, py)) = prev {
                c.line(px, py, x, y, 2.0, col);
            }
            prev = Some((x, y));
        }
    }

    fn spectrogram(&mut self, c: &mut Canvas, f: &Frame, r: Rect, slot: usize, scroll: usize) {
        let (w, h) = (r.w.round() as usize, r.h.round() as usize);
        if w == 0 || h == 0 {
            return;
        }
        let empty = self.style.palette.at(0.0);
        let hist = self.history[slot].get_or_insert_with(|| {
            let mut c = Canvas::new(w, h);
            c.clear(empty);
            c
        });
        if hist.w != w || hist.h != h {
            *hist = Canvas::new(w, h);
            hist.clear(empty);
        }
        let s = scroll.min(w);
        if s > 0 {
            for row in 0..h {
                hist.px[row * w..(row + 1) * w].copy_within(s.., 0);
            }
            let n = f.band_db.len();
            // New columns: each row is a frequency (low at the bottom), from the raw band levels.
            for row in 0..h {
                let p = (1.0 - (row as f32 + 0.5) / h as f32) * (n.max(1) - 1) as f32;
                let i = (p.floor() as usize).min(n.saturating_sub(2));
                let t = p - i as f32;
                let db = if n >= 2 { f.band_db[i] * (1.0 - t) + f.band_db[i + 1] * t } else { -120.0 };
                let v = ((db + 70.0) / 70.0).clamp(0.0, 1.0);
                let col = self.style.palette.at(v * v).u32();
                hist.px[row * w + w - s..(row + 1) * w].fill(col);
            }
        }
        let y0 = r.y.round() as usize;
        for row in 0..h.min(c.h.saturating_sub(y0)) {
            let dst = (y0 + row) * c.w;
            let len = w.min(c.w);
            c.px[dst..dst + len].copy_from_slice(&hist.px[row * w..row * w + len]);
        }
    }

    fn radial(&self, c: &mut Canvas, f: &Frame, r: Rect) {
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        let m = r.w.min(r.h);
        let base = m * (0.16 + 0.03 * self.flash);
        let n = f.bands.len();
        if n == 0 {
            return;
        }
        let level = ((f.rms_db + 40.0) / 40.0).clamp(0.0, 1.0);
        c.circle(cx, cy, base * 0.92, self.style.palette.at(0.15 + 0.5 * level));
        // Each band twice, mirrored left and right, low frequencies at the bottom.
        let count = n * 2;
        let width = (std::f32::consts::TAU * base / count as f32 * 0.6).max(1.0);
        for k in 0..count {
            let band = if k < n { k } else { count - 1 - k };
            let v = f.bands[band];
            let a = std::f32::consts::PI / 2.0 + std::f32::consts::TAU * (k as f32 + 0.5) / count as f32 + self.spin;
            let (sx, sy) = (a.cos(), a.sin());
            let len = 2.0 + v * m * 0.3;
            let col = self.style.palette.at(0.3 + 0.7 * v);
            c.line(cx + sx * base, cy + sy * base, cx + sx * (base + len), cy + sy * (base + len), width, col);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(bands: Vec<f32>) -> Frame {
        Frame {
            peaks: bands.clone(),
            band_db: bands.iter().map(|v| v * 70.0 - 70.0).collect(),
            bands,
            wave: vec![0.0; 64],
            rms_db: -20.0,
            ..Frame::default()
        }
    }

    #[test]
    fn palettes_hit_their_end_stops() {
        for name in Palette::names() {
            let p = Palette::named(name).unwrap();
            assert_eq!(p.at(0.0), p.0[0]);
            assert_eq!(p.at(1.0), *p.0.last().unwrap());
            assert_eq!(p.at(2.0), *p.0.last().unwrap());
        }
    }

    #[test]
    fn bar_heights_follow_the_levels() {
        let mut r = Renderer::new(Style { hud: false, flash: false, peaks: false, bar_gap: 0.0, ..Style::default() });
        let mut c = Canvas::new(40, 100);
        r.draw(&mut c, &frame(vec![0.0, 0.25, 0.5, 1.0]), 0.016);
        let bg = r.style.background;
        let height = |col: usize| (0..100).filter(|&y| c.get(col, y) != bg).count();
        assert_eq!(height(5), 0);
        assert_eq!(height(15), 25);
        assert_eq!(height(25), 50);
        assert_eq!(height(35), 100);
    }

    #[test]
    fn every_effect_draws_at_awkward_sizes() {
        for e in Effect::ALL {
            let mut r = Renderer::new(Style { panels: vec![(e, 1.0), (Effect::Bars, 0.5)], ..Style::default() });
            for (w, h) in [(1, 1), (3, 2), (2000, 3), (7, 900), (640, 360)] {
                let mut c = Canvas::new(w, h);
                let mut f = frame((0..64).map(|i| i as f32 / 63.0).collect());
                f.beat = true;
                f.bpm = Some(128.0);
                f.wave = (0..1024).map(|i| (i as f32 / 20.0).sin()).collect();
                for _ in 0..3 {
                    r.draw(&mut c, &f, 0.02);
                }
            }
        }
    }

    #[test]
    fn spectrogram_scrolls_by_time() {
        let mut r = Renderer::new(Style {
            panels: vec![(Effect::Spectrogram, 1.0)],
            hud: false,
            flash: false,
            scroll: 100.0,
            ..Style::default()
        });
        let mut c = Canvas::new(50, 20);
        let loud = frame(vec![1.0; 8]);
        r.draw(&mut c, &loud, 0.05); // 5 columns of loud
        let quiet = frame(vec![0.0; 8]);
        r.draw(&mut c, &quiet, 0.03); // 3 columns of quiet
        let bright = r.style.palette.at(1.0);
        let lit: Vec<bool> = (0..50).map(|x| c.get(x, 10) == bright).collect();
        assert_eq!(lit.iter().filter(|&&b| b).count(), 5);
        assert!(lit[42..47].iter().all(|&b| b) && !lit[47]);
    }

    #[test]
    fn text_draws_known_glyphs() {
        let mut c = Canvas::new(20, 10);
        c.text(0.0, 0.0, "1", 1.0, Rgb(255, 255, 255));
        let on: usize = c.px.iter().filter(|&&p| p != 0).count();
        assert_eq!(on, 10, "the glyph for 1 has ten pixels");
    }
}
