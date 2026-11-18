//! Draws the wheel (docs/02 §3) into a premultiplied RGBA pixmap.
//!
//! The overlays upload this pixmap to a composition swap chain (Windows) or a wl_shm /
//! XImage buffer (Linux); tests and `swirvo wheel --png` save it as PNG. Geometry comes
//! from `swirvo_wheel::geom`, so what you see is exactly what hit-testing uses.

pub mod icon;

use std::collections::HashMap;
use std::f32::consts::TAU;

pub use ab_glyph::FontArc;
use ab_glyph::{Font, GlyphId, PxScale, ScaleFont};
use swirvo_wheel::geom::{self, R, R0, RB};
pub use tiny_skia::Pixmap;
use tiny_skia::{
    Color, ColorU8, FillRule, Paint, PathBuilder, PremultipliedColorU8, Stroke, Transform,
};

#[derive(Debug, Clone)]
pub struct Wedge {
    pub label: String,
    pub caption: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

/// A sub-ring (presets) opened outside one wedge.
#[derive(Debug, Clone)]
pub struct Sub {
    pub parent: usize,
    pub items: Vec<String>,
    pub hover: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Scene {
    pub wedges: Vec<Wedge>,
    pub hover: Option<usize>,
    /// Index into Convert / Tools / Actions.
    pub wheel: usize,
    pub hub_title: String,
    pub hub_subtitle: String,
    pub theme: Theme,
    pub scale: f32,
    pub sub: Option<Sub>,
    /// Paint an almost-invisible (alpha 1/255) disc of this radius under everything, so a
    /// layered overlay keeps receiving drag events in the push-through band. 0 = off.
    pub hit_radius: f32,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            wedges: Vec::new(),
            hover: None,
            wheel: 0,
            hub_title: String::new(),
            hub_subtitle: String::new(),
            theme: Theme::Light,
            scale: 1.0,
            sub: None,
            hit_radius: 0.0,
        }
    }
}

struct Palette {
    glass: Color,
    wedge: Color,
    line: Color,
    hub: Color,
    ink: Color,
    ink2: Color,
    accents: [Color; 3],
    on_accent: [Color; 3],
}

fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// Token values from docs/02 §14.
fn palette(t: Theme) -> Palette {
    match t {
        Theme::Light => Palette {
            glass: rgba(249, 249, 251, 240),
            wedge: rgba(255, 255, 255, 150),
            line: rgba(20, 24, 33, 26),
            hub: rgba(255, 255, 255, 255),
            ink: rgba(26, 28, 33, 255),
            ink2: rgba(90, 96, 107, 255),
            accents: [
                rgba(0xE8, 0x77, 0x2E, 255),
                rgba(0x0E, 0x8A, 0x84, 255),
                rgba(0x5B, 0x5F, 0xD6, 255),
            ],
            on_accent: [
                rgba(26, 28, 33, 255),
                rgba(255, 255, 255, 255),
                rgba(255, 255, 255, 255),
            ],
        },
        Theme::Dark => Palette {
            glass: rgba(36, 38, 44, 240),
            wedge: rgba(255, 255, 255, 16),
            line: rgba(255, 255, 255, 24),
            hub: rgba(42, 46, 53, 255),
            ink: rgba(238, 240, 243, 255),
            ink2: rgba(163, 169, 180, 255),
            accents: [
                rgba(0xFF, 0x9A, 0x57, 255),
                rgba(0x3C, 0xC9, 0xC0, 255),
                rgba(0x90, 0x94, 0xF7, 255),
            ],
            on_accent: [
                rgba(26, 28, 33, 255),
                rgba(14, 21, 19, 255),
                rgba(17, 18, 42, 255),
            ],
        },
    }
}

const TABS: [&str; 3] = ["Convert", "Tools", "Actions"];

/// Canvas size in px at 100 %: wheel, shadow, tab pill and the sub-ring (R2) all fit.
pub const CANVAS_W: f32 = 2.0 * (geom::R2 + 12.0);
pub const CANVAS_H: f32 = CANVAS_W;
/// Hub center inside the canvas at 100 %.
pub const CENTER: (f32, f32) = (CANVAS_W / 2.0, CANVAS_H / 2.0);

fn point(cx: f32, cy: f32, r: f32, a: f32) -> (f32, f32) {
    (cx + a.sin() * r, cy - a.cos() * r)
}

/// Annular sector, arcs approximated by ~1° segments (anti-aliased by tiny-skia).
fn sector(cx: f32, cy: f32, r1: f32, r2: f32, a0: f32, a1: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let steps = (((a1 - a0) / TAU * 360.0).ceil() as usize).max(2);
    for k in 0..=steps {
        let a = a0 + (a1 - a0) * k as f32 / steps as f32;
        let (x, y) = point(cx, cy, r2, a);
        if k == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    for k in (0..=steps).rev() {
        let a = a0 + (a1 - a0) * k as f32 / steps as f32;
        let (x, y) = point(cx, cy, r1, a);
        pb.line_to(x, y);
    }
    pb.close();
    pb.finish()
}

fn circle(cx: f32, cy: f32, r: f32) -> Option<tiny_skia::Path> {
    PathBuilder::from_circle(cx, cy, r)
}

fn paint(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let k = 0.5523 * r;
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

/// A glyph's coverage, rasterized once per (glyph, size, quarter-pixel offset).
struct Mask {
    left: i32,
    top: i32,
    w: usize,
    cov: Vec<u8>,
}

/// (glyph id, px size bits, x and y offset in quarter pixels).
type GlyphKey = (u16, u32, u8, u8);

fn text_width(font: &FontArc, s: &str, px: f32) -> f32 {
    let f = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Source-over blend of `color` at coverage `cov` (0..=255) into a premultiplied pixel.
fn blend_px(d: PremultipliedColorU8, c: ColorU8, cov: u8) -> PremultipliedColorU8 {
    let a = cov as u32 * c.alpha() as u32 / 255;
    let inv = 255 - a;
    let mix = |s: u8, dv: u8| ((s as u32 * a + dv as u32 * inv + 127) / 255) as u8;
    let out_a = (a + (d.alpha() as u32 * inv + 127) / 255).min(255) as u8;
    let (r, g, b) = (
        mix(c.red(), d.red()),
        mix(c.green(), d.green()),
        mix(c.blue(), d.blue()),
    );
    PremultipliedColorU8::from_rgba(r.min(out_a), g.min(out_a), b.min(out_a), out_a).unwrap_or(d)
}

/// Glyph cache + text drawing.
struct Glyphs {
    font: FontArc,
    cache: HashMap<GlyphKey, Option<Mask>>,
}

impl Glyphs {
    fn mask(&mut self, id: GlyphId, px: f32, qx: u8, qy: u8) -> Option<&Mask> {
        let font = &self.font;
        self.cache
            .entry((id.0, px.to_bits(), qx, qy))
            .or_insert_with(|| {
                let pos = ab_glyph::point(qx as f32 / 4.0, qy as f32 / 4.0);
                let og = font.outline_glyph(id.with_scale_and_position(px, pos))?;
                let b = og.px_bounds();
                let (w, h) = (b.width() as usize, b.height() as usize);
                let mut cov = vec![0u8; w * h];
                og.draw(|x, y, c| {
                    if let Some(v) = cov.get_mut(y as usize * w + x as usize) {
                        *v = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                });
                Some(Mask {
                    left: b.min.x as i32,
                    top: b.min.y as i32,
                    w,
                    cov,
                })
            })
            .as_ref()
    }

    /// Draw `s` centered on `cx` with its baseline at `baseline` (device pixels).
    fn draw_centered(
        &mut self,
        pm: &mut Pixmap,
        s: &str,
        px: f32,
        cx: f32,
        baseline: f32,
        color: Color,
    ) {
        // An Arc clone (no allocation) so the scaled font doesn't borrow `self`.
        let font = self.font.clone();
        let scaled = font.as_scaled(PxScale::from(px));
        let mut x = cx - text_width(&font, s, px) / 2.0;
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        let c = color.to_color_u8();
        let (by, qy) = (baseline.floor(), ((baseline.fract() * 4.0) as u8).min(3));
        let mut prev = None;
        for ch in s.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            prev = Some(id);
            let (bx, qx) = (x.floor(), ((x.fract() * 4.0) as u8).min(3));
            x += scaled.h_advance(id);
            let Some(m) = self.mask(id, px, qx, qy) else {
                continue;
            };
            let pixels = pm.pixels_mut();
            let (ox, oy) = (bx as i32 + m.left, by as i32 + m.top);
            for (row, line) in m.cov.chunks_exact(m.w.max(1)).enumerate() {
                let y = oy + row as i32;
                if y < 0 || y >= h {
                    continue;
                }
                for (col, &cv) in line.iter().enumerate() {
                    let xx = ox + col as i32;
                    if cv == 0 || xx < 0 || xx >= w {
                        continue;
                    }
                    let i = (y * w + xx) as usize;
                    pixels[i] = blend_px(pixels[i], c, cv);
                }
            }
        }
    }
}

fn split_label(label: &str) -> Vec<&str> {
    if label.chars().count() <= 9 {
        return vec![label];
    }
    match label.find([' ', '-']) {
        Some(i) => {
            let (a, b) = label.split_at(i + 1);
            vec![a.trim_end(), b]
        }
        None => vec![label],
    }
}

fn hairline() -> Stroke {
    Stroke {
        width: 1.0,
        ..Stroke::default()
    }
}

/// What the cached background depends on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct BgKey {
    theme: Theme,
    wheel: usize,
    scale: f32,
    hit_radius: f32,
}

/// Incremental wheel renderer with two cached layers:
///
/// 1. **background**: hit floor, shadow, glass and tab pill. Keyed by theme, wheel, scale.
/// 2. **wheel**: background + every wedge unhovered + hub. Keyed by the wedge and hub text.
///
/// A hover change then costs one memcpy, restoring one wedge's area from the background
/// through a coverage mask, and drawing that wedge highlighted: well under a millisecond,
/// with no allocation once warm (PROJECT_GUIDE.md rule 2). Glyphs are rasterized once.
pub struct Renderer {
    glyphs: Option<Glyphs>,
    bg: Option<(BgKey, Pixmap)>,
    wheel: Option<(u64, Pixmap)>,
}

fn wheel_key(bg: &BgKey, scene: &Scene) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (
        bg.theme as u8,
        bg.wheel,
        bg.scale.to_bits(),
        bg.hit_radius.to_bits(),
    )
        .hash(&mut h);
    scene.hub_title.hash(&mut h);
    scene.hub_subtitle.hash(&mut h);
    scene.wedges.len().hash(&mut h);
    for w in &scene.wedges {
        w.label.hash(&mut h);
        w.caption.hash(&mut h);
    }
    h.finish()
}

impl Renderer {
    /// `font` = None draws shapes only (no system font available).
    pub fn new(font: Option<FontArc>) -> Renderer {
        Renderer {
            glyphs: font.map(|font| Glyphs {
                font,
                cache: HashMap::new(),
            }),
            bg: None,
            wheel: None,
        }
    }

    pub fn has_font(&self) -> bool {
        self.glyphs.is_some()
    }

    /// Canvas size in device pixels for `scale`.
    pub fn size(scale: f32) -> (u32, u32) {
        let s = scale.max(0.25);
        ((CANVAS_W * s).ceil() as u32, (CANVAS_H * s).ceil() as u32)
    }

    pub fn render(&mut self, scene: &Scene) -> Option<Pixmap> {
        let (w, h) = Self::size(scene.scale);
        let mut pm = Pixmap::new(w, h)?;
        self.render_into(scene, &mut pm).then_some(pm)
    }

    /// Paint `scene` into `out`, reallocating it only when the size changes.
    pub fn render_into(&mut self, scene: &Scene, out: &mut Pixmap) -> bool {
        let s = scene.scale.max(0.25);
        let (w, h) = Self::size(s);
        if (out.width(), out.height()) != (w, h) {
            match Pixmap::new(w, h) {
                Some(p) => *out = p,
                None => return false,
            }
        }
        let bkey = BgKey {
            theme: scene.theme,
            wheel: scene.wheel.min(2),
            scale: s,
            hit_radius: scene.hit_radius,
        };
        if self.bg.as_ref().is_none_or(|(k, _)| *k != bkey) {
            let Some(mut bg) = Pixmap::new(w, h) else {
                return false;
            };
            draw_background(&mut bg, bkey, self.glyphs.as_mut());
            self.bg = Some((bkey, bg));
            self.wheel = None;
        }
        let Some((_, bg)) = &self.bg else {
            return false;
        };
        let wkey = wheel_key(&bkey, scene);
        if self.wheel.as_ref().is_none_or(|(k, _)| *k != wkey) {
            let mut layer = bg.clone();
            draw_wheel(&mut layer, scene, None, s, self.glyphs.as_mut());
            self.wheel = Some((wkey, layer));
        }
        let Some((_, layer)) = &self.wheel else {
            return false;
        };
        out.data_mut().copy_from_slice(layer.data());

        let n = scene.wedges.len();
        if let Some(i) = scene.hover.filter(|&i| i < n) {
            // Put the background back in the resting wedge's cell, then draw it lifted.
            restore_cell(out, bg, i, n, s);
            draw_wedge(out, scene, i, true, s, self.glyphs.as_mut());
        }
        draw_sub(out, scene, s, self.glyphs.as_mut());
        true
    }
}

/// Copy `bg` over wedge `i`'s cell: the annulus between the hub gap and the rim, between
/// the midlines of the gaps to its neighbours. A hard edge on those midlines leaves the
/// neighbours' anti-aliased strokes intact. Only the cell's bounding box is visited, and
/// the inside test is two dot products per pixel (no trig).
fn restore_cell(out: &mut Pixmap, bg: &Pixmap, i: usize, n: usize, s: f32) {
    let (cx, cy) = (CENTER.0 * s, CENTER.1 * s);
    let (r_in, r_out) = ((RB - 3.0) * s, (R - 2.5) * s);
    let half = std::f32::consts::PI / n.max(1) as f32;
    let mid = geom::wedge_center(i, n);
    let (ux, uy) = (mid.sin(), -mid.cos()); // screen direction of the wedge's bisector
    let cos_half = half.cos();
    // Bounding box from points along the cell's outline.
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for k in 0..=16 {
        let a = mid - half + 2.0 * half * k as f32 / 16.0;
        for r in [r_in, r_out] {
            let (x, y) = (cx + a.sin() * r, cy - a.cos() * r);
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if n <= 2 {
        (x0, y0, x1, y1) = (cx - r_out, cy - r_out, cx + r_out, cy + r_out);
    }
    let w = out.width() as usize;
    let clampx = |v: f32| (v.max(0.0) as usize).min(w);
    let clampy = |v: f32| (v.max(0.0) as usize).min(out.height() as usize);
    let (xa, xb, ya, yb) = (
        clampx(x0 - 2.0),
        clampx(x1 + 3.0),
        clampy(y0 - 2.0),
        clampy(y1 + 3.0),
    );
    let (r_in2, r_out2) = (r_in * r_in, r_out * r_out);
    let src = bg.pixels();
    let dst = out.pixels_mut();
    for y in ya..yb {
        let dy = y as f32 + 0.5 - cy;
        for x in xa..xb {
            let dx = x as f32 + 0.5 - cx;
            let d2 = dx * dx + dy * dy;
            if d2 < r_in2 || d2 > r_out2 {
                continue;
            }
            if n > 1 && dx * ux + dy * uy < d2.sqrt() * cos_half {
                continue;
            }
            let k = y * w + x;
            dst[k] = src[k];
        }
    }
}

/// Wedge `i` of `n`, offset by (ox, oy) and grown by `grow` px on every side.
fn wedge_path(i: usize, n: usize, ox: f32, oy: f32, grow: f32) -> Option<tiny_skia::Path> {
    let (cx, cy) = CENTER;
    if n <= 1 {
        let mut pb = PathBuilder::new();
        pb.push_circle(cx + ox, cy + oy, R - 4.0 + grow);
        pb.push_circle(cx + ox, cy + oy, RB - grow);
        return pb.finish();
    }
    let (a0, a1) = geom::wedge_span(i, n);
    let da = grow / ((RB + R) / 2.0);
    sector(
        cx + ox,
        cy + oy,
        RB - grow,
        R - 4.0 + grow,
        a0 - da,
        a1 + da,
    )
}

fn draw_wedge(
    pm: &mut Pixmap,
    scene: &Scene,
    i: usize,
    hot: bool,
    s: f32,
    text: Option<&mut Glyphs>,
) {
    let Some(w) = scene.wedges.get(i) else { return };
    let n = scene.wedges.len();
    let pal = palette(scene.theme);
    let wheel = scene.wheel.min(2);
    let tf = Transform::from_scale(s, s);
    let (cx, cy) = CENTER;
    let mid = geom::wedge_center(i, n);
    let (ox, oy) = if hot {
        (mid.sin() * 4.0, -mid.cos() * 4.0)
    } else {
        (0.0, 0.0)
    };
    if let Some(p) = wedge_path(i, n, ox, oy, 0.0) {
        pm.fill_path(
            &p,
            &paint(if hot { pal.accents[wheel] } else { pal.wedge }),
            FillRule::EvenOdd,
            tf,
            None,
        );
        pm.stroke_path(&p, &paint(pal.line), &hairline(), tf, None);
    }
    let Some(t) = text else { return };
    let (lx, ly) = point(cx + ox, cy + oy, R * 0.665, mid);
    let (ink, ink2) = if hot {
        (pal.on_accent[wheel], pal.on_accent[wheel])
    } else {
        (pal.ink, pal.ink2)
    };
    let lines = split_label(&w.label);
    let chars = w.label.chars().count();
    let size = if lines.len() > 1 {
        12.5
    } else if chars <= 5 {
        15.0
    } else if chars <= 8 {
        13.5
    } else {
        12.0
    };
    let top = if lines.len() > 1 { ly - 9.0 } else { ly - 1.0 };
    for (k, l) in lines.iter().enumerate() {
        t.draw_centered(pm, l, size * s, lx * s, (top + 14.0 * k as f32) * s, ink);
    }
    let cap_y = top + 14.0 * lines.len() as f32;
    t.draw_centered(pm, &w.caption, 10.5 * s, lx * s, cap_y * s, ink2);
}

/// Every wedge (with `hover` lifted) and the hub, straight onto `pm`.
fn draw_wheel(
    pm: &mut Pixmap,
    scene: &Scene,
    hover: Option<usize>,
    s: f32,
    mut text: Option<&mut Glyphs>,
) {
    for i in 0..scene.wedges.len() {
        draw_wedge(pm, scene, i, hover == Some(i), s, text.as_deref_mut());
    }
    let pal = palette(scene.theme);
    let tf = Transform::from_scale(s, s);
    let (cx, cy) = CENTER;
    if let Some(p) = circle(cx, cy, R0) {
        pm.fill_path(&p, &paint(pal.hub), FillRule::Winding, tf, None);
        pm.stroke_path(&p, &paint(pal.line), &hairline(), tf, None);
    }
    if let Some(t) = text {
        t.draw_centered(
            pm,
            &scene.hub_title,
            11.5 * s,
            cx * s,
            (cy - 2.0) * s,
            pal.ink,
        );
        t.draw_centered(
            pm,
            &scene.hub_subtitle,
            10.0 * s,
            cx * s,
            (cy + 13.0) * s,
            pal.ink2,
        );
    }
}

/// The sub-ring, drawn last: it may overlap the pill when the parent wedge is at the top.
fn draw_sub(pm: &mut Pixmap, scene: &Scene, s: f32, mut text: Option<&mut Glyphs>) {
    let n = scene.wedges.len();
    let Some(sub) = scene
        .sub
        .as_ref()
        .filter(|s| s.parent < n && !s.items.is_empty())
    else {
        return;
    };
    let pal = palette(scene.theme);
    let wheel = scene.wheel.min(2);
    let tf = Transform::from_scale(s, s);
    let (cx, cy) = CENTER;
    let ring = geom::SubRing {
        parent: sub.parent,
        parent_count: n,
        items: sub.items.len(),
    };
    for (j, label) in sub.items.iter().enumerate() {
        let (a0, a1) = ring.span(j);
        let hot = sub.hover == Some(j);
        if let Some(p) = sector(cx, cy, geom::R1, geom::R2, a0, a1) {
            pm.fill_path(
                &p,
                &paint(if hot { pal.accents[wheel] } else { pal.glass }),
                FillRule::Winding,
                tf,
                None,
            );
            pm.stroke_path(&p, &paint(pal.line), &hairline(), tf, None);
        }
        if let Some(t) = text.as_deref_mut() {
            let (lx, ly) = point(cx, cy, (geom::R1 + geom::R2) / 2.0, (a0 + a1) / 2.0);
            let ink = if hot { pal.on_accent[wheel] } else { pal.ink };
            t.draw_centered(pm, label, 11.5 * s, lx * s, (ly + 4.0) * s, ink);
        }
    }
}

fn draw_background(pm: &mut Pixmap, k: BgKey, mut text: Option<&mut Glyphs>) {
    let (s, pal, wheel) = (k.scale, palette(k.theme), k.wheel);
    let accent = pal.accents[wheel];
    let tf = Transform::from_scale(s, s);
    let (cx, cy) = CENTER;

    if k.hit_radius > 0.0
        && let Some(p) = circle(cx, cy, k.hit_radius.min(CANVAS_W / 2.0))
    {
        let mut floor = paint(rgba(0, 0, 0, 1));
        floor.anti_alias = false;
        pm.fill_path(&p, &floor, FillRule::Winding, tf, None);
    }
    // Soft shadow: a few stacked translucent discs.
    for (grow, a) in [(9.0, 6u8), (6.0, 9), (3.0, 12)] {
        if let Some(p) = circle(cx, cy + 5.0, R + grow) {
            pm.fill_path(&p, &paint(rgba(0, 0, 0, a)), FillRule::Winding, tf, None);
        }
    }
    if let Some(p) = circle(cx, cy, R) {
        pm.fill_path(&p, &paint(pal.glass), FillRule::Winding, tf, None);
        pm.stroke_path(&p, &paint(pal.line), &hairline(), tf, None);
    }

    // Tab pill above the wheel (outside the wheel, so it never overlaps wedges).
    let pill_w = 222.0;
    let (px, py) = (cx - pill_w / 2.0, cy - R - 44.0);
    if let Some(p) = rounded_rect(px, py, pill_w, 28.0, 14.0) {
        pm.fill_path(&p, &paint(pal.glass), FillRule::Winding, tf, None);
        pm.stroke_path(&p, &paint(pal.line), &hairline(), tf, None);
    }
    let tab_w = (pill_w - 6.0) / 3.0;
    for (i, name) in TABS.iter().enumerate() {
        let x = px + 3.0 + tab_w * i as f32;
        if i == wheel
            && let Some(p) = rounded_rect(x, py + 3.0, tab_w, 22.0, 11.0)
        {
            pm.fill_path(&p, &paint(accent), FillRule::Winding, tf, None);
        }
        if let Some(t) = text.as_deref_mut() {
            let ink = if i == wheel {
                pal.on_accent[wheel]
            } else {
                pal.ink2
            };
            t.draw_centered(
                pm,
                name,
                12.0 * s,
                (x + tab_w / 2.0) * s,
                (py + 18.5) * s,
                ink,
            );
        }
    }
}

/// The app icon at `px` × `px` (the Swirvo mark, [`icon`]): tray, title bars, `.ico`
/// and Store assets all come from this. Sizes up to 24 px get the simplified mark.
pub fn app_icon(px: u32) -> Option<Pixmap> {
    let mut pm = Pixmap::new(px, px)?;
    let small = icon::small(px);
    let k = px as f32 / 256.0;
    let tf = Transform::from_scale(k, k);
    let path = |cmds: &[icon::Cmd]| {
        let mut pb = PathBuilder::new();
        for c in cmds {
            match *c {
                icon::Cmd::M(x, y) => pb.move_to(x, y),
                icon::Cmd::L(x, y) => pb.line_to(x, y),
                icon::Cmd::C(a, b, c, d, e, f) => pb.cubic_to(a, b, c, d, e, f),
                icon::Cmd::Z => pb.close(),
            }
        }
        pb.finish()
    };
    let (t, b) = (icon::TOP, icon::BOTTOM);
    let shader = tiny_skia::LinearGradient::new(
        tiny_skia::Point::from_xy(0.0, 0.0),
        tiny_skia::Point::from_xy(256.0, 256.0),
        vec![
            tiny_skia::GradientStop::new(0.0, rgba(t.0, t.1, t.2, 255)),
            tiny_skia::GradientStop::new(1.0, rgba(b.0, b.1, b.2, 255)),
        ],
        tiny_skia::SpreadMode::Pad,
        Transform::identity(),
    )?;
    let bg = Paint {
        shader,
        anti_alias: true,
        ..Paint::default()
    };
    pm.fill_path(
        &path(&icon::background(small))?,
        &bg,
        FillRule::Winding,
        tf,
        None,
    );
    let white = paint(rgba(255, 255, 255, 255));
    for shape in icon::mark(small) {
        if let Some(p) = path(&shape) {
            pm.fill_path(&p, &white, FillRule::Winding, tf, None);
        }
    }
    Some(pm)
}

/// One-shot render (CLI, tests). Overlays keep a [`Renderer`] instead.
pub fn render(scene: &Scene, font: Option<&FontArc>) -> Option<Pixmap> {
    Renderer::new(font.cloned()).render(scene)
}

/// The UI font: Segoe UI Variable / Segoe UI on Windows; common sans fonts on Linux.
/// `SWIRVO_FONT` overrides.
pub fn load_ui_font() -> Option<FontArc> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("SWIRVO_FONT") {
        candidates.push(p.into());
    }
    if let Some(win) = std::env::var_os("WINDIR") {
        let fonts = std::path::Path::new(&win).join("Fonts");
        candidates.extend(
            ["SegUIVar.ttf", "segoeui.ttf"]
                .iter()
                .map(|f| fonts.join(f)),
        );
    }
    candidates.extend(
        [
            "/usr/share/fonts/truetype/ubuntu/Ubuntu[wdth,wght].ttf",
            "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
            "/mnt/c/Windows/Fonts/segoeui.ttf",
        ]
        .iter()
        .map(Into::into),
    );
    candidates.into_iter().find_map(|p| {
        std::fs::read(p)
            .ok()
            .and_then(|b| FontArc::try_from_vec(b).ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene(n: usize) -> Scene {
        Scene {
            wedges: (0..n)
                .map(|i| Wedge {
                    label: format!("F{i}"),
                    caption: "cap".into(),
                })
                .collect(),
            hover: Some(0),
            wheel: 0,
            hub_title: "IMG_2041.png".into(),
            hub_subtitle: "1.8 MB".into(),
            ..Scene::default()
        }
    }

    #[test]
    fn hovered_wedge_uses_accent_and_hub_is_opaque() {
        let pm = render(&scene(8), None).unwrap();
        let (cx, cy) = CENTER;
        // A point inside wedge 0 (12 o'clock), mid-band.
        let px = pm.pixel(cx as u32, (cy - 100.0) as u32).unwrap();
        assert!(
            px.red() > 200 && px.green() < 160,
            "wedge 0 should be the accent orange, got {px:?}"
        );
        let hub = pm.pixel(cx as u32, cy as u32).unwrap();
        assert_eq!(hub.alpha(), 255);
        // Corners stay transparent.
        assert_eq!(pm.pixel(0, CANVAS_H as u32 - 1).unwrap().alpha(), 0);
    }

    #[test]
    fn single_wedge_and_scale() {
        let pm = render(
            &Scene {
                scale: 1.5,
                ..scene(1)
            },
            None,
        )
        .unwrap();
        assert_eq!(pm.width(), (CANVAS_W * 1.5).ceil() as u32);
    }

    /// Budget gate (docs/07): a full repaint must fit well inside one 60 Hz frame.
    /// Run in release: `cargo test --release -p swirvo-render -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn repaint_budget() {
        let s = Scene {
            scale: 1.5,
            ..scene(8)
        };
        let time = |font: Option<FontArc>| {
            let mut r = Renderer::new(font);
            let mut pm = r.render(&s).unwrap(); // cold: background + glyphs
            let n = 100;
            let t = std::time::Instant::now();
            for i in 0..n {
                assert!(r.render_into(
                    &Scene {
                        hover: Some(i % 8),
                        ..s.clone()
                    },
                    &mut pm
                ));
            }
            t.elapsed().as_secs_f64() * 1e3 / n as f64
        };
        let font = load_ui_font();
        let shapes = time(None);
        let full = time(font.clone());
        println!(
            "hover repaint at 150 %: {full:.2} ms ({shapes:.2} ms shapes only, font: {})",
            font.is_some()
        );
        assert!(full < 2.0, "repaint took {full:.2} ms");
    }

    #[test]
    fn layered_render_matches_direct_drawing() {
        let font = load_ui_font();
        let mut r = Renderer::new(font.clone());
        let mut pm = r.render(&scene(8)).unwrap();
        let mut glyphs = font.map(|font| Glyphs {
            font,
            cache: HashMap::new(),
        });
        for (hover, wheel) in [(Some(3), 1), (None, 1), (Some(0), 0), (Some(7), 2)] {
            let sc = Scene {
                hover,
                wheel,
                scale: 1.25,
                ..scene(8)
            };
            assert!(r.render_into(&sc, &mut pm));
            // Reference: background + every wedge drawn directly, no layer tricks.
            let key = BgKey {
                theme: sc.theme,
                wheel,
                scale: 1.25,
                hit_radius: 0.0,
            };
            let (w, h) = Renderer::size(1.25);
            let mut direct = Pixmap::new(w, h).unwrap();
            draw_background(&mut direct, key, glyphs.as_mut());
            draw_wheel(&mut direct, &sc, hover, 1.25, glyphs.as_mut());
            let off = pm
                .data()
                .iter()
                .zip(direct.data())
                .filter(|(a, b)| a.abs_diff(**b) > 24)
                .count();
            if off >= 64 {
                let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
                for (k, (a, b)) in pm.data().iter().zip(direct.data()).enumerate() {
                    if a.abs_diff(*b) > 24 {
                        let (x, y) = ((k / 4) as u32 % w, (k / 4) as u32 / w);
                        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                    }
                }
                panic!(
                    "{off} channels differ for hover {hover:?} in ({x0},{y0})-({x1},{y1}) of {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn app_icon_is_opaque_in_the_middle_and_clear_in_the_corners() {
        for px in [16, 20, 24, 32, 48, 256] {
            let pm = app_icon(px).unwrap();
            // Opaque between the arrows and the edge, clear in the very corner.
            assert_eq!(
                pm.pixel(px / 2, px - 1 - px / 16).unwrap().alpha(),
                255,
                "{px}"
            );
            assert_eq!(pm.pixel(0, px - 1).unwrap().alpha(), 0, "{px}");
            // White mark present.
            let white = pm
                .pixels()
                .iter()
                .filter(|p| p.alpha() == 255 && p.red() > 240 && p.green() > 240 && p.blue() > 240)
                .count();
            assert!(white as u32 > px * px / 20, "{px}: {white} white pixels");
        }
        assert!(icon::svg(false).contains("<linearGradient"));
    }

    #[test]
    fn hit_floor_and_sub_ring() {
        let (cx, cy) = CENTER;
        let probe = (cx as u32, (cy + geom::R + 20.0) as u32); // push band, below the wheel
        let plain = render(&scene(8), None).unwrap();
        assert_eq!(plain.pixel(probe.0, probe.1).unwrap().alpha(), 0);
        let floored = render(
            &Scene {
                hit_radius: geom::R + geom::PUSH_BAND,
                ..scene(8)
            },
            None,
        )
        .unwrap();
        assert!(floored.pixel(probe.0, probe.1).unwrap().alpha() >= 1);
        assert_eq!(floored.pixel(0, 0).unwrap().alpha(), 0);

        // Sub-ring outside wedge 4 (6 o'clock of 8): its middle item sits straight down.
        let sub = Sub {
            parent: 4,
            items: vec!["Best".into(), "Balanced".into(), "Smallest".into()],
            hover: Some(1),
        };
        let pm = render(
            &Scene {
                sub: Some(sub),
                ..scene(8)
            },
            None,
        )
        .unwrap();
        let mid = (geom::R1 + geom::R2) / 2.0;
        let px = pm.pixel(cx as u32, (cy + mid) as u32).unwrap();
        assert!(
            px.alpha() > 200 && px.red() > 200 && px.green() < 160,
            "hovered preset should be the accent orange, got {px:?}"
        );
    }
}

#[cfg(test)]
mod profile {
    use super::*;
    #[test]
    #[ignore]
    fn profile_parts() {
        let s = 1.5;
        let (w, h) = Renderer::size(s);
        let mut pm = Pixmap::new(w, h).unwrap();
        let bg = pm.clone();
        let scene = Scene {
            scale: s,
            wedges: (0..8)
                .map(|i| Wedge {
                    label: format!("F{i}"),
                    caption: "cap".into(),
                })
                .collect(),
            ..Scene::default()
        };
        let mut t = |name: &str, f: &mut dyn FnMut(&mut Pixmap)| {
            let n = 200;
            let st = std::time::Instant::now();
            for _ in 0..n {
                f(&mut pm);
            }
            println!(
                "{name}: {:.3} ms",
                st.elapsed().as_secs_f64() * 1e3 / n as f64
            );
        };
        t("copy layer", &mut |pm| {
            pm.data_mut()
                .copy_from_slice(std::hint::black_box(bg.data()))
        });
        t("restore cell", &mut |pm| restore_cell(pm, &bg, 3, 8, s));
        t("draw_wedge hot (no text)", &mut |pm| {
            draw_wedge(pm, &scene, 3, true, s, None)
        });
        t("draw_wheel (no text)", &mut |pm| {
            draw_wheel(pm, &scene, Some(3), s, None)
        });
    }
}
