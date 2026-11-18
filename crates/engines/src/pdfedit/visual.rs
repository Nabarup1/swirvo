//! Pixel comparison of two renderings of a page, for drawings and scans that have no
//! text to diff (docs/17 §2, Compare).
//!
//! 1. Both pages are scaled to the same pixel size (the new page's).
//! 2. A small shift between them (a scan fed a little crooked, a drawing re-exported with
//!    a different margin) is found by trying offsets on half-size copies, then refined at
//!    full size, and undone.
//! 3. A pixel is changed only when no pixel within one pixel of it on the other side has
//!    a similar colour, so anti-aliasing and sub-pixel shifts don't count.
//! 4. Isolated specks are dropped, and what's left is grouped into clusters with a box
//!    around each.

use image::RgbaImage;

/// Colour distance (largest channel difference) that counts as different.
const TOLERANCE: i32 = 72;
/// A change darker by less than this isn't new ink (or ink taken away).
const INK: i32 = 16;
/// Largest shift tried, in full-size pixels.
const MAX_SHIFT: i32 = 8;
/// Clusters with fewer changed pixels than this are noise.
const MIN_PIXELS: usize = 8;

/// What changed on one page, in the new page's pixel grid.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageDiff {
    pub w: u32,
    pub h: u32,
    /// Ink in the new page that the old one doesn't have.
    pub added: Vec<bool>,
    /// Ink in the old page (shifted onto the new) that the new one doesn't have.
    pub removed: Vec<bool>,
    /// The shift that lines old up with new: old pixel (x + dx, y + dy) ↔ new (x, y).
    pub shift: (i32, i32),
    /// Boxes around clusters of changes: `[x0, y0, x1, y1]` in pixels, y down, exclusive.
    pub boxes: Vec<[u32; 4]>,
}

impl PageDiff {
    pub fn changed(&self) -> bool {
        !self.boxes.is_empty()
    }
}

fn luma(p: [u8; 3]) -> i32 {
    (299 * p[0] as i32 + 587 * p[1] as i32 + 114 * p[2] as i32) / 1000
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3)
        .map(|i| (a[i] as i32 - b[i] as i32).abs())
        .max()
        .unwrap_or(0)
}

/// RGB over white (renders are opaque, but be safe with alpha).
fn rgb_of(img: &RgbaImage) -> Vec<[u8; 3]> {
    img.pixels()
        .map(|p| {
            let a = p[3] as u32;
            let over = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            [over(p[0]), over(p[1]), over(p[2])]
        })
        .collect()
}

struct Grid {
    w: i32,
    h: i32,
    px: Vec<[u8; 3]>,
}

impl Grid {
    /// The pixel at (x, y), white outside.
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            [255; 3]
        } else {
            self.px[(y * self.w + x) as usize]
        }
    }

    fn half(&self) -> Grid {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut s = [0u32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = self.at(2 * x + dx, 2 * y + dy);
                    for c in 0..3 {
                        s[c] += p[c] as u32;
                    }
                }
                px.push(s.map(|v| (v / 4) as u8));
            }
        }
        Grid { w, h, px }
    }
}

/// A grid's luma, row by row (the shift search reads each pixel many times).
struct Luma {
    w: i32,
    h: i32,
    v: Vec<i16>,
}

impl Luma {
    fn of(g: &Grid) -> Luma {
        Luma {
            w: g.w,
            h: g.h,
            v: g.px.iter().map(|&p| luma(p) as i16).collect(),
        }
    }
}

/// Sum of luma differences between `b` and `a` shifted by (dx, dy), away from the edges
/// (|dx|, |dy| ≤ MAX_SHIFT, so every read is inside the page).
fn cost(a: &Luma, b: &Luma, dx: i32, dy: i32) -> u64 {
    let m = MAX_SHIFT;
    let mut s = 0u64;
    for y in m..(b.h - m).max(m) {
        let (ra, rb) = (((y + dy) * a.w) as usize, (y * b.w) as usize);
        for x in m..(b.w - m).max(m) {
            let d = a.v[ra + (x + dx) as usize] - b.v[rb + x as usize];
            s += d.unsigned_abs() as u64;
        }
    }
    s
}

/// The offset (dx, dy) that best lines `a` up with `b`: coarse on half-size copies, then
/// refined at full size. A shift is only taken when it clearly beats none.
fn best_shift(a: &Grid, b: &Grid) -> (i32, i32) {
    let (ha, hb) = (Luma::of(&a.half()), Luma::of(&b.half()));
    let (a, b) = (Luma::of(a), Luma::of(b));
    let r = MAX_SHIFT / 2;
    let mut best = ((0, 0), cost(&ha, &hb, 0, 0));
    for dy in -r..=r {
        for dx in -r..=r {
            let c = cost(&ha, &hb, dx, dy);
            if c < best.1 {
                best = ((dx, dy), c);
            }
        }
    }
    let (cx, cy) = (best.0.0 * 2, best.0.1 * 2);
    let none = cost(&a, &b, 0, 0);
    let mut fine = ((cx, cy), cost(&a, &b, cx, cy));
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (x, y) = (cx + dx, cy + dy);
            if x.abs() > MAX_SHIFT || y.abs() > MAX_SHIFT {
                continue;
            }
            let c = cost(&a, &b, x, y);
            if c < fine.1 {
                fine = ((x, y), c);
            }
        }
    }
    // Ten percent better, or it's noise and the pages simply differ.
    if fine.1 * 10 < none * 9 {
        fine.0
    } else {
        (0, 0)
    }
}

/// True when no pixel of `other` within one pixel of (x, y) (after `shift`) is close
/// to `c`.
fn unmatched(c: [u8; 3], other: &Grid, x: i32, y: i32) -> bool {
    for oy in -1..=1 {
        for ox in -1..=1 {
            if dist(c, other.at(x + ox, y + oy)) <= TOLERANCE {
                return false;
            }
        }
    }
    true
}

/// Compare `old` with `new` (the old page is scaled to the new page's size first).
pub(crate) fn diff(old: &RgbaImage, new: &RgbaImage) -> PageDiff {
    let (w, h) = new.dimensions();
    let old = if old.dimensions() == (w, h) {
        old.clone()
    } else {
        image::imageops::resize(old, w, h, image::imageops::FilterType::Triangle)
    };
    let a = Grid {
        w: w as i32,
        h: h as i32,
        px: rgb_of(&old),
    };
    let b = Grid {
        w: w as i32,
        h: h as i32,
        px: rgb_of(new),
    };
    let (dx, dy) = best_shift(&a, &b);
    // Old lined up with new: pixel (x, y) of `s` is old (x + dx, y + dy).
    let mut px = Vec::with_capacity(b.px.len());
    for y in 0..b.h {
        for x in 0..b.w {
            px.push(a.at(x + dx, y + dy));
        }
    }
    let s = Grid { w: b.w, h: b.h, px };
    let n = (w * h) as usize;
    let mut added = vec![false; n];
    let mut removed = vec![false; n];
    for y in 0..b.h {
        for x in 0..b.w {
            let i = (y * b.w + x) as usize;
            let (cn, co) = (b.px[i], s.px[i]);
            if dist(cn, co) <= TOLERANCE {
                continue;
            }
            if luma(cn) <= luma(co) + INK && unmatched(cn, &s, x, y) {
                added[i] = true;
            }
            if luma(co) <= luma(cn) + INK && unmatched(co, &b, x, y) {
                removed[i] = true;
            }
        }
    }
    let mut d = PageDiff {
        w,
        h,
        added,
        removed,
        shift: (dx, dy),
        boxes: Vec::new(),
    };
    despeckle(&mut d);
    cluster(&mut d);
    d
}

/// Drop changed pixels with fewer than two changed neighbours.
fn despeckle(d: &mut PageDiff) {
    let (w, h) = (d.w as i32, d.h as i32);
    let any: Vec<bool> = d
        .added
        .iter()
        .zip(&d.removed)
        .map(|(a, r)| *a || *r)
        .collect();
    let on = |x: i32, y: i32| x >= 0 && y >= 0 && x < w && y < h && any[(y * w + x) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !any[i] {
                continue;
            }
            let mut k = 0;
            for oy in -1..=1 {
                for ox in -1..=1 {
                    if (ox, oy) != (0, 0) && on(x + ox, y + oy) {
                        k += 1;
                    }
                }
            }
            if k < 2 {
                d.added[i] = false;
                d.removed[i] = false;
            }
        }
    }
}

/// Group changes that are close together (within about 1 % of the page width) and box
/// them; tiny groups are noise and are cleared.
fn cluster(d: &mut PageDiff) {
    let (w, h) = (d.w as usize, d.h as usize);
    let any: Vec<bool> = d
        .added
        .iter()
        .zip(&d.removed)
        .map(|(a, r)| *a || *r)
        .collect();
    let r = (w.max(h) / 120).max(3);
    // Separable dilation: rows, then columns, with a running count.
    let dilate_1d = |src: &[bool], len: usize, stride: usize, lines: usize, lstride: usize| {
        let mut out = vec![false; src.len()];
        for l in 0..lines {
            let base = l * lstride;
            let mut count = 0usize;
            // Window [i - r, i + r].
            for i in 0..len.min(r) {
                count += src[base + i * stride] as usize;
            }
            for i in 0..len {
                if i + r < len {
                    count += src[base + (i + r) * stride] as usize;
                }
                if i > r {
                    count -= src[base + (i - r - 1) * stride] as usize;
                }
                out[base + i * stride] = count > 0;
            }
        }
        out
    };
    let rows = dilate_1d(&any, w, 1, h, w);
    let grown = dilate_1d(&rows, h, w, w, 1);
    // Label the grown mask (4-connected) and box the original pixels in each label.
    let mut label = vec![u32::MAX; w * h];
    let mut boxes: Vec<([u32; 4], usize)> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !grown[start] || label[start] != u32::MAX {
            continue;
        }
        let id = boxes.len() as u32;
        boxes.push(([u32::MAX, u32::MAX, 0, 0], 0));
        label[start] = id;
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            if any[i] {
                let b = &mut boxes[id as usize];
                b.0 = [
                    b.0[0].min(x as u32),
                    b.0[1].min(y as u32),
                    b.0[2].max(x as u32 + 1),
                    b.0[3].max(y as u32 + 1),
                ];
                b.1 += 1;
            }
            let mut push = |j: usize| {
                if grown[j] && label[j] == u32::MAX {
                    label[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                push(i - 1);
            }
            if x + 1 < w {
                push(i + 1);
            }
            if y > 0 {
                push(i - w);
            }
            if y + 1 < h {
                push(i + w);
            }
        }
    }
    for i in 0..w * h {
        if any[i] && boxes[label[i] as usize].1 < MIN_PIXELS {
            d.added[i] = false;
            d.removed[i] = false;
        }
    }
    d.boxes = boxes
        .into_iter()
        .filter(|(_, n)| *n >= MIN_PIXELS)
        .map(|(b, _)| b)
        .collect();
}

/// A transparent overlay the size of the diff: `mask` pixels in `color`, moved by `shift`
/// (to land on the old page, pass the diff's shift; on the new page, (0, 0)).
pub(crate) fn overlay(d: &PageDiff, mask: &[bool], color: [u8; 3], shift: (i32, i32)) -> Vec<u8> {
    let mut img = RgbaImage::new(d.w, d.h);
    for (i, _) in mask.iter().enumerate().filter(|(_, m)| **m) {
        let (x, y) = ((i as u32 % d.w) as i32, (i as u32 / d.w) as i32);
        let (tx, ty) = (x + shift.0, y + shift.1);
        if tx >= 0 && ty >= 0 && (tx as u32) < d.w && (ty as u32) < d.h {
            img.put_pixel(
                tx as u32,
                ty as u32,
                image::Rgba([color[0], color[1], color[2], 170]),
            );
        }
    }
    let mut out = Vec::new();
    // Encoding an in-memory RGBA buffer as PNG can't fail short of running out of memory;
    // an empty overlay is the harmless fallback.
    let _ = img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A white page with black boxes, as a renderer would produce it (with a soft edge).
    fn page(w: u32, h: u32, boxes: &[[u32; 4]], dx: i32, dy: i32) -> RgbaImage {
        let mut img = RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
        for b in boxes {
            for y in b[1]..b[3] {
                for x in b[0]..b[2] {
                    let (tx, ty) = (x as i32 + dx, y as i32 + dy);
                    if tx >= 0 && ty >= 0 && (tx as u32) < w && (ty as u32) < h {
                        img.put_pixel(tx as u32, ty as u32, image::Rgba([0, 0, 0, 255]));
                    }
                }
            }
            // Anti-aliased edge: a grey column to the right.
            for y in b[1]..b[3] {
                let (tx, ty) = (b[2] as i32 + dx, y as i32 + dy);
                if tx >= 0 && ty >= 0 && (tx as u32) < w && (ty as u32) < h {
                    img.put_pixel(tx as u32, ty as u32, image::Rgba([140, 140, 140, 255]));
                }
            }
        }
        img
    }

    const SHAPES: [[u32; 4]; 3] = [[40, 40, 140, 52], [40, 80, 300, 84], [200, 200, 260, 330]];

    #[test]
    fn same_page_has_no_changes_even_shifted_or_resampled() {
        let a = page(400, 500, &SHAPES, 0, 0);
        assert!(!diff(&a, &a).changed());
        // Fed a few pixels off: the shift is found and undone.
        let b = page(400, 500, &SHAPES, 5, -3);
        let d = diff(&a, &b);
        assert_eq!(d.shift, (-5, 3));
        assert!(!d.changed(), "{:?}", d.boxes);
        // Rendered at a different size: scaled, and anti-aliasing doesn't count.
        let c = page(800, 1000, &SHAPES.map(|b| b.map(|v| v * 2)), 0, 0);
        assert!(!diff(&c, &a).changed(), "{:?}", diff(&c, &a).boxes);
    }

    #[test]
    fn finds_what_was_added_and_removed() {
        let a = page(400, 500, &SHAPES, 0, 0);
        let mut more = SHAPES.to_vec();
        more.push([60, 400, 120, 440]);
        more.remove(0);
        let b = page(400, 500, &more, 2, 1);
        let d = diff(&a, &b);
        assert_eq!(d.boxes.len(), 2, "{:?}", d.boxes);
        let has = |mask: &[bool], x: u32, y: u32| mask[(y * d.w + x) as usize];
        // The new box (shifted by the page offset), and the old one that went.
        assert!(has(&d.added, 62 + 30, 401 + 20));
        assert!(has(&d.removed, 42 + 50, 41 + 5));
        assert!(!has(&d.added, 42 + 50, 41 + 5));
        // Each box hugs its cluster.
        for b in &d.boxes {
            assert!(b[2] - b[0] <= 120 && b[3] - b[1] <= 60, "{b:?}");
        }
        assert!(!overlay(&d, &d.added, [0, 160, 60], (0, 0)).is_empty());
    }

    #[test]
    fn specks_are_not_changes() {
        let a = page(300, 300, &SHAPES[..1], 0, 0);
        let mut b = a.clone();
        // Scanner dust: scattered single dark pixels.
        for k in 0..40u32 {
            b.put_pixel(
                (k * 37) % 290 + 5,
                (k * 53) % 290 + 5,
                image::Rgba([0, 0, 0, 255]),
            );
        }
        assert!(!diff(&a, &b).changed());
    }
}
