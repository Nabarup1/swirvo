//! Text: a small font set (regular + semibold, plus lazily loaded fallbacks), kerning,
//! measuring, ellipsizing and wrapping, and a glyph cache keyed by size and ¼-px offset.
//!
//! No shaping engine: scripts that need shaping (Arabic joining, Indic conjuncts) render
//! as separate glyphs. File names and UI copy in Latin, Greek, Cyrillic and CJK are fine.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;

use ab_glyph::{Font, FontArc, FontVec, GlyphId, PxScale, ScaleFont, VariableFont, point};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Semibold,
}

pub(crate) struct Raster {
    pub left: i32,
    pub top: i32,
    pub w: u32,
    pub h: u32,
    pub cov: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    face: u8,
    glyph: u16,
    /// Size in ¼ px.
    size: u16,
    /// Horizontal sub-pixel offset in ¼ px.
    sub: u8,
}

struct Face {
    font: FontArc,
    /// Multiplier from em size to ab_glyph's PxScale (which is ascent − descent).
    em_to_px: f32,
}

impl Face {
    fn new(font: FontArc) -> Face {
        let upem = font.units_per_em().unwrap_or(1000.0);
        let em_to_px = font.height_unscaled() / upem;
        Face { font, em_to_px }
    }
    fn scale(&self, em: f32) -> PxScale {
        PxScale::from(em * self.em_to_px)
    }
}

/// A laid-out run: (face, glyph, x offset from the run start) in px of the layout size.
pub struct Run {
    pub glyphs: Vec<(u8, GlyphId, f32)>,
    pub width: f32,
}

pub struct Fonts {
    faces: Vec<Face>,
    /// Fallback fonts tried for missing glyphs: (path, collection index). Loaded on first miss.
    pending: Vec<(PathBuf, u32)>,
    cache: HashMap<Key, Raster>,
    cache_bytes: usize,
    /// Which face covers a char, so a missing glyph costs one lookup per char, not per frame.
    coverage: HashMap<(char, Weight), u8>,
}

const CACHE_BUDGET: usize = 8 << 20;

fn font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(win) = std::env::var_os("WINDIR") {
        v.push(PathBuf::from(win).join("Fonts"));
    }
    // Developing in WSL: the Windows fonts make snapshots match the real app.
    v.push(PathBuf::from("/mnt/c/Windows/Fonts"));
    v
}

fn variable(bytes: &[u8], wght: f32) -> Option<FontArc> {
    let mut f = FontVec::try_from_vec(bytes.to_vec()).ok()?;
    f.set_variation(b"wght", wght);
    f.set_variation(b"opsz", 10.5);
    Some(FontArc::new(f))
}

fn load(path: &std::path::Path, index: u32) -> Option<FontArc> {
    let bytes = std::fs::read(path).ok()?;
    FontVec::try_from_vec_and_index(bytes, index)
        .ok()
        .map(FontArc::new)
}

impl Fonts {
    /// The system UI font: Segoe UI Variable (Windows 11), Segoe UI (10), else common Linux
    /// sans fonts. `SWIRVO_FONT` overrides the regular face.
    pub fn system() -> Option<Fonts> {
        let dirs = font_dirs();
        let mut pair: Option<(FontArc, FontArc)> = None;
        if let Some(p) = std::env::var_os("SWIRVO_FONT")
            && let Some(f) = load(std::path::Path::new(&p), 0)
        {
            pair = Some((f.clone(), f));
        }
        for d in &dirs {
            if pair.is_some() {
                break;
            }
            if let Ok(bytes) = std::fs::read(d.join("SegUIVar.ttf")) {
                pair = variable(&bytes, 400.0).zip(variable(&bytes, 600.0));
            }
            if pair.is_none()
                && let Some(r) = load(&d.join("segoeui.ttf"), 0)
            {
                let sb = load(&d.join("seguisb.ttf"), 0)
                    .or_else(|| load(&d.join("segoeuib.ttf"), 0))
                    .unwrap_or_else(|| r.clone());
                pair = Some((r, sb));
            }
        }
        if pair.is_none() {
            for (r, b) in [
                (
                    "/usr/share/fonts/truetype/ubuntu/Ubuntu-R.ttf",
                    "/usr/share/fonts/truetype/ubuntu/Ubuntu-M.ttf",
                ),
                (
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
                    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
                ),
                (
                    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
                    "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
                ),
            ] {
                if let Some(rf) = load(std::path::Path::new(r), 0) {
                    let bf = load(std::path::Path::new(b), 0).unwrap_or_else(|| rf.clone());
                    pair = Some((rf, bf));
                    break;
                }
            }
        }
        let (regular, semibold) = pair?;
        let mut pending = Vec::new();
        for d in &dirs {
            for (f, i) in [
                ("seguisym.ttf", 0),
                ("msyh.ttc", 0),
                ("Nirmala.ttc", 0),
                ("YuGothR.ttc", 0),
                ("malgun.ttf", 0),
                ("seguihis.ttf", 0),
            ] {
                let p = d.join(f);
                if p.exists() {
                    pending.push((p, i));
                }
            }
        }
        for p in [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ] {
            let p = PathBuf::from(p);
            if p.exists() {
                pending.push((p, 0));
            }
        }
        Some(Fonts::from_faces(regular, semibold, pending))
    }

    pub fn from_faces(
        regular: FontArc,
        semibold: FontArc,
        fallbacks: Vec<(PathBuf, u32)>,
    ) -> Fonts {
        Fonts {
            faces: vec![Face::new(regular), Face::new(semibold)],
            pending: fallbacks,
            cache: HashMap::new(),
            cache_bytes: 0,
            coverage: HashMap::new(),
        }
    }

    fn primary(w: Weight) -> u8 {
        match w {
            Weight::Regular => 0,
            Weight::Semibold => 1,
        }
    }

    /// The face that has `c`, loading fallbacks on demand. Falls back to the primary face
    /// (which draws .notdef) when nothing covers it.
    fn face_for(&mut self, c: char, w: Weight) -> u8 {
        let p = Self::primary(w);
        if self.faces[p as usize].font.glyph_id(c).0 != 0 || c.is_control() {
            return p;
        }
        if let Some(&f) = self.coverage.get(&(c, w)) {
            return f;
        }
        let mut found = None;
        for (i, f) in self.faces.iter().enumerate().skip(2) {
            if f.font.glyph_id(c).0 != 0 {
                found = Some(i as u8);
                break;
            }
        }
        while found.is_none() && !self.pending.is_empty() {
            let (path, idx) = self.pending.remove(0);
            if let Some(font) = load(&path, idx)
                && self.faces.len() < 250
            {
                let has = font.glyph_id(c).0 != 0;
                self.faces.push(Face::new(font));
                if has {
                    found = Some((self.faces.len() - 1) as u8);
                }
            }
        }
        let f = found.unwrap_or(p);
        self.coverage.insert((c, w), f);
        f
    }

    /// Baseline offset from the top of a line box of height `line_h`, for `size` px text.
    pub fn baseline(&self, size: f32, w: Weight, line_h: f32) -> f32 {
        let f = &self.faces[Self::primary(w) as usize];
        let s = f.font.as_scaled(f.scale(size));
        let (asc, desc) = (s.ascent(), s.descent());
        // Center the ascent+descent box in the line, which centers caps and x-height well
        // enough for UI fonts.
        (line_h - (asc - desc)) / 2.0 + asc
    }

    /// Lay out `text` at `size` px (em). Newlines are treated as spaces.
    pub fn layout(&mut self, text: &str, size: f32, w: Weight) -> Run {
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x = 0.0;
        let mut prev: Option<(u8, GlyphId)> = None;
        for c in text.chars() {
            let c = if c == '\n' || c == '\t' { ' ' } else { c };
            let fi = self.face_for(c, w);
            let face = &self.faces[fi as usize];
            let s = face.font.as_scaled(face.scale(size));
            let id = face.font.glyph_id(c);
            if let Some((pf, pid)) = prev
                && pf == fi
            {
                x += s.kern(pid, id);
            }
            glyphs.push((fi, id, x));
            x += s.h_advance(id);
            prev = Some((fi, id));
        }
        Run { glyphs, width: x }
    }

    pub fn measure(&mut self, text: &str, size: f32, w: Weight) -> f32 {
        self.layout(text, size, w).width
    }

    /// `text`, shortened with "…" in the middle-end so it fits `max_w`. Keeps the extension
    /// visible for file names ("Quarterly rep….pdf").
    pub fn ellipsize<'a>(
        &mut self,
        text: &'a str,
        size: f32,
        w: Weight,
        max_w: f32,
    ) -> Cow<'a, str> {
        if self.measure(text, size, w) <= max_w {
            return Cow::Borrowed(text);
        }
        let chars: Vec<char> = text.chars().collect();
        // Keep a short tail (the extension) when there is one.
        let tail_len = text
            .rfind('.')
            .map(|i| text[i..].chars().count())
            .filter(|&n| n <= 6 && n < chars.len() / 2)
            .unwrap_or(0);
        let tail: String = chars[chars.len() - tail_len..].iter().collect();
        let (mut lo, mut hi) = (0usize, chars.len() - tail_len);
        // Binary search the longest head that fits.
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            let cand: String = chars[..mid].iter().collect::<String>() + "…" + &tail;
            if self.measure(&cand, size, w) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let head: String = chars[..lo].iter().collect();
        Cow::Owned(format!("{}…{tail}", head.trim_end()))
    }

    /// Greedy word wrap into lines no wider than `max_w`; long words are broken by char.
    pub fn wrap(&mut self, text: &str, size: f32, w: Weight, max_w: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split(' ') {
                let cand = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if self.measure(&cand, size, w) <= max_w || line.is_empty() && word.is_empty() {
                    line = cand;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // A single word wider than the line: break it.
                let mut piece = String::new();
                for c in word.chars() {
                    piece.push(c);
                    if self.measure(&piece, size, w) > max_w && piece.chars().count() > 1 {
                        piece.pop();
                        lines.push(std::mem::take(&mut piece));
                        piece.push(c);
                    }
                }
                line = piece;
            }
            lines.push(line);
        }
        lines
    }

    /// Rasterized glyph (cached). `size` and `sub` are quantized to ¼ px.
    pub(crate) fn raster(&mut self, face: u8, id: GlyphId, size: f32, sub: u8) -> Option<&Raster> {
        let key = Key {
            face,
            glyph: id.0,
            size: (size * 4.0).round().clamp(1.0, 4000.0) as u16,
            sub,
        };
        if !self.cache.contains_key(&key) {
            if self.cache_bytes > CACHE_BUDGET {
                self.cache.clear();
                self.cache_bytes = 0;
            }
            let f = self.faces.get(face as usize)?;
            let g = id.with_scale_and_position(
                f.scale(key.size as f32 / 4.0),
                point(sub as f32 / 4.0, 0.0),
            );
            let r = match f.font.outline_glyph(g) {
                Some(o) => {
                    let b = o.px_bounds();
                    let (w, h) = (b.width().max(0.0) as u32, b.height().max(0.0) as u32);
                    let mut cov = vec![0u8; (w * h) as usize];
                    o.draw(|x, y, c| {
                        if x < w && y < h {
                            cov[(y * w + x) as usize] = (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                        }
                    });
                    Raster {
                        left: b.min.x as i32,
                        top: b.min.y as i32,
                        w,
                        h,
                        cov,
                    }
                }
                None => Raster {
                    left: 0,
                    top: 0,
                    w: 0,
                    h: 0,
                    cov: Vec::new(),
                },
            };
            self.cache_bytes += r.cov.len() + 32;
            self.cache.insert(key, r);
        }
        self.cache.get(&key)
    }
}
