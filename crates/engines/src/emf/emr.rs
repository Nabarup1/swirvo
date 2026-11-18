//! Enhanced metafiles ([MS-EMF]): the header sets the picture's size and its device;
//! every record is size-checked before it's read. EMF+ records ride in comments and are
//! skipped: dual files always carry the plain EMF drawing too.

use swirvo_core::EngineError;

use super::Picture;
use super::dib;
use super::gdi::{ArcKind, GBrush, GFont, GPen, ID, M, Obj, Player};
use crate::docxlayout::props::Rgb;

/// A bounds-checked view of one record.
struct R<'a>(&'a [u8]);

impl R<'_> {
    fn u32(&self, o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.0.get(o..o + 4)?.try_into().ok()?))
    }
    fn i32(&self, o: usize) -> Option<i32> {
        self.u32(o).map(|v| v as i32)
    }
    fn f(&self, o: usize) -> Option<f32> {
        self.i32(o).map(|v| v as f32)
    }
    fn f32(&self, o: usize) -> Option<f32> {
        self.u32(o).map(f32::from_bits).filter(|v| v.is_finite())
    }
    fn i16(&self, o: usize) -> Option<f32> {
        Some(i16::from_le_bytes(self.0.get(o..o + 2)?.try_into().ok()?) as f32)
    }
    fn u16(&self, o: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.0.get(o..o + 2)?.try_into().ok()?))
    }
    fn color(&self, o: usize) -> Option<Rgb> {
        let b = self.0.get(o..o + 3)?;
        Some(Rgb(b[0], b[1], b[2]))
    }
    fn rect(&self, o: usize) -> Option<[f32; 4]> {
        Some([self.f(o)?, self.f(o + 4)?, self.f(o + 8)?, self.f(o + 12)?])
    }
    fn xform(&self, o: usize) -> Option<M> {
        Some([
            self.f32(o)?,
            self.f32(o + 4)?,
            self.f32(o + 8)?,
            self.f32(o + 12)?,
            self.f32(o + 16)?,
            self.f32(o + 20)?,
        ])
    }
    /// `n` points from `o`: 32-bit POINTL, or 16-bit POINTS.
    fn points(&self, o: usize, n: usize, small: bool) -> Option<Vec<(f32, f32)>> {
        let each = if small { 4 } else { 8 };
        let end = o.checked_add(n.checked_mul(each)?)?;
        if end > self.0.len() {
            return None;
        }
        Some(
            (0..n)
                .map(|i| {
                    let p = o + i * each;
                    if small {
                        (self.i16(p).unwrap_or(0.0), self.i16(p + 2).unwrap_or(0.0))
                    } else {
                        (self.f(p).unwrap_or(0.0), self.f(p + 4).unwrap_or(0.0))
                    }
                })
                .collect(),
        )
    }
    /// A sub-slice given as (offset, length) from the record start.
    fn part(&self, off: u32, len: u32) -> Option<&[u8]> {
        let (o, l) = (off as usize, len as usize);
        self.0.get(o..o.checked_add(l)?)
    }
}

pub(crate) fn is_emf(b: &[u8]) -> bool {
    b.len() >= 88 && b[..4] == [1, 0, 0, 0] && &b[40..44] == b" EMF"
}

/// Region data (RGNDATA): its rectangles, in device units.
fn region(d: &[u8]) -> Vec<[f32; 4]> {
    let r = R(d);
    let n = r.u32(8).unwrap_or(0) as usize;
    let hdr = r.u32(0).unwrap_or(32) as usize;
    (0..n.min(100_000))
        .map_while(|i| r.rect(hdr + i * 16))
        .collect()
}

pub(crate) fn parse(b: &[u8]) -> Result<Picture, EngineError> {
    if !is_emf(b) {
        return Err(EngineError::Decode("not an EMF picture".into()));
    }
    let h = R(b);
    let bad = || EngineError::Decode("damaged EMF header".into());
    let hsize = h.u32(4).ok_or_else(bad)? as usize;
    let bounds = h.rect(8).ok_or_else(bad)?;
    let frame = h.rect(24).ok_or_else(bad)?;
    let dev = (h.f(72).unwrap_or(0.0), h.f(76).unwrap_or(0.0));
    let mut mm = (h.f(80).unwrap_or(0.0), h.f(84).unwrap_or(0.0));
    if hsize >= 108 {
        let um = (h.f(100).unwrap_or(0.0), h.f(104).unwrap_or(0.0));
        if um.0 > 0.0 && um.1 > 0.0 {
            mm = (um.0 / 1000.0, um.1 / 1000.0);
        }
    }
    let fallback = 96.0 / 25.4;
    let dpm = |px: f32, mm: f32| {
        let v = px / mm;
        if v.is_finite() && v > 0.01 && v < 1000.0 {
            v
        } else {
            fallback
        }
    };
    let dpm = (dpm(dev.0, mm.0), dpm(dev.1, mm.1));
    let pt_per_mm = 72.0 / 25.4;
    let (fw, fh) = ((frame[2] - frame[0]) / 100.0, (frame[3] - frame[1]) / 100.0);
    let (origin, size_mm) = if fw > 0.0 && fh > 0.0 {
        (
            (frame[0] / 100.0 * dpm.0, frame[1] / 100.0 * dpm.1),
            (fw, fh),
        )
    } else {
        (
            (bounds[0], bounds[1]),
            (
                (bounds[2] - bounds[0] + 1.0).max(1.0) / dpm.0,
                (bounds[3] - bounds[1] + 1.0).max(1.0) / dpm.1,
            ),
        )
    };
    let (sx, sy) = (pt_per_mm / dpm.0, pt_per_mm / dpm.1);
    let dev2pic = [sx, 0.0, 0.0, sy, -origin.0 * sx, -origin.1 * sy];
    let w = (size_mm.0 * pt_per_mm).clamp(1.0, 14_400.0);
    let hgt = (size_mm.1 * pt_per_mm).clamp(1.0, 14_400.0);
    let mut p = Player::new(dev2pic, w, hgt, dpm);

    let mut off = 0usize;
    let mut records = 0usize;
    while off + 8 <= b.len() && records < 5_000_000 && !p.full() {
        let ty = h.u32(off).unwrap_or(0);
        let size = h.u32(off + 4).unwrap_or(0) as usize;
        if size < 8 || off + size > b.len() {
            break;
        }
        let rec = R(&b[off..off + size]);
        if ty == 14 {
            break;
        }
        // A record too short for its fields is skipped, not fatal.
        let _ = record(&mut p, ty, &rec);
        off += size;
        records += 1;
    }
    p.finish();
    Ok(Picture {
        w: p.w,
        h: p.h,
        items: std::mem::take(&mut p.items),
    })
}

fn font(r: &R<'_>, o: usize) -> Option<GFont> {
    let face: Vec<u16> = (0..32)
        .map_while(|i| r.u16(o + 28 + i * 2))
        .take_while(|&c| c != 0)
        .collect();
    Some(GFont {
        height: r.f(o)?,
        escapement: r.f(o + 8)?,
        weight: r.i32(o + 16)?,
        italic: r.0.get(o + 20).is_some_and(|&v| v != 0),
        underline: r.0.get(o + 21).is_some_and(|&v| v != 0),
        strike: r.0.get(o + 22).is_some_and(|&v| v != 0),
        charset: r.0.get(o + 23).copied().unwrap_or(0),
        face: String::from_utf16_lossy(&face),
    })
}

/// A brush from a LOGBRUSH: style, colour, hatch.
fn brush(style: u32, color: Rgb, hatch: u32) -> GBrush {
    match style {
        0 => GBrush::Solid(color),
        1 => GBrush::Null,
        2 => GBrush::Hatch(color, hatch),
        _ => GBrush::Solid(color),
    }
}

fn bitmap(r: &R<'_>, bmi: (u32, u32), bits: (u32, u32), alpha: bool) -> Option<image::RgbaImage> {
    if bmi.1 == 0 {
        return None;
    }
    dib::decode(r.part(bmi.0, bmi.1)?, r.part(bits.0, bits.1)?, alpha)
}

/// The source rectangle of a blit, inside the bitmap (bottom-up DIBs count rows from the
/// bottom for StretchDIBits).
fn crop(
    img: image::RgbaImage,
    sx: f32,
    sy: f32,
    sw: f32,
    sh: f32,
    from_bottom: bool,
) -> Option<image::RgbaImage> {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (sw, sh) = (sw.abs(), sh.abs());
    if sw <= 0.0 || sh <= 0.0 || (sx <= 0.0 && sy <= 0.0 && sw >= iw && sh >= ih) {
        return Some(img);
    }
    let x0 = sx.clamp(0.0, iw);
    let y0 = if from_bottom { ih - sy - sh } else { sy }.clamp(0.0, ih);
    let x1 = (x0 + sw).min(iw);
    let y1 = (y0 + sh).min(ih);
    if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
        return None;
    }
    Some(
        image::imageops::crop_imm(
            &img,
            x0 as u32,
            y0 as u32,
            (x1 - x0) as u32,
            (y1 - y0) as u32,
        )
        .to_image(),
    )
}

/// Raster operations that put the source on the page (copy, paint, and-ing masks).
fn draws_source(rop: u32) -> bool {
    matches!(
        rop,
        0x00CC_0020 | 0x00EE_0086 | 0x0088_00C6 | 0x00C0_00CA | 0x0066_0046 | 0x0033_0008
    )
}

fn record(p: &mut Player, ty: u32, r: &R<'_>) -> Option<()> {
    match ty {
        // Polygons and lines, 32-bit (2–6) and 16-bit (85–89) points.
        2..=6 | 85..=89 => {
            let small = ty >= 85;
            let kind = if small { ty - 83 } else { ty };
            let n = r.u32(24)? as usize;
            let pts = r.points(28, n, small)?;
            match kind {
                2 => p.poly(&pts, false, true),
                3 => p.poly(&pts, true, false),
                4 => p.poly(&pts, false, false),
                5 => p.poly_to(&pts, true),
                _ => p.poly_to(&pts, false),
            }
        }
        // PolyPolyline / PolyPolygon (7, 8; 16-bit 90, 91).
        7 | 8 | 90 | 91 => {
            let small = ty >= 90;
            let n = r.u32(24)? as usize;
            let total = r.u32(28)? as usize;
            if n > r.0.len() / 4 {
                return None;
            }
            let counts: Vec<usize> = (0..n)
                .map(|i| r.u32(32 + i * 4).map(|c| c as usize))
                .collect::<Option<_>>()?;
            if counts.iter().sum::<usize>() > total {
                return None;
            }
            let pts = r.points(32 + n * 4, total, small)?;
            let mut polys = Vec::with_capacity(n);
            let mut at = 0;
            for c in counts {
                polys.push(pts[at..at + c].to_vec());
                at += c;
            }
            p.polypoly(&polys, ty == 8 || ty == 91);
        }
        9 => p.set_window_ext(r.f(8)?, r.f(12)?),
        10 => p.set_window_org(r.f(8)?, r.f(12)?),
        11 => p.set_viewport_ext(r.f(8)?, r.f(12)?),
        12 => p.set_viewport_org(r.f(8)?, r.f(12)?),
        15 => p.set_pixel(r.f(8)?, r.f(12)?, r.color(16)?),
        17 => p.set_map_mode(r.u32(8)?),
        18 => p.set_bk_mode(r.u32(8)?),
        19 => p.set_fill_mode(r.u32(8)?),
        22 => p.set_text_align(r.u32(8)?),
        24 => p.set_text_color(r.color(8)?),
        25 => p.set_bk_color(r.color(8)?),
        27 => p.move_to(r.f(8)?, r.f(12)?),
        29 => {
            let [l, t, rr, b] = r.rect(8)?;
            p.exclude_clip(l, t, rr, b);
        }
        30 => {
            let [l, t, rr, b] = r.rect(8)?;
            p.intersect_clip(l, t, rr, b);
        }
        31 => p.scale_viewport_ext(r.f(8)?, r.f(12)?, r.f(16)?, r.f(20)?),
        32 => p.scale_window_ext(r.f(8)?, r.f(12)?, r.f(16)?, r.f(20)?),
        33 => p.save(),
        34 => p.restore(r.i32(8)?),
        35 => p.set_world(r.xform(8)?),
        36 => p.modify_world(r.xform(8)?, r.u32(32)?),
        37 => p.select(r.u32(8)?),
        38 => {
            // A LOGPEN width is in logical units; 0 means one device pixel.
            let width = r.f(16)?.abs();
            p.create(
                r.u32(8)?,
                Obj::Pen(GPen {
                    style: r.u32(12)?,
                    width,
                    color: r.color(24)?,
                    cosmetic: width == 0.0,
                    user: Vec::new(),
                }),
            );
        }
        39 => p.create(
            r.u32(8)?,
            Obj::Brush(brush(r.u32(12)?, r.color(16)?, r.u32(20)?)),
        ),
        40 => p.delete(r.u32(8)?),
        41 => p.angle_arc(r.f(8)?, r.f(12)?, r.u32(16)? as f32, r.f32(20)?, r.f32(24)?),
        42 => {
            let [l, t, rr, b] = r.rect(8)?;
            p.ellipse(l, t, rr, b);
        }
        43 => {
            let [l, t, rr, b] = r.rect(8)?;
            p.rectangle(l, t, rr, b);
        }
        44 => {
            let [l, t, rr, b] = r.rect(8)?;
            p.round_rect(l, t, rr, b, r.f(24)?, r.f(28)?);
        }
        45 | 46 | 47 | 55 => {
            let kind = match ty {
                45 => ArcKind::Arc,
                46 => ArcKind::Chord,
                47 => ArcKind::Pie,
                _ => ArcKind::ArcTo,
            };
            p.arc(kind, r.rect(8)?, (r.f(24)?, r.f(28)?), (r.f(32)?, r.f(36)?));
        }
        49 => p.create(r.u32(8)?, Obj::Other),
        54 => p.line_to(r.f(8)?, r.f(12)?),
        56 | 92 => {
            let small = ty == 92;
            let n = r.u32(24)? as usize;
            let pts = r.points(28, n, small)?;
            let at = 28 + n * if small { 4 } else { 8 };
            let kinds = r.0.get(at..at + n)?;
            p.poly_draw(&pts, kinds);
        }
        57 => p.set_arc_direction(r.u32(8)?),
        59 => p.begin_path(),
        60 => p.end_path(),
        61 => p.close_figure(),
        62 => p.paint_path(true, false),
        63 => p.paint_path(true, true),
        64 => p.paint_path(false, true),
        67 => p.select_clip_path(r.u32(8)?),
        68 => p.abort_path(),
        // FILLRGN (brush handle), PAINTRGN (current brush).
        71 | 74 => {
            let len = r.u32(24)?;
            let (brush, at) = if ty == 71 {
                (Some(r.u32(28)?), 32)
            } else {
                (None, 28)
            };
            let rects = region(r.part(at, len)?);
            let b = match brush {
                Some(h) if h & 0x8000_0000 != 0 => None,
                Some(h) => match p.objects.get(&h) {
                    Some(Obj::Brush(b)) => Some(b.clone()),
                    _ => return None,
                },
                None => None,
            };
            p.fill_device_rects(&rects, b);
        }
        75 => {
            let len = r.u32(8)?;
            let mode = r.u32(12)?;
            if len == 0 {
                if mode == 5 {
                    p.reset_clip();
                }
            } else {
                p.clip_device_rects(&region(r.part(16, len)?), mode);
            }
        }
        // BITBLT, STRETCHBLT, ALPHABLEND, TRANSPARENTBLT.
        76 | 77 | 114 | 116 => {
            let (x, y, w, h) = (r.f(24)?, r.f(28)?, r.f(32)?, r.f(36)?);
            let rop = r.u32(40)?;
            let bmi = (r.u32(84)?, r.u32(88)?);
            let bits = (r.u32(92)?, r.u32(96)?);
            if bmi.1 == 0 {
                if ty == 76 || ty == 77 {
                    p.pat_blt(x, y, w, h, rop);
                }
                return Some(());
            }
            let alpha = ty == 114 && r.0.get(43).is_some_and(|&f| f & 1 != 0);
            let mut img = bitmap(r, bmi, bits, alpha)?;
            if ty == 114 {
                let k = r.0.get(42).copied().unwrap_or(255) as u32;
                if k < 255 {
                    for px in img.pixels_mut() {
                        px.0[3] = (px.0[3] as u32 * k / 255) as u8;
                    }
                }
            } else if ty == 116 {
                let key = r.color(40)?;
                for px in img.pixels_mut() {
                    if px.0[..3] == [key.0, key.1, key.2] {
                        px.0[3] = 0;
                    }
                }
            } else if !draws_source(rop) {
                return Some(());
            }
            let (sx, sy) = (r.f(44)?, r.f(48)?);
            let (sw, sh) = if ty == 76 {
                (w, h)
            } else {
                (r.f(100)?, r.f(104)?)
            };
            let src = r.xform(52).unwrap_or(ID);
            // The source rectangle is in the source DC's space; its transform is usually
            // the identity. Otherwise the whole bitmap is drawn.
            let img = if src == ID {
                crop(img, sx, sy, sw, sh, false)?
            } else {
                img
            };
            p.blit(x, y, w, h, img);
        }
        // SETDIBITSTODEVICE: drawn 1:1.
        80 => {
            let (x, y) = (r.f(24)?, r.f(28)?);
            let (sx, sy, sw, sh) = (r.f(32)?, r.f(36)?, r.f(40)?, r.f(44)?);
            let img = bitmap(r, (r.u32(48)?, r.u32(52)?), (r.u32(56)?, r.u32(60)?), false)?;
            let img = crop(img, sx, sy, sw, sh, true)?;
            p.blit(x, y, sw, sh, img);
        }
        81 => {
            let (x, y) = (r.f(24)?, r.f(28)?);
            let (sx, sy, sw, sh) = (r.f(32)?, r.f(36)?, r.f(40)?, r.f(44)?);
            let rop = r.u32(68)?;
            let (w, h) = (r.f(72)?, r.f(76)?);
            let bmi = (r.u32(48)?, r.u32(52)?);
            if bmi.1 == 0 {
                p.pat_blt(x, y, w, h, rop);
                return Some(());
            }
            if !draws_source(rop) {
                return Some(());
            }
            let img = bitmap(r, bmi, (r.u32(56)?, r.u32(60)?), false)?;
            let img = crop(img, sx, sy, sw, sh, true)?;
            p.blit(x, y, w, h, img);
        }
        82 => p.create(r.u32(8)?, Obj::Font(font(r, 12)?)),
        // EXTTEXTOUTA / EXTTEXTOUTW.
        83 | 84 => {
            let (x, y) = (r.f(36)?, r.f(40)?);
            let n = r.u32(44)? as usize;
            let off_s = r.u32(48)? as usize;
            let opts = r.u32(52)?;
            let has_rect = opts & 0x100 == 0;
            let rect = if has_rect { r.rect(56) } else { None };
            let off_dx = r.u32(if has_rect { 72 } else { 56 })? as usize;
            if opts & 0x10 != 0 {
                // Glyph indices of a font we may not have: nothing reliable to show.
                return Some(());
            }
            let (text, units): (String, Vec<usize>) = if ty == 84 {
                let units: Vec<u16> = (0..n.min(r.0.len() / 2))
                    .map_while(|i| r.u16(off_s + i * 2))
                    .collect();
                utf16_with_units(&units)
            } else {
                let bytes = r.0.get(off_s..off_s.checked_add(n)?)?;
                let cs = p.font_charset();
                (
                    bytes.iter().map(|&c| ansi(c, cs)).collect(),
                    vec![1; bytes.len()],
                )
            };
            let pairs = opts & 0x2000 != 0;
            let step = if pairs { 8 } else { 4 };
            let raw: Vec<f32> = if off_dx == 0 {
                Vec::new()
            } else {
                (0..n.min(r.0.len() / 4))
                    .map_while(|i| r.f(off_dx + i * step))
                    .collect()
            };
            let dx = merge_units(&raw, &units);
            let opaque = (opts & 2 != 0).then_some(rect).flatten();
            let clip = (opts & 4 != 0).then_some(rect).flatten();
            p.text(x, y, text, dx, opaque, clip);
        }
        // CREATEMONOBRUSH, CREATEDIBPATTERNBRUSHPT: the bitmap's average colour.
        93 | 94 => {
            let img = bitmap(r, (r.u32(16)?, r.u32(20)?), (r.u32(24)?, r.u32(28)?), false);
            let c = img.map_or([128, 128, 128], |i| dib::average(&i));
            p.create(r.u32(8)?, Obj::Brush(GBrush::Solid(Rgb(c[0], c[1], c[2]))));
        }
        95 => {
            let style = r.u32(28)?;
            let width = r.u32(32)? as f32;
            let brush_style = r.u32(36)?;
            let color = r.color(40)?;
            let n = r.u32(48)? as usize;
            let user: Vec<f32> = (0..n.min(16)).map_while(|i| r.f(52 + i * 4)).collect();
            let mut pen = GPen {
                style,
                width,
                color,
                cosmetic: style & 0x0001_0000 == 0,
                user,
            };
            if brush_style == 1 {
                pen.style = (pen.style & !0xF) | 5;
            }
            p.create(r.u32(8)?, Obj::Pen(pen));
        }
        // SMALLTEXTOUT.
        108 => {
            let (x, y) = (r.f(8)?, r.f(12)?);
            let n = r.u32(16)? as usize;
            let opts = r.u32(20)?;
            let at: usize = if opts & 0x100 == 0 { 52 } else { 36 };
            let text: String = if opts & 0x200 != 0 {
                let cs = p.font_charset();
                r.0.get(at..at.checked_add(n)?)?
                    .iter()
                    .map(|&c| ansi(c, cs))
                    .collect()
            } else {
                let units: Vec<u16> = (0..n.min(r.0.len() / 2))
                    .map_while(|i| r.u16(at + i * 2))
                    .collect();
                String::from_utf16_lossy(&units)
            };
            p.text(x, y, text, Vec::new(), None, None);
        }
        // GRADIENTFILL: rectangles (modes 0, 1) or triangles (2).
        118 => {
            let nv = r.u32(24)? as usize;
            let nt = r.u32(28)? as usize;
            let mode = r.u32(32)?;
            if nv > r.0.len() / 16 || nt > r.0.len() / 8 {
                return None;
            }
            let vert = |i: usize| -> Option<((f32, f32), Rgb)> {
                let o = 36 + i * 16;
                let c = |k: usize| r.u16(o + 8 + k * 2).map(|v| (v >> 8) as u8);
                Some(((r.f(o)?, r.f(o + 4)?), Rgb(c(0)?, c(1)?, c(2)?)))
            };
            let at = 36 + nv * 16;
            for k in 0..nt {
                if mode == 2 {
                    let o = at + k * 12;
                    let (a, b, c) = (
                        vert(r.u32(o)? as usize)?,
                        vert(r.u32(o + 4)? as usize)?,
                        vert(r.u32(o + 8)? as usize)?,
                    );
                    let avg = |f: fn(&Rgb) -> u8| {
                        ((f(&a.1) as u32 + f(&b.1) as u32 + f(&c.1) as u32) / 3) as u8
                    };
                    p.gradient_triangle(
                        [a.0, b.0, c.0],
                        Rgb(avg(|c| c.0), avg(|c| c.1), avg(|c| c.2)),
                    );
                } else {
                    let o = at + k * 8;
                    let (a, b) = (vert(r.u32(o)? as usize)?, vert(r.u32(o + 4)? as usize)?);
                    p.gradient_rect(a.0, b.0, a.1, b.1, mode == 1);
                }
            }
        }
        _ => {}
    }
    Some(())
}

/// Decode UTF-16, remembering how many code units each character used (advance arrays
/// have one entry per unit).
fn utf16_with_units(units: &[u16]) -> (String, Vec<usize>) {
    let mut s = String::new();
    let mut per = Vec::new();
    for r in char::decode_utf16(units.iter().copied()) {
        let c = r.unwrap_or('\u{FFFD}');
        s.push(c);
        per.push(c.len_utf16());
    }
    (s, per)
}

/// Per-unit advances → per-character advances.
pub(crate) fn merge_units(raw: &[f32], units: &[usize]) -> Vec<f32> {
    if raw.len() < units.iter().sum::<usize>() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(units.len());
    let mut at = 0;
    for &u in units {
        out.push(raw[at..at + u].iter().sum());
        at += u;
    }
    out
}

/// One byte of 8-bit text: Windows-1252, or the symbol range of a symbol font.
pub(crate) fn ansi(b: u8, charset: u8) -> char {
    if charset == 2 && b >= 0x20 {
        return char::from_u32(0xF000 + b as u32).unwrap_or(' ');
    }
    match b {
        0x80..=0x9F => crate::docxlayout::rtf::cp1252(b),
        _ => b as char,
    }
}
