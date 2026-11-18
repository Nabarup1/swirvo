//! Stage 1 (docs/13 §3): hayro's interpreter drives a collecting [`Device`]. Output is in
//! page points, y down from the top of the crop box, rotation applied.

use super::fonts::{FontBook, FontHints, describe};
use super::model::{BBox, BLACK, Media, Rgb};
use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, CacheKey, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageData,
    ImageDrawProps, InterpreterCache, InterpreterSettings, LumaData, Paint, SoftMask, TransformExt,
    interpret_page,
};
use hayro::hayro_syntax::object::{Array, Dict, Name, Stream, String as PdfString};
use hayro::hayro_syntax::page::{Page, Resources};
use hayro::hayro_syntax::{Filter, Pdf, PdfData};
use hayro::kurbo::{Affine, BezPath, PathEl, Point, Rect};
use std::collections::HashMap;
use std::sync::Arc;
use swirvo_core::{CancelToken, EngineError};

pub const MAX_PAGES: usize = 2000;
/// Vector marks kept per page; beyond this the rest are summarized as one art box.
const MAX_MARKS: usize = 20_000;
const MAX_IMAGES: usize = 400;
/// Pictures are decoded at up to this resolution.
const IMAGE_DPI: f64 = 220.0;

#[derive(Clone, Debug, PartialEq)]
pub struct RawGlyph {
    pub ch: char,
    pub x0: f32,
    pub x1: f32,
    pub base: f32,
    pub size: f32,
    pub font: u16,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub invisible: bool,
    pub stroked: bool,
    pub underline: Option<Rgb>,
    pub strike: bool,
    pub highlight: Option<Rgb>,
    pub link: Option<u32>,
}

impl RawGlyph {
    pub fn cx(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    /// A point safely inside the glyph's ink, for "which cell/box is this in".
    pub fn mid_y(&self) -> f32 {
        self.base - 0.3 * self.size
    }

    #[cfg(test)]
    pub fn test(ch: char, x0: f32, base: f32, size: f32) -> RawGlyph {
        RawGlyph {
            ch,
            x0,
            x1: x0 + 0.5 * size,
            base,
            size,
            font: 0,
            color: BLACK,
            bold: false,
            italic: false,
            invisible: false,
            stroked: false,
            underline: None,
            strike: false,
            highlight: None,
            link: None,
        }
    }
}

/// A vector mark, classified as it's drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    /// Filled axis-aligned rectangle.
    Box { bbox: BBox, fill: Rgb },
    /// Stroked axis-aligned rectangle.
    Frame { bbox: BBox, color: Rgb, width: f32 },
    /// Axis-aligned line. `at` is y for horizontal rules, x for vertical ones.
    Rule {
        horizontal: bool,
        at: f32,
        lo: f32,
        hi: f32,
        width: f32,
        color: Rgb,
    },
    /// Anything else: curves, polygons, gradients, rotated images.
    Art { bbox: BBox },
}

impl Mark {
    pub fn bbox(&self) -> BBox {
        match *self {
            Mark::Box { bbox, .. } | Mark::Frame { bbox, .. } | Mark::Art { bbox } => bbox,
            Mark::Rule {
                horizontal,
                at,
                lo,
                hi,
                width,
                ..
            } => {
                let h = width / 2.0;
                if horizontal {
                    BBox::new(lo, at - h, hi, at + h)
                } else {
                    BBox::new(at - h, lo, at + h, hi)
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RawImage {
    pub bbox: BBox,
    pub media: u32,
}

#[derive(Clone, Debug, Default)]
pub struct RawPage {
    pub w: f32,
    pub h: f32,
    pub glyphs: Vec<RawGlyph>,
    pub marks: Vec<Mark>,
    pub images: Vec<RawImage>,
    pub links: Vec<(BBox, u32)>,
}

#[derive(Default)]
pub struct Extracted {
    pub pages: Vec<RawPage>,
    pub fonts: FontBook,
    pub media: Vec<Media>,
    pub links: Vec<String>,
}

/// A simple (non-CID) font dictionary, for naming fonts whose program carries no metadata.
struct DictFont {
    hints: FontHints,
    first: u32,
    widths: Vec<f32>,
}

#[derive(Default)]
struct MediaStore {
    media: Vec<Media>,
    keys: HashMap<(u128, u8, [i32; 4]), u32>,
}

pub fn extract(bytes: &[u8], cancel: &CancelToken) -> Result<Extracted, EngineError> {
    let data: PdfData = Arc::new(bytes.to_vec()).into();
    let pdf = Pdf::new(data).map_err(|e| {
        EngineError::Decode(format!(
            "couldn't open this PDF ({e:?}); it may be damaged or password-protected"
        ))
    })?;
    let pages = pdf.pages();
    if pages.len() > MAX_PAGES {
        return Err(EngineError::Invalid(format!(
            "this PDF has {} pages; layout conversion handles up to {MAX_PAGES}",
            pages.len()
        )));
    }
    let cache = InterpreterCache::new();
    let settings = InterpreterSettings::default();
    let mut out = Extracted::default();
    let mut store = MediaStore::default();
    let mut embedded: HashMap<u128, u16> = HashMap::new();
    for page in pages.iter() {
        cancel.check()?;
        let (w, h) = page.render_dimensions();
        let tf = page.initial_transform(true).to_kurbo();
        let mut ctx = Context::new(
            tf,
            Rect::new(0.0, 0.0, f64::from(w), f64::from(h)),
            &cache,
            page.xref(),
            settings.clone(),
        );
        let mut dict_fonts = Vec::new();
        page_fonts(page.resources(), 0, &mut dict_fonts);
        let mut raw = RawPage {
            w,
            h,
            ..Default::default()
        };
        {
            let mut dev = Collector {
                page_box: BBox::new(0.0, 0.0, w, h),
                page: &mut raw,
                clip: Vec::new(),
                fonts: &mut out.fonts,
                embedded: &mut embedded,
                dict_fonts: &dict_fonts,
                media: &mut store,
                overflow: BBox::EMPTY,
            };
            interpret_page(page, &mut ctx, &mut dev);
            if !dev.overflow.is_void() {
                let bbox = dev.overflow;
                dev.page.marks.push(Mark::Art { bbox });
            }
        }
        raw.links = page_links(page, tf, &mut out.links);
        out.pages.push(raw);
    }
    out.media = store.media;
    Ok(out)
}

struct Collector<'s> {
    page_box: BBox,
    page: &'s mut RawPage,
    clip: Vec<BBox>,
    fonts: &'s mut FontBook,
    embedded: &'s mut HashMap<u128, u16>,
    dict_fonts: &'s [DictFont],
    media: &'s mut MediaStore,
    /// Union of marks past [`MAX_MARKS`].
    overflow: BBox,
}

fn to_bbox(r: Rect) -> BBox {
    BBox::new(r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32)
}

fn solid(p: &Paint<'_>) -> Option<(Rgb, u8)> {
    match p {
        Paint::Color(c) => {
            let [r, g, b, a] = c.to_rgba().to_rgba8();
            Some(([r, g, b], a))
        }
        Paint::Pattern(_) => None,
    }
}

struct Sub {
    pts: Vec<Point>,
    curved: bool,
}

/// Split a path into device-space subpaths.
fn subpaths(path: &BezPath, tf: Affine) -> Vec<Sub> {
    let mut out: Vec<Sub> = Vec::new();
    let mut cur = Sub {
        pts: Vec::new(),
        curved: false,
    };
    let flush = |cur: &mut Sub, out: &mut Vec<Sub>| {
        if !cur.pts.is_empty() {
            out.push(std::mem::replace(
                cur,
                Sub {
                    pts: Vec::new(),
                    curved: false,
                },
            ));
        }
    };
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => {
                flush(&mut cur, &mut out);
                cur.pts.push(tf * p);
            }
            PathEl::LineTo(p) => cur.pts.push(tf * p),
            PathEl::QuadTo(a, p) => {
                cur.curved = true;
                cur.pts.push(tf * a);
                cur.pts.push(tf * p);
            }
            PathEl::CurveTo(a, b, p) => {
                cur.curved = true;
                cur.pts.push(tf * a);
                cur.pts.push(tf * b);
                cur.pts.push(tf * p);
            }
            PathEl::ClosePath => {
                if let Some(&first) = cur.pts.first() {
                    cur.pts.push(first);
                }
                flush(&mut cur, &mut out);
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

fn pts_bbox(pts: &[Point]) -> BBox {
    pts.iter().fold(BBox::EMPTY, |b, p| {
        b.union(BBox::new(p.x as f32, p.y as f32, p.x as f32, p.y as f32))
    })
}

/// The rectangle a closed 4-corner axis-aligned subpath describes.
fn as_rect(pts: &[Point]) -> Option<BBox> {
    let mut v: Vec<Point> = Vec::with_capacity(5);
    for &p in pts {
        if v.last()
            .is_none_or(|q: &Point| (q.x - p.x).abs() > 0.01 || (q.y - p.y).abs() > 0.01)
        {
            v.push(p);
        }
    }
    if v.len() == 5 && (v[0].x - v[4].x).abs() < 0.01 && (v[0].y - v[4].y).abs() < 0.01 {
        v.pop();
    }
    if v.len() != 4 {
        return None;
    }
    let same = |a: f64, b: f64| (a - b).abs() < 0.05;
    let a = same(v[0].x, v[1].x)
        && same(v[1].y, v[2].y)
        && same(v[2].x, v[3].x)
        && same(v[3].y, v[0].y);
    let b = same(v[0].y, v[1].y)
        && same(v[1].x, v[2].x)
        && same(v[2].y, v[3].y)
        && same(v[3].x, v[0].x);
    (a || b).then(|| pts_bbox(&v))
}

impl Collector<'_> {
    fn clip(&self) -> BBox {
        self.clip.last().copied().unwrap_or(self.page_box)
    }

    fn push(&mut self, m: Mark) {
        if self.page.marks.len() >= MAX_MARKS {
            self.overflow = self.overflow.union(m.bbox());
        } else {
            self.page.marks.push(m);
        }
    }

    fn art(&mut self, bbox: BBox) {
        let b = bbox.intersect(self.clip());
        if !b.is_void() && b.w() + b.h() > 0.5 {
            self.push(Mark::Art { bbox: b });
        }
    }

    fn filled_rect(&mut self, b: BBox, color: Rgb) {
        let b = b.intersect(self.clip());
        if b.is_void() || (b.w() < 0.05 && b.h() < 0.05) {
            return;
        }
        // Thin filled rectangles are how most producers draw rules and table borders.
        if b.h() <= 2.0 && b.w() > 2.0 * b.h() {
            self.push(Mark::Rule {
                horizontal: true,
                at: b.cy(),
                lo: b.x0,
                hi: b.x1,
                width: b.h().max(0.25),
                color,
            });
        } else if b.w() <= 2.0 && b.h() > 2.0 * b.w() {
            self.push(Mark::Rule {
                horizontal: false,
                at: b.cx(),
                lo: b.y0,
                hi: b.y1,
                width: b.w().max(0.25),
                color,
            });
        } else {
            self.push(Mark::Box {
                bbox: b,
                fill: color,
            });
        }
    }

    fn stroke_segment(&mut self, a: Point, b: Point, width: f32, color: Rgb) {
        let c = self.clip();
        let (ax, ay, bx, by) = (a.x as f32, a.y as f32, b.x as f32, b.y as f32);
        if (ay - by).abs() < 0.1 {
            let (lo, hi) = (ax.min(bx).max(c.x0), ax.max(bx).min(c.x1));
            if hi > lo && ay >= c.y0 && ay <= c.y1 {
                self.push(Mark::Rule {
                    horizontal: true,
                    at: (ay + by) / 2.0,
                    lo,
                    hi,
                    width,
                    color,
                });
            }
        } else if (ax - bx).abs() < 0.1 {
            let (lo, hi) = (ay.min(by).max(c.y0), ay.max(by).min(c.y1));
            if hi > lo && ax >= c.x0 && ax <= c.x1 {
                self.push(Mark::Rule {
                    horizontal: false,
                    at: (ax + bx) / 2.0,
                    lo,
                    hi,
                    width,
                    color,
                });
            }
        } else {
            self.art(BBox::new(ax, ay, bx, by).inflate(width / 2.0));
        }
    }

    fn run_font(&mut self, run: &GlyphRun<'_, '_>) -> u16 {
        let Some(first) = run.glyphs().first() else {
            return 0;
        };
        let mut meta = None;
        if let Glyph::Outline(o) = &**first
            && let Some(fd) = o.font_data()
        {
            if let Some(&id) = self.embedded.get(&fd.cache_key) {
                return id;
            }
            meta = Some(fd);
        }
        let fingerprint = self.fingerprint(run);
        let hints = match (&meta, fingerprint) {
            (Some(fd), dict) if fd.postscript_name.is_some() => FontHints {
                ps_name: fd.postscript_name.clone(),
                weight: fd.weight,
                italic: fd.is_italic,
                serif: fd.is_serif,
                mono: fd.is_monospace,
                force_bold: dict.is_some_and(|d| d.force_bold),
            },
            (Some(fd), Some(d)) => FontHints {
                weight: fd.weight.or(d.weight),
                italic: fd.is_italic || d.italic,
                ..d.clone()
            },
            (None, Some(d)) => d.clone(),
            (Some(fd), None) => FontHints {
                weight: fd.weight,
                italic: fd.is_italic,
                serif: fd.is_serif,
                mono: fd.is_monospace,
                ..Default::default()
            },
            (None, None) => FontHints::default(),
        };
        let id = self.fonts.intern(describe(&hints));
        if let Some(fd) = meta {
            self.embedded.insert(fd.cache_key, id);
        }
        id
    }

    /// The page font whose `Widths` match this run's (code, advance) pairs.
    fn fingerprint(&self, run: &GlyphRun<'_, '_>) -> Option<&FontHints> {
        let pairs: Vec<(u32, f32)> = run
            .glyphs()
            .iter()
            .take(48)
            .map(|g| (g.char_code(), advance(g).unwrap_or(-1.0)))
            .collect();
        let mut best: Option<(&DictFont, usize)> = None;
        let mut no_widths = Vec::new();
        for f in self.dict_fonts {
            if f.widths.is_empty() {
                no_widths.push(f);
                continue;
            }
            let mut hits = 0;
            let mut ok = true;
            for &(code, adv) in &pairs {
                let Some(i) = code.checked_sub(f.first) else {
                    continue;
                };
                let Some(&w) = f.widths.get(i as usize) else {
                    continue;
                };
                if (w - adv).abs() <= 0.6 {
                    hits += 1;
                } else {
                    ok = false;
                    break;
                }
            }
            if ok && hits > 0 && best.is_none_or(|(_, h)| hits > h) {
                best = Some((f, hits));
            }
        }
        match best {
            Some((f, _)) => Some(&f.hints),
            None if no_widths.len() == 1 => Some(&no_widths[0].hints),
            None => None,
        }
    }
}

fn advance(g: &Glyph<'_>) -> Option<f32> {
    match g {
        Glyph::Outline(o) => o.advance_width(),
        Glyph::Type3(t) => t.advance_width(),
    }
}

impl<'a> Device<'a> for Collector<'_> {
    fn draw_path(&mut self, path: &BezPath, props: DrawProps<'a>, mode: &DrawMode) {
        let tf = props.transform;
        let (fill, stroke) = match mode {
            DrawMode::Fill(_) => (true, None),
            DrawMode::Stroke(s) => (false, Some(s)),
            DrawMode::FillAndStroke(_, s) => (true, Some(s)),
            DrawMode::Invisible => return,
        };
        let Some((color, alpha)) = solid(&props.paint) else {
            // Gradients and patterns: art.
            self.art(to_bbox(
                tf.transform_rect_bbox(hayro::kurbo::Shape::bounding_box(path)),
            ));
            return;
        };
        if alpha < 8 {
            return;
        }
        let scale = tf.determinant().abs().sqrt() as f32;
        for sub in subpaths(path, tf) {
            if sub.curved {
                self.art(pts_bbox(&sub.pts));
                continue;
            }
            let rect = as_rect(&sub.pts);
            if fill {
                match rect {
                    Some(b) => self.filled_rect(b, color),
                    None if sub.pts.len() >= 3 => self.art(pts_bbox(&sub.pts)),
                    None => {}
                }
            }
            if let Some(s) = stroke {
                let width = (s.line_width * scale).max(0.25);
                match rect {
                    Some(b) if b.w() > 2.0 && b.h() > 2.0 => {
                        let b = b.intersect(self.clip());
                        if !b.is_void() {
                            self.push(Mark::Frame {
                                bbox: b,
                                color,
                                width,
                            });
                        }
                    }
                    _ => {
                        for w in sub.pts.windows(2) {
                            self.stroke_segment(w[0], w[1], width, color);
                        }
                    }
                }
            }
        }
    }

    fn push_clip_path(&mut self, clip_path: &ClipPath) {
        let b = to_bbox(hayro::kurbo::Shape::bounding_box(&clip_path.path));
        let c = self.clip().intersect(b);
        self.clip.push(c);
    }

    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        let invisible = matches!(mode, DrawMode::Invisible);
        let stroked = matches!(mode, DrawMode::Stroke(_));
        let color = match solid(&props.paint) {
            Some((_, a)) if a < 8 && !invisible => return,
            Some((c, _)) => c,
            None => BLACK,
        };
        let font = self.run_font(run);
        let clip = self.clip();
        let glyphs = run.glyphs();
        for (k, g) in glyphs.iter().enumerate() {
            let m = props.transform * g.transform();
            let [a, b, c, d, e, f] = m.as_coeffs();
            let size = (c.hypot(d) * 1000.0) as f32;
            // Upright, horizontal, not mirrored. Rotated text is a non-goal for now (docs/13 §1).
            if !(0.5..=2000.0).contains(&size) || a <= 0.0 || d >= 0.0 || b.abs() > 0.035 * a {
                continue;
            }
            let (x0, base) = (e as f32, f as f32);
            if !clip.inflate(size).contains(x0, base) {
                continue;
            }
            let mut adv = advance(g).map(|w| (f64::from(w) * a) as f32).unwrap_or(0.0);
            if !(adv > 0.0 && adv < 3.0 * size) {
                // Fall back to the next glyph's origin, else half an em.
                adv = glyphs
                    .get(k + 1)
                    .map(|n| (props.transform * n.transform()).as_coeffs()[4] as f32 - x0)
                    .filter(|w| *w > 0.0 && *w < 2.0 * size)
                    .unwrap_or(0.5 * size);
            }
            let italic = c.abs() > 0.12 * d.abs();
            let chars: Vec<char> = match g.as_unicode() {
                Some(BfString::Char(ch)) => vec![ch],
                Some(BfString::String(s)) => s.chars().collect(),
                None => {
                    let code = g.char_code();
                    vec![
                        char::from_u32(code)
                            .filter(|c| (' '..='~').contains(c))
                            .unwrap_or('\u{FFFD}'),
                    ]
                }
            };
            let n = chars.iter().filter(|c| !c.is_control()).count().max(1) as f32;
            let w = adv / n;
            let mut i = 0.0;
            for ch in chars {
                if ch.is_control() {
                    continue;
                }
                let ch = match ch {
                    '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{3000}' => ' ',
                    '\u{00AD}' => '-',
                    c => c,
                };
                self.page.glyphs.push(RawGlyph {
                    ch,
                    x0: x0 + i * w,
                    x1: x0 + (i + 1.0) * w,
                    base,
                    size,
                    font,
                    color,
                    bold: false,
                    italic,
                    invisible,
                    stroked,
                    underline: None,
                    strike: false,
                    highlight: None,
                    link: None,
                });
                i += 1.0;
            }
        }
    }

    fn draw_image(&mut self, image: Image<'a, '_>, props: ImageDrawProps<'a>) {
        let (iw, ih) = (f64::from(image.width()), f64::from(image.height()));
        if iw < 1.0 || ih < 1.0 {
            return;
        }
        let tf = props.transform;
        let full = to_bbox(tf.transform_rect_bbox(Rect::new(0.0, 0.0, iw, ih)));
        let vis = full.intersect(self.clip());
        if vis.is_void() || vis.w() < 1.5 || vis.h() < 1.5 {
            return;
        }
        let [a, b, c, d, _, _] = tf.as_coeffs();
        let axis = b.abs() <= 1e-3 * a.abs().max(d.abs()) && c.abs() <= 1e-3 * a.abs().max(d.abs());
        if !axis || self.page.images.len() >= MAX_IMAGES {
            self.art(vis);
            return;
        }
        let (flip_h, flip_v) = (a < 0.0, d < 0.0);
        let crop = [
            (vis.x0 - full.x0) / full.w(),
            (vis.y0 - full.y0) / full.h(),
            (vis.x1 - full.x0) / full.w(),
            (vis.y1 - full.y0) / full.h(),
        ];
        let q = crop.map(|v| (v * 1000.0).round() as i32);
        let cropped = q != [0, 0, 1000, 1000];
        let key = (
            image.cache_key(),
            u8::from(flip_h) | u8::from(flip_v) << 1,
            q,
        );
        if let Some(&m) = self.media.keys.get(&key) {
            self.page.images.push(RawImage {
                bbox: vis,
                media: m,
            });
            return;
        }
        let mut encoded = None;
        if let Image::Raster(r) = &image
            && !flip_h
            && !flip_v
            && !cropped
        {
            encoded = passthrough_jpeg(r.stream()).map(|bytes| Media { bytes, ext: "jpeg" });
        }
        if encoded.is_none() {
            let target = Some((
                ((f64::from(full.w()) / 72.0 * IMAGE_DPI).ceil() as u32).max(1),
                ((f64::from(full.h()) / 72.0 * IMAGE_DPI).ceil() as u32).max(1),
            ));
            let mut rgba: Option<image::RgbaImage> = None;
            match &image {
                Image::Raster(r) => r.with_rgba(|data, alpha| rgba = to_rgba(data, alpha), target),
                Image::Stencil(s) => s.with_stencil(
                    |luma, paint| {
                        let color = solid(paint).map(|c| c.0).unwrap_or(BLACK);
                        rgba = stencil(luma, color);
                    },
                    target,
                ),
            }
            let Some(mut img) = rgba else { return };
            if flip_h {
                img = image::imageops::flip_horizontal(&img);
            }
            if flip_v {
                img = image::imageops::flip_vertical(&img);
            }
            if cropped {
                let (w, h) = (img.width() as f32, img.height() as f32);
                let x = (crop[0] * w).floor().clamp(0.0, w - 1.0) as u32;
                let y = (crop[1] * h).floor().clamp(0.0, h - 1.0) as u32;
                let cw = ((crop[2] - crop[0]) * w).ceil().max(1.0) as u32;
                let ch = ((crop[3] - crop[1]) * h).ceil().max(1.0) as u32;
                img = image::imageops::crop_imm(
                    &img,
                    x,
                    y,
                    cw.min(img.width() - x),
                    ch.min(img.height() - y),
                )
                .to_image();
            }
            encoded = encode(img);
        }
        let Some(m) = encoded else { return };
        let idx = self.media.media.len() as u32;
        self.media.media.push(m);
        self.media.keys.insert(key, idx);
        self.page.images.push(RawImage {
            bbox: vis,
            media: idx,
        });
    }

    fn pop_clip(&mut self) {
        self.clip.pop();
    }

    fn pop_transparency_group(&mut self) {}
}

/// A plain baseline/progressive RGB or gray JPEG can go into the DOCX untouched.
fn passthrough_jpeg(s: &Stream<'_>) -> Option<Vec<u8>> {
    if s.filters().as_slice() != [Filter::DctDecode] {
        return None;
    }
    let d = s.dict();
    if [&b"SMask"[..], b"Mask", b"Decode", b"ImageMask"]
        .iter()
        .any(|k| d.contains_key(k))
    {
        return None;
    }
    let cs = d.get::<Name<'_>>(b"ColorSpace")?;
    if !matches!(cs.as_str(), "DeviceRGB" | "DeviceGray") {
        return None;
    }
    let raw = s.raw_data();
    raw.starts_with(&[0xFF, 0xD8]).then(|| raw.to_vec())
}

fn to_rgba(data: ImageData, alpha: Option<LumaData>) -> Option<image::RgbaImage> {
    let (w, h, mut px) = match data {
        ImageData::Rgb(d) => {
            let mut v = Vec::with_capacity(d.data.len() / 3 * 4);
            for c in d.data.as_chunks::<3>().0 {
                v.extend_from_slice(&[c[0], c[1], c[2], 255]);
            }
            (d.width, d.height, v)
        }
        ImageData::Luma(d) => {
            let mut v = Vec::with_capacity(d.data.len() * 4);
            for &g in &d.data {
                v.extend_from_slice(&[g, g, g, 255]);
            }
            (d.width, d.height, v)
        }
    };
    if let Some(a) = alpha
        && a.width > 0
        && a.height > 0
    {
        for y in 0..h {
            let ay = (u64::from(y) * u64::from(a.height) / u64::from(h)) as usize;
            for x in 0..w {
                let ax = (u64::from(x) * u64::from(a.width) / u64::from(w)) as usize;
                let v = a
                    .data
                    .get(ay * a.width as usize + ax)
                    .copied()
                    .unwrap_or(255);
                if let Some(p) = px.get_mut((y as usize * w as usize + x as usize) * 4 + 3) {
                    *p = v;
                }
            }
        }
    }
    image::RgbaImage::from_raw(w, h, px)
}

fn stencil(m: LumaData, color: Rgb) -> Option<image::RgbaImage> {
    let mut v = Vec::with_capacity(m.data.len() * 4);
    for &a in &m.data {
        v.extend_from_slice(&[color[0], color[1], color[2], a]);
    }
    image::RgbaImage::from_raw(m.width, m.height, v)
}

/// PNG for anything with transparency or few pixels; JPEG for large opaque pictures.
pub fn encode(img: image::RgbaImage) -> Option<Media> {
    let opaque = img.pixels().all(|p| p.0[3] == 255);
    let mut bytes = Vec::new();
    if opaque && u64::from(img.width()) * u64::from(img.height()) >= 256 * 256 {
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 88)
            .encode_image(&rgb)
            .ok()?;
        Some(Media { bytes, ext: "jpeg" })
    } else {
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .ok()?;
        Some(Media { bytes, ext: "png" })
    }
}

fn page_fonts(res: &Resources<'_>, depth: u8, out: &mut Vec<DictFont>) {
    for name in res.fonts.keys() {
        if let Some(d) = res.fonts.get::<Dict<'_>>(&name)
            && let Some(f) = dict_font(&d)
        {
            out.push(f);
        }
    }
    if depth >= 3 {
        return;
    }
    for name in res.x_objects.keys() {
        let Some(s) = res.x_objects.get::<Stream<'_>>(&name) else {
            continue;
        };
        if s.dict()
            .get::<Name<'_>>(b"Subtype")
            .is_some_and(|n| n.as_str() == "Form")
            && let Some(rd) = s.dict().get::<Dict<'_>>(b"Resources")
        {
            page_fonts(&Resources::new(rd), depth + 1, out);
        }
    }
}

fn dict_font(d: &Dict<'_>) -> Option<DictFont> {
    let subtype = d.get::<Name<'_>>(b"Subtype")?;
    let subtype = subtype.as_str();
    if subtype == "Type0" {
        return None; // CID fonts are embedded programs with their own metadata
    }
    let mut hints = FontHints {
        ps_name: d
            .get::<Name<'_>>(b"BaseFont")
            .map(|n| n.as_str().to_string()),
        ..Default::default()
    };
    if let Some(fd) = d.get::<Dict<'_>>(b"FontDescriptor") {
        let flags = fd.get::<f64>(b"Flags").unwrap_or(0.0) as u32;
        hints.mono = flags & 1 != 0;
        hints.serif = flags & 2 != 0;
        hints.italic =
            flags & 64 != 0 || fd.get::<f32>(b"ItalicAngle").is_some_and(|a| a.abs() > 1.0);
        hints.force_bold = flags & (1 << 18) != 0;
        hints.weight = fd.get::<f32>(b"FontWeight").map(|w| w as u32);
        if hints.ps_name.is_none() {
            hints.ps_name = fd
                .get::<Name<'_>>(b"FontName")
                .map(|n| n.as_str().to_string());
        }
    }
    let first = d.get::<f32>(b"FirstChar").unwrap_or(0.0).max(0.0) as u32;
    let mut widths = d.get::<Vec<f32>>(b"Widths").unwrap_or_default();
    if subtype == "Type3"
        && let Some(m) = d.get::<Vec<f32>>(b"FontMatrix")
        && let Some(&sx) = m.first()
    {
        // Type 3 widths are in glyph space; hayro reports them per 1000 text units.
        for w in &mut widths {
            *w *= sx * 1000.0;
        }
    }
    Some(DictFont {
        hints,
        first,
        widths,
    })
}

fn page_links(page: &Page<'_>, tf: Affine, table: &mut Vec<String>) -> Vec<(BBox, u32)> {
    let mut out = Vec::new();
    let Some(annots) = page.raw().get::<Array<'_>>(b"Annots") else {
        return out;
    };
    for a in annots.iter::<Dict<'_>>() {
        if a.get::<Name<'_>>(b"Subtype")
            .is_none_or(|n| n.as_str() != "Link")
        {
            continue;
        }
        let Some(r) = a.get::<[f32; 4]>(b"Rect") else {
            continue;
        };
        let Some(act) = a.get::<Dict<'_>>(b"A") else {
            continue;
        };
        if act
            .get::<Name<'_>>(b"S")
            .is_none_or(|n| n.as_str() != "URI")
        {
            continue;
        }
        let Some(uri) = act.get::<PdfString<'_>>(b"URI") else {
            continue;
        };
        let uri = String::from_utf8_lossy(uri.as_bytes()).trim().to_string();
        // Only links a document can safely carry.
        if !["http://", "https://", "mailto:"]
            .iter()
            .any(|p| uri.to_ascii_lowercase().starts_with(p))
        {
            continue;
        }
        let rect = Rect::new(
            f64::from(r[0]),
            f64::from(r[1]),
            f64::from(r[2]),
            f64::from(r[3]),
        );
        let idx = match table.iter().position(|u| *u == uri) {
            Some(i) => i,
            None => {
                table.push(uri);
                table.len() - 1
            }
        } as u32;
        out.push((to_bbox(tf.transform_rect_bbox(rect)), idx));
    }
    out
}
