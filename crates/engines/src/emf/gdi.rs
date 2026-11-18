//! GDI playback shared by EMF and WMF: the device context (pens, brushes, fonts, colours,
//! mapping mode, window and viewport, world transform, current position, clip), saved
//! states, path brackets, and the drawing primitives. Everything drawn becomes an
//! [`Item`] in picture coordinates (points, y down, origin at the picture's top-left).
//!
//! Coordinates go logical → (world transform) → page → (window/viewport mapping) →
//! device → picture. Paths and clips are converted when they are recorded, as GDI does.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

use crate::docxlayout::layout::{PathCmd, Pen};
use crate::docxlayout::props::Rgb;

use super::{Item, TextItem};

/// `[a b c d e f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
pub(crate) type M = [f32; 6];
pub(crate) const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Apply `first`, then `then`.
pub(crate) fn mul(first: M, then: M) -> M {
    let [a, b, c, d, e, f] = first;
    let [a2, b2, c2, d2, e2, f2] = then;
    [
        a * a2 + b * c2,
        a * b2 + b * d2,
        c * a2 + d * c2,
        c * b2 + d * d2,
        e * a2 + f * c2 + e2,
        e * b2 + f * d2 + f2,
    ]
}

pub(crate) fn apply(m: &M, x: f32, y: f32) -> (f32, f32) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

fn det(m: &M) -> f32 {
    m[0] * m[3] - m[1] * m[2]
}

/// How much a transform scales lengths (geometric mean of its axes).
fn scale_of(m: &M) -> f32 {
    det(m).abs().sqrt()
}

/// Work limits: a crafted file can't make playback run long or allocate without bound.
pub(crate) const MAX_ITEMS: usize = 400_000;
pub(crate) const MAX_POINTS: usize = 4_000_000;
const MAX_SAVED: usize = 256;
const MAX_OBJECTS: usize = 65_536;
const MAX_IMAGE_BYTES: usize = 256 << 20;

// Stock objects (index with the high bit set).
const STOCK: u32 = 0x8000_0000;

#[derive(Debug, Clone)]
pub(crate) struct GPen {
    pub style: u32,
    /// Width in logical units (0 = one device pixel).
    pub width: f32,
    pub color: Rgb,
    pub cosmetic: bool,
    /// PS_USERSTYLE dash lengths, in logical units.
    pub user: Vec<f32>,
}

#[derive(Debug, Clone)]
pub(crate) enum GBrush {
    Null,
    Solid(Rgb),
    Hatch(Rgb, u32),
}

#[derive(Debug, Clone)]
pub(crate) struct GFont {
    /// LOGFONT height: < 0 is the em height, > 0 the cell height, 0 the default.
    pub height: f32,
    /// Tenths of a degree, counter-clockwise.
    pub escapement: f32,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub charset: u8,
    pub face: String,
}

impl Default for GFont {
    fn default() -> GFont {
        GFont {
            height: 0.0,
            escapement: 0.0,
            weight: 400,
            italic: false,
            underline: false,
            strike: false,
            charset: 0,
            face: "Arial".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Obj {
    Pen(GPen),
    Brush(GBrush),
    Font(GFont),
    /// A WMF region: its bounding box in logical units.
    Region(Option<[f32; 4]>),
    Other,
}

#[derive(Debug)]
struct Clip {
    id: u32,
    path: Arc<Vec<PathCmd>>,
    even_odd: bool,
}

#[derive(Debug, Clone)]
struct Dc {
    pen: GPen,
    brush: GBrush,
    font: GFont,
    text_color: Rgb,
    bk_color: Rgb,
    bk_opaque: bool,
    winding: bool,
    text_align: u32,
    map_mode: u32,
    win_org: (f32, f32),
    win_ext: (f32, f32),
    vp_org: (f32, f32),
    vp_ext: (f32, f32),
    world: M,
    cur: (f32, f32),
    clockwise: bool,
    clip: Vec<Arc<Clip>>,
}

fn black_pen() -> GPen {
    GPen {
        style: 0,
        width: 0.0,
        color: Rgb(0, 0, 0),
        cosmetic: true,
        user: Vec::new(),
    }
}

/// What a primitive draws with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Paint {
    Stroke,
    Fill,
    Both,
}

pub(crate) struct Player {
    dc: Dc,
    saved: Vec<Dc>,
    pub objects: HashMap<u32, Obj>,
    pub items: Vec<Item>,
    emitted: Vec<u32>,
    next_clip: u32,
    /// Path bracket: figures in picture coordinates.
    path: Vec<PathCmd>,
    in_path: bool,
    fig_open: bool,
    /// Device → picture.
    dev2pic: M,
    /// Device units per millimetre (for the fixed mapping modes).
    dpm: (f32, f32),
    points: usize,
    image_bytes: usize,
    pub w: f32,
    pub h: f32,
}

impl Player {
    pub fn new(dev2pic: M, w: f32, h: f32, dpm: (f32, f32)) -> Player {
        Player {
            dc: Dc {
                pen: black_pen(),
                brush: GBrush::Solid(Rgb(255, 255, 255)),
                font: GFont::default(),
                text_color: Rgb(0, 0, 0),
                bk_color: Rgb(255, 255, 255),
                bk_opaque: true,
                winding: false,
                text_align: 0,
                map_mode: 1,
                win_org: (0.0, 0.0),
                win_ext: (1.0, 1.0),
                vp_org: (0.0, 0.0),
                vp_ext: (1.0, 1.0),
                world: ID,
                cur: (0.0, 0.0),
                clockwise: false,
                clip: Vec::new(),
            },
            saved: Vec::new(),
            objects: HashMap::new(),
            items: Vec::new(),
            emitted: Vec::new(),
            next_clip: 1,
            path: Vec::new(),
            in_path: false,
            fig_open: false,
            dev2pic,
            dpm: (dpm.0.max(1e-3), dpm.1.max(1e-3)),
            points: 0,
            image_bytes: 0,
            w,
            h,
        }
    }

    /// True once a work limit is reached; playback stops.
    pub fn full(&self) -> bool {
        self.items.len() >= MAX_ITEMS || self.points >= MAX_POINTS
    }

    /// The initial window and viewport (WMF: the window maps onto the picture's box).
    pub fn set_initial_window(&mut self, org: (f32, f32), ext: (f32, f32)) {
        self.dc.win_org = org;
        self.dc.win_ext = ext;
        self.dc.vp_org = org;
        self.dc.vp_ext = ext;
        self.dc.map_mode = 8;
    }

    // ------------------------------------------------------------------ transforms

    /// Page → device.
    fn mapping(&self) -> M {
        let d = &self.dc;
        let fixed = |mm: f32, flip: bool| -> (f32, f32) {
            let sx = mm * self.dpm.0;
            let sy = mm * self.dpm.1;
            (sx, if flip { -sy } else { sy })
        };
        let (sx, sy) = match d.map_mode {
            2 => fixed(0.1, true),
            3 => fixed(0.01, true),
            4 => fixed(0.254, true),
            5 => fixed(0.0254, true),
            6 => fixed(25.4 / 1440.0, true),
            7 | 8 => {
                let r = |v: f32, w: f32| if w.abs() < 1e-9 { 1.0 } else { v / w };
                let (mut sx, mut sy) = (r(d.vp_ext.0, d.win_ext.0), r(d.vp_ext.1, d.win_ext.1));
                if d.map_mode == 7 {
                    // Isotropic: the smaller scale for both axes, signs kept.
                    let m = sx.abs().min(sy.abs());
                    sx = m.copysign(sx);
                    sy = m.copysign(sy);
                }
                (sx, sy)
            }
            _ => (1.0, 1.0),
        };
        [
            sx,
            0.0,
            0.0,
            sy,
            d.vp_org.0 - d.win_org.0 * sx,
            d.vp_org.1 - d.win_org.1 * sy,
        ]
    }

    /// Logical → picture.
    pub fn t(&self) -> M {
        mul(mul(self.dc.world, self.mapping()), self.dev2pic)
    }

    fn sane(v: f32) -> f32 {
        if v.is_finite() { v } else { 0.0 }
    }

    pub fn set_map_mode(&mut self, m: u32) {
        if (1..=8).contains(&m) {
            self.dc.map_mode = m;
        }
    }
    pub fn set_window_org(&mut self, x: f32, y: f32) {
        self.dc.win_org = (Self::sane(x), Self::sane(y));
    }
    pub fn offset_window_org(&mut self, x: f32, y: f32) {
        self.dc.win_org.0 += Self::sane(x);
        self.dc.win_org.1 += Self::sane(y);
    }
    pub fn set_window_ext(&mut self, x: f32, y: f32) {
        if x != 0.0 && y != 0.0 {
            self.dc.win_ext = (Self::sane(x), Self::sane(y));
        }
    }
    pub fn set_viewport_org(&mut self, x: f32, y: f32) {
        self.dc.vp_org = (Self::sane(x), Self::sane(y));
    }
    pub fn offset_viewport_org(&mut self, x: f32, y: f32) {
        self.dc.vp_org.0 += Self::sane(x);
        self.dc.vp_org.1 += Self::sane(y);
    }
    pub fn set_viewport_ext(&mut self, x: f32, y: f32) {
        if x != 0.0 && y != 0.0 {
            self.dc.vp_ext = (Self::sane(x), Self::sane(y));
        }
    }
    pub fn scale_window_ext(&mut self, xn: f32, xd: f32, yn: f32, yd: f32) {
        if xd != 0.0 && yd != 0.0 {
            let (x, y) = self.dc.win_ext;
            self.set_window_ext(x * xn / xd, y * yn / yd);
        }
    }
    pub fn scale_viewport_ext(&mut self, xn: f32, xd: f32, yn: f32, yd: f32) {
        if xd != 0.0 && yd != 0.0 {
            let (x, y) = self.dc.vp_ext;
            self.set_viewport_ext(x * xn / xd, y * yn / yd);
        }
    }
    pub fn set_world(&mut self, m: M) {
        if m.iter().all(|v| v.is_finite()) && det(&m).abs() > 1e-12 {
            self.dc.world = m;
        }
    }
    /// MODIFYWORLDTRANSFORM: 1 identity, 2 left-multiply, 3 right-multiply, 4 set.
    pub fn modify_world(&mut self, m: M, mode: u32) {
        match mode {
            1 => self.dc.world = ID,
            2 => self.set_world(mul(m, self.dc.world)),
            3 => self.set_world(mul(self.dc.world, m)),
            4 => self.set_world(m),
            _ => {}
        }
    }

    pub fn save(&mut self) {
        if self.saved.len() < MAX_SAVED {
            self.saved.push(self.dc.clone());
        }
    }

    /// RESTOREDC: negative is relative to the current state, positive an absolute level.
    pub fn restore(&mut self, n: i32) {
        let target = if n < 0 {
            self.saved.len() as i64 + n as i64
        } else {
            n as i64 - 1
        };
        if target < 0 || target as usize >= self.saved.len() {
            return;
        }
        self.saved.truncate(target as usize + 1);
        if let Some(dc) = self.saved.pop() {
            self.dc = dc;
        }
    }

    // ------------------------------------------------------------------ state

    pub fn set_text_color(&mut self, c: Rgb) {
        self.dc.text_color = c;
    }
    pub fn set_bk_color(&mut self, c: Rgb) {
        self.dc.bk_color = c;
    }
    pub fn set_bk_mode(&mut self, m: u32) {
        self.dc.bk_opaque = m == 2;
    }
    pub fn set_fill_mode(&mut self, m: u32) {
        self.dc.winding = m == 2;
    }
    pub fn set_text_align(&mut self, a: u32) {
        self.dc.text_align = a;
    }
    pub fn set_arc_direction(&mut self, d: u32) {
        self.dc.clockwise = d == 2;
    }
    pub fn move_to(&mut self, x: f32, y: f32) {
        self.dc.cur = (x, y);
        if self.in_path {
            let (px, py) = apply(&self.t(), x, y);
            self.path.push(PathCmd::Move(px, py));
            self.fig_open = true;
        }
    }

    pub fn create(&mut self, handle: u32, o: Obj) {
        if self.objects.len() < MAX_OBJECTS || self.objects.contains_key(&handle) {
            self.objects.insert(handle, o);
        }
    }
    pub fn delete(&mut self, handle: u32) {
        self.objects.remove(&handle);
    }

    pub fn select(&mut self, handle: u32) {
        if handle & STOCK != 0 {
            return self.stock(handle & !STOCK);
        }
        match self.objects.get(&handle) {
            Some(Obj::Pen(p)) => self.dc.pen = p.clone(),
            Some(Obj::Brush(b)) => self.dc.brush = b.clone(),
            Some(Obj::Font(f)) => self.dc.font = f.clone(),
            Some(Obj::Region(r)) => {
                let r = *r;
                self.reset_clip();
                if let Some([l, t, r, b]) = r {
                    self.intersect_clip(l, t, r, b);
                }
            }
            _ => {}
        }
    }

    fn stock(&mut self, n: u32) {
        let gray = |v| GBrush::Solid(Rgb(v, v, v));
        match n {
            0 => self.dc.brush = gray(255),
            1 => self.dc.brush = gray(0xC0),
            2 => self.dc.brush = gray(0x80),
            3 => self.dc.brush = gray(0x40),
            4 => self.dc.brush = gray(0),
            5 => self.dc.brush = GBrush::Null,
            6 => {
                self.dc.pen = GPen {
                    color: Rgb(255, 255, 255),
                    ..black_pen()
                }
            }
            7 => self.dc.pen = black_pen(),
            8 => {
                self.dc.pen = GPen {
                    style: 5,
                    ..black_pen()
                }
            }
            10 | 11 | 16 => {
                self.dc.font = GFont {
                    face: "Courier New".into(),
                    ..GFont::default()
                }
            }
            12..=14 | 17 => self.dc.font = GFont::default(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ clipping

    fn new_clip(&mut self, path: Vec<PathCmd>, even_odd: bool) -> Arc<Clip> {
        let id = self.next_clip;
        self.next_clip = self.next_clip.wrapping_add(1);
        Arc::new(Clip {
            id,
            path: Arc::new(path),
            even_odd,
        })
    }

    fn rect_path(&self, l: f32, t: f32, r: f32, b: f32) -> Vec<PathCmd> {
        let m = self.t();
        let p = |x, y| apply(&m, x, y);
        let (a, b2, c, d) = (p(l, t), p(r, t), p(r, b), p(l, b));
        vec![
            PathCmd::Move(a.0, a.1),
            PathCmd::Line(b2.0, b2.1),
            PathCmd::Line(c.0, c.1),
            PathCmd::Line(d.0, d.1),
            PathCmd::Close,
        ]
    }

    /// A rectangle far bigger than the picture, for "everything except" clips.
    fn everything(&self) -> Vec<PathCmd> {
        let (w, h) = (self.w.max(1.0) * 100.0, self.h.max(1.0) * 100.0);
        vec![
            PathCmd::Move(-w, -h),
            PathCmd::Line(w, -h),
            PathCmd::Line(w, h),
            PathCmd::Line(-w, h),
            PathCmd::Close,
        ]
    }

    pub fn reset_clip(&mut self) {
        self.dc.clip.clear();
    }

    pub fn intersect_clip(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let p = self.rect_path(l, t, r, b);
        let c = self.new_clip(p, false);
        self.dc.clip.push(c);
    }

    pub fn exclude_clip(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let mut p = self.everything();
        p.extend(self.rect_path(l, t, r, b));
        let c = self.new_clip(p, true);
        self.dc.clip.push(c);
    }

    /// Region rectangles in device units (EMR_EXTSELECTCLIPRGN): 1 AND, 4 DIFF, 5 COPY.
    pub fn clip_device_rects(&mut self, rects: &[[f32; 4]], mode: u32) {
        let mut p = Vec::new();
        for r in rects {
            let q = |x, y| apply(&self.dev2pic, x, y);
            let (a, b, c, d) = (q(r[0], r[1]), q(r[2], r[1]), q(r[2], r[3]), q(r[0], r[3]));
            p.extend([
                PathCmd::Move(a.0, a.1),
                PathCmd::Line(b.0, b.1),
                PathCmd::Line(c.0, c.1),
                PathCmd::Line(d.0, d.1),
                PathCmd::Close,
            ]);
        }
        self.apply_clip(p, false, mode);
    }

    fn apply_clip(&mut self, path: Vec<PathCmd>, even_odd: bool, mode: u32) {
        match mode {
            5 => {
                self.dc.clip.clear();
                if !path.is_empty() {
                    let c = self.new_clip(path, even_odd);
                    self.dc.clip.push(c);
                }
            }
            1 => {
                let c = self.new_clip(path, even_odd);
                self.dc.clip.push(c);
            }
            4 => {
                let mut p = self.everything();
                p.extend(path);
                let c = self.new_clip(p, true);
                self.dc.clip.push(c);
            }
            // OR and XOR would widen the clip; drawing a little more is the safe side.
            _ => {}
        }
    }

    /// Bring the emitted clip nesting in line with the current clip.
    fn sync_clip(&mut self) {
        let k = self
            .emitted
            .iter()
            .zip(&self.dc.clip)
            .take_while(|(a, b)| **a == b.id)
            .count();
        while self.emitted.len() > k {
            self.emitted.pop();
            self.items.push(Item::Unclip);
        }
        for c in self.dc.clip[k..].iter().cloned() {
            self.emitted.push(c.id);
            self.items.push(Item::Clip {
                path: c.path.clone(),
                even_odd: c.even_odd,
            });
        }
    }

    /// Close every emitted clip (end of playback).
    pub fn finish(&mut self) {
        for _ in 0..self.emitted.len() {
            self.items.push(Item::Unclip);
        }
        self.emitted.clear();
    }

    // ------------------------------------------------------------------ emitting

    fn px(&self) -> f32 {
        scale_of(&self.dev2pic).max(1e-4)
    }

    fn line_pen(&self) -> Option<Pen> {
        let p = &self.dc.pen;
        let style = p.style & 0xF;
        if style == 5 {
            return None;
        }
        let geometric = !p.cosmetic && p.width > 0.0;
        // GDI never draws thinner than one device pixel.
        let width = if geometric {
            (p.width * scale_of(&self.t())).max(self.px())
        } else {
            self.px()
        };
        let cap = p.style & 0xF00;
        let join = p.style & 0xF000;
        let dash = match style {
            1 => vec![3.0, 1.0],
            2 => vec![1.0, 1.0],
            3 => vec![3.0, 1.0, 1.0, 1.0],
            4 => vec![3.0, 1.0, 1.0, 1.0, 1.0, 1.0],
            7 if !p.user.is_empty() => {
                let s = scale_of(&self.t());
                p.user
                    .iter()
                    .take(16)
                    .map(|v| (v * s / width).max(0.1))
                    .collect()
            }
            _ => Vec::new(),
        };
        Some(Pen {
            color: p.color,
            alpha: 1.0,
            width,
            dash,
            round: geometric && cap == 0 && join == 0,
        })
    }

    fn push(&mut self, it: Item) {
        // Clips always go in, so every Clip keeps its Unclip even at the limit.
        if self.items.len() < MAX_ITEMS || matches!(it, Item::Clip { .. } | Item::Unclip) {
            self.items.push(it);
        }
    }

    fn stroke(&mut self, path: &Arc<Vec<PathCmd>>) {
        if let Some(pen) = self.line_pen() {
            self.sync_clip();
            self.push(Item::Stroke {
                path: path.clone(),
                pen,
            });
        }
    }

    fn fill(&mut self, path: &Arc<Vec<PathCmd>>) {
        let even_odd = !self.dc.winding;
        match self.dc.brush.clone() {
            GBrush::Null => {}
            GBrush::Solid(c) => {
                self.sync_clip();
                self.push(Item::Fill {
                    path: path.clone(),
                    color: c,
                    alpha: 1.0,
                    even_odd,
                });
            }
            GBrush::Hatch(c, style) => {
                self.sync_clip();
                if self.dc.bk_opaque {
                    let bk = self.dc.bk_color;
                    self.push(Item::Fill {
                        path: path.clone(),
                        color: bk,
                        alpha: 1.0,
                        even_odd,
                    });
                }
                let lines = self.hatch(path, style);
                if lines.is_empty() {
                    return;
                }
                self.push(Item::Clip {
                    path: path.clone(),
                    even_odd,
                });
                let width = self.px();
                self.push(Item::Stroke {
                    path: Arc::new(lines),
                    pen: Pen {
                        color: c,
                        alpha: 1.0,
                        width,
                        dash: Vec::new(),
                        round: false,
                    },
                });
                self.push(Item::Unclip);
            }
        }
    }

    /// Hatch lines over a path's bounding box, 8 device pixels apart.
    fn hatch(&self, path: &[PathCmd], style: u32) -> Vec<PathCmd> {
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for c in path {
            let pts: &[(f32, f32)] = &match *c {
                PathCmd::Move(x, y) | PathCmd::Line(x, y) => vec![(x, y)],
                PathCmd::Quad(a, b, x, y) => vec![(a, b), (x, y)],
                PathCmd::Cubic(a, b, c2, d, x, y) => vec![(a, b), (c2, d), (x, y)],
                PathCmd::Close => vec![],
            };
            for &(x, y) in pts {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
        if x0 > x1 {
            return Vec::new();
        }
        let s = self.px() * 8.0;
        let mut out = Vec::new();
        let (w, h) = (x1 - x0, y1 - y0);
        if w / s > 2000.0 || h / s > 2000.0 {
            return out;
        }
        let horiz = matches!(style, 0 | 4);
        let vert = matches!(style, 1 | 4);
        let fdiag = matches!(style, 2 | 5);
        let bdiag = matches!(style, 3 | 5);
        if horiz {
            let mut y = (y0 / s).floor() * s;
            while y <= y1 {
                out.extend([PathCmd::Move(x0, y), PathCmd::Line(x1, y)]);
                y += s;
            }
        }
        if vert {
            let mut x = (x0 / s).floor() * s;
            while x <= x1 {
                out.extend([PathCmd::Move(x, y0), PathCmd::Line(x, y1)]);
                x += s;
            }
        }
        let n = ((w + h) / s).ceil() as i32 + 1;
        for k in 0..n {
            let o = (x0 / s).floor() * s - h + k as f32 * s;
            if fdiag {
                // "\": down to the right.
                out.extend([PathCmd::Move(o, y0), PathCmd::Line(o + h, y1)]);
            }
            if bdiag {
                // "/": up to the right.
                out.extend([PathCmd::Move(o, y1), PathCmd::Line(o + h, y0)]);
            }
        }
        out
    }

    fn draw(&mut self, path: Vec<PathCmd>, paint: Paint) {
        if path.is_empty() {
            return;
        }
        self.points += path.len();
        let path = Arc::new(path);
        if matches!(paint, Paint::Fill | Paint::Both) {
            self.fill(&path);
        }
        if matches!(paint, Paint::Stroke | Paint::Both) {
            self.stroke(&path);
        }
    }

    /// A finished figure: into the path bracket, or drawn now.
    fn figure(&mut self, cmds: Vec<PathCmd>, paint: Paint) {
        if self.in_path {
            self.points += cmds.len();
            if self.points < MAX_POINTS {
                self.path.extend(cmds);
            }
            self.fig_open = false;
        } else {
            self.draw(cmds, paint);
        }
    }

    // ------------------------------------------------------------------ primitives

    pub fn poly(&mut self, pts: &[(f32, f32)], closed: bool, bezier: bool) {
        if pts.is_empty() || self.points + pts.len() > MAX_POINTS {
            return;
        }
        let m = self.t();
        let q: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| apply(&m, x, y)).collect();
        let mut c = Vec::with_capacity(q.len() + 1);
        c.push(PathCmd::Move(q[0].0, q[0].1));
        push_segments(&mut c, &q[1..], bezier);
        if closed {
            c.push(PathCmd::Close);
        }
        self.figure(c, if closed { Paint::Both } else { Paint::Stroke });
    }

    pub fn polypoly(&mut self, polys: &[Vec<(f32, f32)>], closed: bool) {
        let total: usize = polys.iter().map(Vec::len).sum();
        if total == 0 || self.points + total > MAX_POINTS {
            return;
        }
        let m = self.t();
        let mut c = Vec::with_capacity(total + polys.len());
        for p in polys.iter().filter(|p| !p.is_empty()) {
            let q: Vec<(f32, f32)> = p.iter().map(|&(x, y)| apply(&m, x, y)).collect();
            c.push(PathCmd::Move(q[0].0, q[0].1));
            push_segments(&mut c, &q[1..], false);
            if closed {
                c.push(PathCmd::Close);
            }
        }
        self.figure(c, if closed { Paint::Both } else { Paint::Stroke });
    }

    /// PolylineTo / PolyBezierTo / LineTo: from the current position.
    pub fn poly_to(&mut self, pts: &[(f32, f32)], bezier: bool) {
        let usable = if bezier { pts.len() / 3 * 3 } else { pts.len() };
        if usable == 0 || self.points + usable > MAX_POINTS {
            return;
        }
        let m = self.t();
        let start = apply(&m, self.dc.cur.0, self.dc.cur.1);
        let q: Vec<(f32, f32)> = pts[..usable]
            .iter()
            .map(|&(x, y)| apply(&m, x, y))
            .collect();
        self.dc.cur = pts[usable - 1];
        if self.in_path {
            if !self.fig_open {
                self.path.push(PathCmd::Move(start.0, start.1));
                self.fig_open = true;
            }
            self.points += usable;
            push_segments(&mut self.path, &q, bezier);
        } else {
            let mut c = vec![PathCmd::Move(start.0, start.1)];
            push_segments(&mut c, &q, bezier);
            self.draw(c, Paint::Stroke);
        }
    }

    pub fn line_to(&mut self, x: f32, y: f32) {
        self.poly_to(&[(x, y)], false);
    }

    /// POLYDRAW: points tagged PT_MOVETO (6), PT_LINETO (2) or PT_BEZIERTO (4), with
    /// PT_CLOSEFIGURE (1) on the last point of a figure.
    pub fn poly_draw(&mut self, pts: &[(f32, f32)], kinds: &[u8]) {
        let n = pts.len().min(kinds.len());
        if n == 0 || self.points + n > MAX_POINTS {
            return;
        }
        let m = self.t();
        let q = |p: (f32, f32)| apply(&m, p.0, p.1);
        let mut c = Vec::with_capacity(n + 1);
        let mut started = false;
        let start = q(self.dc.cur);
        let mut i = 0;
        let mut last = self.dc.cur;
        while i < n {
            let k = kinds[i];
            let close;
            match k & !1 {
                6 => {
                    let p = q(pts[i]);
                    c.push(PathCmd::Move(p.0, p.1));
                    started = true;
                    close = k & 1 != 0;
                    last = pts[i];
                    i += 1;
                }
                4 if i + 2 < n => {
                    if !started {
                        c.push(PathCmd::Move(start.0, start.1));
                        started = true;
                    }
                    let (a, b, e) = (q(pts[i]), q(pts[i + 1]), q(pts[i + 2]));
                    c.push(PathCmd::Cubic(a.0, a.1, b.0, b.1, e.0, e.1));
                    close = kinds[i + 2] & 1 != 0;
                    last = pts[i + 2];
                    i += 3;
                }
                _ => {
                    if !started {
                        c.push(PathCmd::Move(start.0, start.1));
                        started = true;
                    }
                    let p = q(pts[i]);
                    c.push(PathCmd::Line(p.0, p.1));
                    close = k & 1 != 0;
                    last = pts[i];
                    i += 1;
                }
            }
            if close {
                c.push(PathCmd::Close);
            }
        }
        self.dc.cur = last;
        if self.in_path {
            self.points += c.len();
            self.path.extend(c);
            self.fig_open = true;
        } else {
            self.draw(c, Paint::Stroke);
        }
    }

    pub fn rectangle(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let c = self.rect_path(l, t, r, b);
        self.figure(c, Paint::Both);
    }

    /// The corners of a rounded rectangle are quarter ellipses `w` × `h`.
    pub fn round_rect(&mut self, l: f32, t: f32, r: f32, b: f32, w: f32, h: f32) {
        let (l, r) = (l.min(r), l.max(r));
        let (t, b) = (t.min(b), t.max(b));
        let rx = (w.abs() / 2.0).min((r - l) / 2.0);
        let ry = (h.abs() / 2.0).min((b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return self.rectangle(l, t, r, b);
        }
        let corner = |cx: f32, cy: f32, start: f32| ArcSpec {
            cx,
            cy,
            rx,
            ry,
            start,
            sweep: FRAC_PI_2,
        };
        // Clockwise on screen (y down): top-right, bottom-right, bottom-left, top-left.
        let arcs = [
            corner(r - rx, t + ry, -FRAC_PI_2),
            corner(r - rx, b - ry, 0.0),
            corner(l + rx, b - ry, FRAC_PI_2),
            corner(l + rx, t + ry, PI),
        ];
        let m = self.t();
        let mut c = Vec::new();
        for (k, a) in arcs.iter().enumerate() {
            a.append(&mut c, &m, k == 0);
        }
        c.push(PathCmd::Close);
        self.figure(c, Paint::Both);
    }

    pub fn ellipse(&mut self, l: f32, t: f32, r: f32, b: f32) {
        let a = ArcSpec {
            cx: (l + r) / 2.0,
            cy: (t + b) / 2.0,
            rx: (r - l).abs() / 2.0,
            ry: (b - t).abs() / 2.0,
            start: 0.0,
            sweep: 2.0 * PI,
        };
        if a.rx <= 0.0 || a.ry <= 0.0 {
            return;
        }
        let mut c = Vec::new();
        a.append(&mut c, &self.t(), true);
        c.push(PathCmd::Close);
        self.figure(c, Paint::Both);
    }

    /// ARC, CHORD, PIE and ARCTO: the ellipse in the box, from the ray through `s` to the
    /// ray through `e`, in the current arc direction.
    pub fn arc(&mut self, kind: ArcKind, bx: [f32; 4], s: (f32, f32), e: (f32, f32)) {
        let (cx, cy) = ((bx[0] + bx[2]) / 2.0, (bx[1] + bx[3]) / 2.0);
        let (rx, ry) = ((bx[2] - bx[0]).abs() / 2.0, (bx[3] - bx[1]).abs() / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return;
        }
        let ang = |p: (f32, f32)| ((p.1 - cy) / ry).atan2((p.0 - cx) / rx);
        let (a0, a1) = (ang(s), ang(e));
        let m = self.t();
        // On a y-down screen, growing angles turn clockwise; a flipping transform reverses
        // what the reader sees.
        let clockwise = self.dc.clockwise != (det(&m) < 0.0);
        let tau = 2.0 * PI;
        let sweep = if clockwise {
            let d = (a1 - a0).rem_euclid(tau);
            if d < 1e-6 { tau } else { d }
        } else {
            let d = (a0 - a1).rem_euclid(tau);
            -(if d < 1e-6 { tau } else { d })
        };
        let a = ArcSpec {
            cx,
            cy,
            rx,
            ry,
            start: a0,
            sweep,
        };
        let mut c = Vec::new();
        match kind {
            ArcKind::Arc => {
                a.append(&mut c, &m, true);
                self.figure(c, Paint::Stroke);
            }
            ArcKind::Chord => {
                a.append(&mut c, &m, true);
                c.push(PathCmd::Close);
                self.figure(c, Paint::Both);
            }
            ArcKind::Pie => {
                let (px, py) = apply(&m, cx, cy);
                c.push(PathCmd::Move(px, py));
                a.append(&mut c, &m, false);
                c.push(PathCmd::Close);
                self.figure(c, Paint::Both);
            }
            ArcKind::ArcTo => self.arc_from_cur(&a),
        }
    }

    /// A line from the current position to the arc's start, then the arc; the current
    /// position ends at the arc's end.
    fn arc_from_cur(&mut self, a: &ArcSpec) {
        let p0 = a.point(a.start);
        self.poly_to(&[p0], false);
        let m = self.t();
        let mut c = Vec::new();
        a.append(&mut c, &m, false);
        self.points += c.len();
        if self.in_path {
            self.path.extend(c);
            self.fig_open = true;
        } else {
            let (sx, sy) = apply(&m, p0.0, p0.1);
            let mut d = vec![PathCmd::Move(sx, sy)];
            d.extend(c);
            self.draw(d, Paint::Stroke);
        }
        self.dc.cur = a.point(a.start + a.sweep);
    }

    /// ANGLEARC: a circular arc; degrees, counter-clockwise from the x axis.
    pub fn angle_arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, sweep: f32) {
        if !(r > 0.0 && start.is_finite() && sweep.is_finite()) {
            return;
        }
        let a = ArcSpec {
            cx,
            cy,
            rx: r,
            ry: r,
            start: -start.to_radians(),
            sweep: -sweep.to_radians().clamp(-4.0 * PI, 4.0 * PI),
        };
        self.arc_from_cur(&a);
    }

    pub fn set_pixel(&mut self, x: f32, y: f32, c: Rgb) {
        let p = self.rect_path(x, y, x + 1.0, y + 1.0);
        self.sync_clip();
        self.push(Item::Fill {
            path: Arc::new(p),
            color: c,
            alpha: 1.0,
            even_odd: false,
        });
    }

    // ------------------------------------------------------------------ paths

    pub fn begin_path(&mut self) {
        self.path.clear();
        self.in_path = true;
        self.fig_open = false;
    }
    pub fn end_path(&mut self) {
        self.in_path = false;
    }
    pub fn abort_path(&mut self) {
        self.path.clear();
        self.in_path = false;
    }
    pub fn close_figure(&mut self) {
        if self.in_path && self.fig_open {
            self.path.push(PathCmd::Close);
            self.fig_open = false;
        }
    }
    /// FILLPATH, STROKEPATH and STROKEANDFILLPATH.
    pub fn paint_path(&mut self, fill: bool, stroke: bool) {
        self.in_path = false;
        let mut p = std::mem::take(&mut self.path);
        if fill {
            // Filling closes open figures.
            close_open(&mut p);
        }
        let paint = match (fill, stroke) {
            (true, true) => Paint::Both,
            (true, false) => Paint::Fill,
            _ => Paint::Stroke,
        };
        self.draw(p, paint);
    }
    pub fn select_clip_path(&mut self, mode: u32) {
        self.in_path = false;
        let mut p = std::mem::take(&mut self.path);
        close_open(&mut p);
        let even_odd = !self.dc.winding;
        self.apply_clip(p, even_odd, mode);
    }

    // ------------------------------------------------------------------ fills

    /// Rectangles in device units, filled with `brush` or the current one (FILLRGN,
    /// PAINTRGN).
    pub fn fill_device_rects(&mut self, rects: &[[f32; 4]], brush: Option<GBrush>) {
        let saved = brush.map(|b| std::mem::replace(&mut self.dc.brush, b));
        let mut p = Vec::new();
        for r in rects {
            let q = |x, y| apply(&self.dev2pic, x, y);
            let (a, b, c, d) = (q(r[0], r[1]), q(r[2], r[1]), q(r[2], r[3]), q(r[0], r[3]));
            p.extend([
                PathCmd::Move(a.0, a.1),
                PathCmd::Line(b.0, b.1),
                PathCmd::Line(c.0, c.1),
                PathCmd::Line(d.0, d.1),
                PathCmd::Close,
            ]);
        }
        self.draw(p, Paint::Fill);
        if let Some(b) = saved {
            self.dc.brush = b;
        }
    }

    /// A blit without a source (PATCOPY, BLACKNESS, WHITENESS): fill the destination.
    pub fn pat_blt(&mut self, x: f32, y: f32, w: f32, h: f32, rop: u32) {
        let brush = match rop {
            0x0000_0042 => GBrush::Solid(Rgb(0, 0, 0)),
            0x00FF_0062 => GBrush::Solid(Rgb(255, 255, 255)),
            0x00F0_0021 => self.dc.brush.clone(),
            _ => return,
        };
        let saved = std::mem::replace(&mut self.dc.brush, brush);
        let p = self.rect_path(x, y, x + w, y + h);
        self.draw(p, Paint::Fill);
        self.dc.brush = saved;
    }

    /// A GRADIENTFILL rectangle, from `c0` to `c1` across (or, `vertical`, down) it.
    pub fn gradient_rect(
        &mut self,
        a: (f32, f32),
        b: (f32, f32),
        c0: Rgb,
        c1: Rgb,
        vertical: bool,
    ) {
        let path = self.rect_path(a.0, a.1, b.0, b.1);
        let m = self.t();
        let from = apply(&m, a.0, a.1);
        let to = if vertical {
            apply(&m, a.0, b.1)
        } else {
            apply(&m, b.0, a.1)
        };
        self.sync_clip();
        self.push(Item::Gradient {
            path: Arc::new(path),
            from,
            to,
            c0,
            c1,
        });
    }

    /// A GRADIENTFILL triangle, filled with the average of its corners.
    pub fn gradient_triangle(&mut self, p: [(f32, f32); 3], c: Rgb) {
        let m = self.t();
        let q: Vec<(f32, f32)> = p.iter().map(|&(x, y)| apply(&m, x, y)).collect();
        let path = vec![
            PathCmd::Move(q[0].0, q[0].1),
            PathCmd::Line(q[1].0, q[1].1),
            PathCmd::Line(q[2].0, q[2].1),
            PathCmd::Close,
        ];
        self.sync_clip();
        self.push(Item::Fill {
            path: Arc::new(path),
            color: c,
            alpha: 1.0,
            even_odd: false,
        });
    }

    // ------------------------------------------------------------------ text

    /// EXTTEXTOUT: `text` at reference point (x, y) with optional per-character advances
    /// (logical units), an opaque background box and a clip box.
    pub fn text(
        &mut self,
        x: f32,
        y: f32,
        text: String,
        dx: Vec<f32>,
        opaque: Option<[f32; 4]>,
        clip: Option<[f32; 4]>,
    ) {
        if self.in_path {
            return;
        }
        if let Some([l, t, r, b]) = opaque {
            let p = self.rect_path(l, t, r, b);
            let bk = self.dc.bk_color;
            self.sync_clip();
            self.push(Item::Fill {
                path: Arc::new(p),
                color: bk,
                alpha: 1.0,
                even_odd: false,
            });
        }
        // Drop NULs, keeping each remaining character's advance.
        let (text, dx): (String, Vec<f32>) = if dx.len() == text.chars().count() {
            text.chars().zip(dx).filter(|(c, _)| *c != '\0').unzip()
        } else {
            (text.chars().filter(|&c| c != '\0').collect(), Vec::new())
        };
        if text.trim().is_empty() {
            return;
        }
        let m = self.t();
        let mut lin = [m[0], m[1], m[2], m[3], 0.0, 0.0];
        if det(&lin) < 0.0 {
            // GDI keeps text upright when the mapping flips y.
            lin[2] = -lin[2];
            lin[3] = -lin[3];
        }
        let f = self.dc.font.clone();
        let a = (f.escapement / 10.0).to_radians();
        let rot = [a.cos(), -a.sin(), a.sin(), a.cos(), 0.0, 0.0];
        let mut tm = mul(rot, lin);
        let (ox, oy) = apply(&m, x, y);
        tm[4] = ox;
        tm[5] = oy;
        if !tm.iter().all(|v| v.is_finite()) || det(&tm).abs() < 1e-12 {
            return;
        }
        let (size, cell) = if f.height < 0.0 {
            (-f.height, false)
        } else if f.height > 0.0 {
            (f.height, true)
        } else {
            // The default font: about 12 device pixels.
            (12.0 * self.px() / scale_of(&m).max(1e-6), true)
        };
        let ta = self.dc.text_align;
        let halign = match ta & 6 {
            6 => 1,
            2 => 2,
            _ => 0,
        };
        let valign = match ta & 24 {
            24 => 1,
            8 => 2,
            _ => 0,
        };
        let dx = if dx.iter().all(|v| v.is_finite()) {
            dx
        } else {
            Vec::new()
        };
        self.sync_clip();
        if let Some([l, t, r, b]) = clip {
            let p = self.rect_path(l, t, r, b);
            self.push(Item::Clip {
                path: Arc::new(p),
                even_odd: false,
            });
        }
        self.push(Item::Text(Box::new(TextItem {
            m: tm,
            text,
            family: f.face.clone(),
            bold: f.weight >= 600,
            italic: f.italic,
            underline: f.underline,
            strike: f.strike,
            size: size.min(1e6),
            cell,
            color: self.dc.text_color,
            halign,
            valign,
            dx,
        })));
        if clip.is_some() {
            self.push(Item::Unclip);
        }
    }

    pub fn font_charset(&self) -> u8 {
        self.dc.font.charset
    }

    // ------------------------------------------------------------------ pictures

    /// Draw `img` into the destination rectangle (logical units; negative sizes mirror).
    pub fn blit(&mut self, x: f32, y: f32, w: f32, h: f32, img: image::RgbaImage) {
        if w == 0.0 || h == 0.0 || ![x, y, w, h].iter().all(|v| v.is_finite()) {
            return;
        }
        let bytes = img.as_raw().len();
        if self.image_bytes + bytes > MAX_IMAGE_BYTES {
            return;
        }
        self.image_bytes += bytes;
        let mut png = Vec::new();
        if image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .is_err()
        {
            return;
        }
        let m = mul([w, 0.0, 0.0, h, x, y], self.t());
        self.sync_clip();
        self.push(Item::Image {
            png: Arc::new(png),
            m,
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArcKind {
    Arc,
    Chord,
    Pie,
    ArcTo,
}

/// An elliptical arc in logical units; angles in radians, y down (growing = clockwise).
struct ArcSpec {
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
    start: f32,
    sweep: f32,
}

impl ArcSpec {
    fn point(&self, a: f32) -> (f32, f32) {
        (self.cx + self.rx * a.cos(), self.cy + self.ry * a.sin())
    }

    /// Cubic segments of at most 90°, transformed by `m` (affine, so exact).
    fn append(&self, out: &mut Vec<PathCmd>, m: &M, start_new: bool) {
        let n = ((self.sweep.abs() / FRAC_PI_2).ceil() as usize).clamp(1, 16);
        let d = self.sweep / n as f32;
        let k = 4.0 / 3.0 * (d / 4.0).tan();
        let p0 = self.point(self.start);
        let q0 = apply(m, p0.0, p0.1);
        if start_new {
            out.push(PathCmd::Move(q0.0, q0.1));
        } else {
            out.push(PathCmd::Line(q0.0, q0.1));
        }
        let mut a = self.start;
        for _ in 0..n {
            let b = a + d;
            let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
            let c1 = (
                self.cx + self.rx * (ca - k * sa),
                self.cy + self.ry * (sa + k * ca),
            );
            let c2 = (
                self.cx + self.rx * (cb + k * sb),
                self.cy + self.ry * (sb - k * cb),
            );
            let e = (self.cx + self.rx * cb, self.cy + self.ry * sb);
            let (c1, c2, e) = (
                apply(m, c1.0, c1.1),
                apply(m, c2.0, c2.1),
                apply(m, e.0, e.1),
            );
            out.push(PathCmd::Cubic(c1.0, c1.1, c2.0, c2.1, e.0, e.1));
            a = b;
        }
    }
}

fn push_segments(c: &mut Vec<PathCmd>, q: &[(f32, f32)], bezier: bool) {
    if bezier {
        for t in q.as_chunks::<3>().0 {
            c.push(PathCmd::Cubic(
                t[0].0, t[0].1, t[1].0, t[1].1, t[2].0, t[2].1,
            ));
        }
    } else {
        c.extend(q.iter().map(|&(x, y)| PathCmd::Line(x, y)));
    }
}

/// Close every figure that isn't closed (filling does it implicitly).
fn close_open(p: &mut Vec<PathCmd>) {
    let mut out = Vec::with_capacity(p.len() + 4);
    let mut open = false;
    for c in p.drain(..) {
        match c {
            PathCmd::Move(..) => {
                if open {
                    out.push(PathCmd::Close);
                }
                open = true;
            }
            PathCmd::Close => open = false,
            _ => {}
        }
        out.push(c);
    }
    if open {
        out.push(PathCmd::Close);
    }
    *p = out;
}
