//! Drawing primitives on [`Ui`]. Everything takes logical px and converts to device px.
//!
//! - Shapes go through tiny-skia (anti-aliased paths); the clip is a rectangle stack
//!   turned into a reusable mask only while a clip is active.
//! - Text and shadows are coverage masks blended straight into the premultiplied canvas.
//! - Soft shadows are a real blur (3 box passes ≈ Gaussian), cached per size.

use std::sync::OnceLock;

use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LinearGradient, Mask, Paint, PathBuilder, Pattern,
    Pixmap, RadialGradient, Shader, SpreadMode, Stroke, Transform,
};

use crate::ctx::Ui;
use crate::geom::Rect;
use crate::text::Weight;
use crate::theme::Rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub(crate) fn rrect_path(r: Rect, rad: f32) -> Option<tiny_skia::Path> {
    if r.is_empty() {
        return None;
    }
    let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    if rad < 0.01 {
        return Some(PathBuilder::from_rect(tiny_skia::Rect::from_xywh(
            r.x, r.y, r.w, r.h,
        )?));
    }
    // Cubic Bézier circle approximation: control points 0.5523·r from the corner ends.
    let k = 0.4477 * rad;
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + rad, y0);
    pb.close();
    pb.finish()
}

fn solid(c: Rgba) -> Paint<'static> {
    Paint {
        shader: Shader::SolidColor(c.to_skia()),
        anti_alias: true,
        ..Paint::default()
    }
}

/// Coverage curves: light text on dark backgrounds reads thin with linear blending, so
/// its coverage is lifted; dark text is left almost linear.
fn lut(light_text: bool) -> &'static [u8; 256] {
    static LIGHT: OnceLock<[u8; 256]> = OnceLock::new();
    static DARK: OnceLock<[u8; 256]> = OnceLock::new();
    let make = |g: f32| {
        let mut t = [0u8; 256];
        for (i, v) in t.iter_mut().enumerate() {
            *v = ((i as f32 / 255.0).powf(g) * 255.0).round() as u8;
        }
        t
    };
    if light_text {
        LIGHT.get_or_init(|| make(0.78))
    } else {
        DARK.get_or_init(|| make(0.94))
    }
}

/// Blend a coverage mask of `color` into `pm` at device (`x0`, `y0`), inside `clip`.
#[allow(clippy::too_many_arguments)]
fn blend_cov(
    pm: &mut Pixmap,
    x0: i32,
    y0: i32,
    w: u32,
    h: u32,
    cov: &[u8],
    color: Rgba,
    clip: Rect,
    curve: Option<&[u8; 256]>,
) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let cx0 = (clip.x.floor() as i32).max(0);
    let cy0 = (clip.y.floor() as i32).max(0);
    let cx1 = (clip.right().ceil() as i32).min(pw);
    let cy1 = (clip.bottom().ceil() as i32).min(ph);
    let xs = x0.max(cx0);
    let xe = (x0 + w as i32).min(cx1);
    let ys = y0.max(cy0);
    let ye = (y0 + h as i32).min(cy1);
    if xs >= xe || ys >= ye {
        return;
    }
    let (sr, sg, sb, sa) = (
        color.r as u32,
        color.g as u32,
        color.b as u32,
        color.a as u32,
    );
    let data = pm.data_mut();
    for y in ys..ye {
        let row = ((y - y0) as u32 * w) as usize;
        let drow = (y * pw) as usize * 4;
        for x in xs..xe {
            let mut c = cov[row + (x - x0) as usize] as u32;
            if c == 0 {
                continue;
            }
            if let Some(t) = curve {
                c = t[c as usize] as u32;
            }
            // Source alpha after coverage, 0–255.
            let a = (sa * c + 127) / 255;
            if a == 0 {
                continue;
            }
            let inv = 255 - a;
            let i = drow + x as usize * 4;
            let d = &mut data[i..i + 4];
            d[0] = ((sr * a + d[0] as u32 * inv + 127) / 255) as u8;
            d[1] = ((sg * a + d[1] as u32 * inv + 127) / 255) as u8;
            d[2] = ((sb * a + d[2] as u32 * inv + 127) / 255) as u8;
            d[3] = ((a * 255 + d[3] as u32 * inv + 127) / 255) as u8;
        }
    }
}

/// In-place box blur of a w×h coverage buffer (one horizontal + one vertical pass).
fn box_blur(buf: &mut [u8], w: usize, h: usize, r: usize) {
    if r == 0 {
        return;
    }
    let mut tmp = vec![0u8; buf.len()];
    let div = (2 * r + 1) as u32;
    for y in 0..h {
        let row = &buf[y * w..(y + 1) * w];
        let mut sum: u32 = row[..=r.min(w - 1)].iter().map(|&v| v as u32).sum();
        for x in 0..w {
            tmp[y * w + x] = (sum / div) as u8;
            if x + r + 1 < w {
                sum += row[x + r + 1] as u32;
            }
            if x >= r {
                sum -= row[x - r] as u32;
            }
        }
    }
    for x in 0..w {
        let mut sum: u32 = 0;
        for i in 0..=r.min(h - 1) {
            sum += tmp[i * w + x] as u32;
        }
        for y in 0..h {
            buf[y * w + x] = (sum / div) as u8;
            if y + r + 1 < h {
                sum += tmp[(y + r + 1) * w + x] as u32;
            }
            if y >= r {
                sum -= tmp[(y - r) * w + x] as u32;
            }
        }
    }
}

impl Ui {
    fn clip_rect(&self) -> Rect {
        self.clip.last().copied().unwrap_or(Rect::new(
            0.0,
            0.0,
            self.pm.width() as f32,
            self.pm.height() as f32,
        ))
    }

    /// Restrict drawing (and hit testing) to `r` until [`Ui::pop_clip`].
    pub fn push_clip(&mut self, r: Rect) {
        let d = self.device(r);
        let top = self.clip_rect().intersect(d);
        self.clip.push(top);
    }

    pub fn pop_clip(&mut self) {
        self.clip.pop();
    }

    /// Make the mask match the current clip. Returns whether a mask applies.
    fn sync_mask(&mut self) -> bool {
        let Some(&top) = self.clip.last() else {
            return false;
        };
        let (w, h) = (self.pm.width(), self.pm.height());
        if self
            .mask
            .as_ref()
            .is_none_or(|m| (m.width(), m.height()) != (w, h))
        {
            self.mask = Mask::new(w, h);
            self.mask_rect = None;
        }
        if self.mask_rect != Some(top)
            && let Some(m) = self.mask.as_mut()
        {
            let data = m.data_mut();
            data.fill(0);
            let x0 = (top.x.round() as usize).min(w as usize);
            let x1 = (top.right().round() as usize).min(w as usize);
            let y0 = (top.y.round() as usize).min(h as usize);
            let y1 = (top.bottom().round() as usize).min(h as usize);
            for y in y0..y1 {
                data[y * w as usize + x0..y * w as usize + x1].fill(255);
            }
            self.mask_rect = Some(top);
        }
        self.mask.is_some()
    }

    fn fill_path_dev(&mut self, path: &tiny_skia::Path, paint: &Paint) {
        let masked = self.sync_mask();
        let mask = if masked { self.mask.as_ref() } else { None };
        self.pm
            .fill_path(path, paint, FillRule::Winding, Transform::identity(), mask);
    }

    fn stroke_path_dev(&mut self, path: &tiny_skia::Path, paint: &Paint, width: f32) {
        let masked = self.sync_mask();
        let mask = if masked { self.mask.as_ref() } else { None };
        let stroke = Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        self.pm
            .stroke_path(path, paint, &stroke, Transform::identity(), mask);
    }

    pub(crate) fn paint_background(&mut self) {
        if self.backdrop {
            self.pm.fill(tiny_skia::Color::TRANSPARENT);
            let t = self.theme.glass_tint;
            self.pm.fill(t.to_skia());
        } else {
            let (a, b) = self.theme.solid_bg;
            let h = self.pm.height() as f32;
            if let Some(sh) = LinearGradient::new(
                tiny_skia::Point::from_xy(0.0, 0.0),
                tiny_skia::Point::from_xy(0.0, h.max(1.0)),
                vec![
                    GradientStop::new(0.0, a.to_skia()),
                    GradientStop::new(1.0, b.to_skia()),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) && let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, self.pm.width() as f32, h)
            {
                let p = Paint {
                    shader: sh,
                    ..Paint::default()
                };
                self.pm.fill_rect(r, &p, Transform::identity(), None);
            }
        }
    }

    pub fn fill_rect(&mut self, r: Rect, c: Rgba) {
        self.fill_rrect(r, 0.0, c);
    }

    pub fn fill_rrect(&mut self, r: Rect, rad: f32, c: Rgba) {
        if c.a == 0 {
            return;
        }
        if let Some(p) = rrect_path(self.device(r), rad * self.scale) {
            self.fill_path_dev(&p, &solid(c));
        }
    }

    /// A vertical two-color gradient in a rounded rect.
    pub fn gradient_rrect(&mut self, r: Rect, rad: f32, top: Rgba, bottom: Rgba) {
        let d = self.device(r);
        let Some(p) = rrect_path(d, rad * self.scale) else {
            return;
        };
        let Some(sh) = LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, d.y),
            tiny_skia::Point::from_xy(0.0, d.bottom().max(d.y + 1.0)),
            vec![
                GradientStop::new(0.0, top.to_skia()),
                GradientStop::new(1.0, bottom.to_skia()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let paint = Paint {
            shader: sh,
            anti_alias: true,
            ..Paint::default()
        };
        self.fill_path_dev(&p, &paint);
    }

    /// A hairline border inside `r` (1 device px, so it stays crisp at any scale).
    pub fn stroke_rrect(&mut self, r: Rect, rad: f32, c: Rgba) {
        self.stroke_rrect_w(r, rad, c, 1.0 / self.scale);
    }

    /// A border of `width` logical px, drawn inside `r`.
    pub fn stroke_rrect_w(&mut self, r: Rect, rad: f32, c: Rgba, width: f32) {
        if c.a == 0 {
            return;
        }
        let w = (width * self.scale).max(1.0);
        let d = self.device(r).inset(w / 2.0);
        if let Some(p) = rrect_path(d, (rad * self.scale - w / 2.0).max(0.0)) {
            self.stroke_path_dev(&p, &solid(c), w);
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, c: Rgba) {
        let s = self.scale;
        if let Some(p) = PathBuilder::from_circle(cx * s, cy * s, (r * s).max(0.1)) {
            self.fill_path_dev(&p, &solid(c));
        }
    }

    /// Stroke a path given in logical px (icons, checkmarks, spinners).
    pub fn stroke_logical(
        &mut self,
        build: impl FnOnce(&mut PathBuilder, f32),
        c: Rgba,
        width: f32,
    ) {
        let mut pb = PathBuilder::new();
        build(&mut pb, self.scale);
        if let Some(p) = pb.finish() {
            self.stroke_path_dev(&p, &solid(c), (width * self.scale).max(1.0));
        }
    }

    /// Fill a path given in logical px.
    pub fn fill_logical(&mut self, build: impl FnOnce(&mut PathBuilder, f32), c: Rgba) {
        let mut pb = PathBuilder::new();
        build(&mut pb, self.scale);
        if let Some(p) = pb.finish() {
            self.fill_path_dev(&p, &solid(c));
        }
    }

    /// A soft drop shadow under the rounded rect `r`: Gaussian-like blur of `blur` px,
    /// moved down by `dy`.
    pub fn shadow(&mut self, r: Rect, rad: f32, blur: f32, dy: f32, c: Rgba) {
        if c.a == 0 || r.is_empty() {
            return;
        }
        let s = self.scale;
        let d = self.device(r);
        let (w, h) = (d.w.round().max(1.0) as u32, d.h.round().max(1.0) as u32);
        let b = (blur * s).round().max(1.0) as u32;
        let key = (w, h, (rad * s).round() as u32, b);
        if !self.shadows.contains_key(&key) {
            if self.shadows.len() > 48 {
                self.shadows.clear();
            }
            let pad = b * 2;
            let (mw, mh) = (w + 2 * pad, h + 2 * pad);
            let mut buf = vec![0u8; (mw * mh) as usize];
            if let (Some(mut m), Some(p)) = (
                Mask::new(mw, mh),
                rrect_path(
                    Rect::new(pad as f32, pad as f32, w as f32, h as f32),
                    rad * s,
                ),
            ) {
                m.fill_path(&p, FillRule::Winding, true, Transform::identity());
                buf.copy_from_slice(m.data());
            }
            // Three box passes approximate a Gaussian with sigma ≈ blur/2.
            let br = ((b as f32) / 2.0).ceil() as usize;
            for _ in 0..3 {
                box_blur(&mut buf, mw as usize, mh as usize, br);
            }
            self.shadows.insert(key, (mw, mh, buf));
        }
        let clip = self.clip_rect();
        if let Some((mw, mh, buf)) = self.shadows.get(&key) {
            let pad = (b * 2) as i32;
            blend_cov(
                &mut self.pm,
                d.x.round() as i32 - pad,
                (d.y + dy * s).round() as i32 - pad,
                *mw,
                *mh,
                buf,
                c,
                clip,
                None,
            );
        }
    }

    /// A glass panel: soft shadow, translucent fill, hairline border and a lit top edge.
    pub fn glass(&mut self, r: Rect, rad: f32, fill: Rgba, elevation: f32) {
        if elevation > 0.0 {
            let sh = self.theme.shadow;
            self.shadow(r, rad, elevation, elevation * 0.35, sh);
        }
        self.fill_rrect(r, rad, fill);
        let (stroke, hi) = (self.theme.panel_stroke, self.theme.panel_highlight);
        self.stroke_rrect(r, rad, stroke);
        // Lit top edge: a hairline that fades out toward the sides.
        let s = self.scale;
        let d = self.device(r);
        let y = d.y + 1.0;
        let x0 = d.x + rad * s * 0.6;
        let x1 = d.right() - rad * s * 0.6;
        if x1 > x0
            && let Some(sh) = LinearGradient::new(
                tiny_skia::Point::from_xy(x0, y),
                tiny_skia::Point::from_xy(x1, y),
                vec![
                    GradientStop::new(0.0, hi.fade(0.0).to_skia()),
                    GradientStop::new(0.5, hi.to_skia()),
                    GradientStop::new(1.0, hi.fade(0.0).to_skia()),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            )
            && let Some(rr) = tiny_skia::Rect::from_xywh(x0, d.y, x1 - x0, 1.0)
        {
            let p = Paint {
                shader: sh,
                ..Paint::default()
            };
            let masked = self.sync_mask();
            let mask = if masked { self.mask.as_ref() } else { None };
            self.pm.fill_rect(rr, &p, Transform::identity(), mask);
        }
    }

    /// Draw `img` into `r` with rounded corners. `cover` fills and crops; otherwise the
    /// whole image fits inside (letterboxed, centered).
    pub fn image(&mut self, r: Rect, img: &Pixmap, rad: f32, cover: bool) {
        let d = self.device(r);
        let (iw, ih) = (img.width() as f32, img.height() as f32);
        if iw < 1.0 || ih < 1.0 || d.is_empty() {
            return;
        }
        let k = if cover {
            (d.w / iw).max(d.h / ih)
        } else {
            (d.w / iw).min(d.h / ih)
        };
        let (w, h) = (iw * k, ih * k);
        let (tx, ty) = (d.x + (d.w - w) / 2.0, d.y + (d.h - h) / 2.0);
        let area = if cover { d } else { Rect::new(tx, ty, w, h) };
        let quality = if k < 0.5 {
            FilterQuality::Bicubic
        } else {
            FilterQuality::Bilinear
        };
        let paint = Paint {
            shader: Pattern::new(
                img.as_ref(),
                SpreadMode::Pad,
                quality,
                1.0,
                Transform::from_row(k, 0.0, 0.0, k, tx, ty),
            ),
            anti_alias: true,
            ..Paint::default()
        };
        if let Some(p) = rrect_path(area, rad * self.scale) {
            self.fill_path_dev(&p, &paint);
        }
    }

    /// Draw text with its baseline at (`x`, `baseline`). Returns the advance width.
    pub fn text(&mut self, x: f32, baseline: f32, s: &str, size: f32, w: Weight, c: Rgba) -> f32 {
        let sc = self.scale;
        let dsize = size * sc;
        let run = self.fonts.layout(s, dsize, w);
        let clip = self.clip_rect();
        let curve = lut(c.luma() > 0.6);
        let by = (baseline * sc).round() as i32;
        let ox = x * sc;
        for (face, id, gx) in run.glyphs {
            let px = ox + gx;
            let sub = ((px.fract().abs() * 4.0) as u8).min(3);
            let ix = px.floor() as i32;
            if let Some(g) = self.fonts.raster(face, id, dsize, sub)
                && g.w > 0
            {
                blend_cov(
                    &mut self.pm,
                    ix + g.left,
                    by + g.top,
                    g.w,
                    g.h,
                    &g.cov,
                    c,
                    clip,
                    Some(curve),
                );
            }
        }
        run.width / sc
    }

    /// Text vertically centered in `r`, aligned horizontally, ellipsized to fit.
    pub fn text_in(&mut self, r: Rect, s: &str, size: f32, w: Weight, c: Rgba, align: Align) {
        let fitted = self.fonts.ellipsize(s, size, w, r.w).into_owned();
        let width = self.fonts.measure(&fitted, size, w);
        let x = match align {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - width) / 2.0,
            Align::Right => r.right() - width,
        };
        let base = r.y + self.fonts.baseline(size, w, r.h);
        self.text(x, base, &fitted, size, w, c);
    }

    pub fn measure(&mut self, s: &str, size: f32, w: Weight) -> f32 {
        self.fonts.measure(s, size, w)
    }

    /// A one-pixel horizontal divider.
    pub fn hline(&mut self, x0: f32, x1: f32, y: f32, c: Rgba) {
        let s = self.scale;
        let h = 1.0 / s;
        self.fill_rect(Rect::new(x0, (y * s).round() / s, x1 - x0, h), c);
    }
}

/// A soft "wallpaper through glass" stand-in, for snapshots and previews where no OS
/// backdrop exists. Painted under the UI before compositing.
pub fn fake_backdrop(pm: &mut Pixmap, dark: bool) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    pm.fill(if dark {
        tiny_skia::Color::from_rgba8(0x22, 0x22, 0x28, 255)
    } else {
        tiny_skia::Color::from_rgba8(0xE9, 0xEC, 0xF2, 255)
    });
    let blobs: [(f32, f32, f32, u32); 3] = [
        (0.15, 0.2, 0.6, 0xFF9A57),
        (0.85, 0.35, 0.55, 0x3CC9C0),
        (0.55, 1.0, 0.7, 0x9094F7),
    ];
    for (fx, fy, fr, hex) in blobs {
        let c = Rgba::rgb(hex);
        let a = if dark { 70 } else { 90 };
        let center = tiny_skia::Point::from_xy(fx * w, fy * h);
        let rad = fr * w.max(h);
        if let Some(sh) = RadialGradient::new(
            center,
            0.0,
            center,
            rad,
            vec![
                GradientStop::new(0.0, Rgba { a, ..c }.to_skia()),
                GradientStop::new(1.0, Rgba { a: 0, ..c }.to_skia()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) && let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, w, h)
        {
            let p = Paint {
                shader: sh,
                ..Paint::default()
            };
            pm.fill_rect(r, &p, Transform::identity(), None);
        }
    }
}
