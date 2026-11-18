//! Windows metafiles ([MS-WMF]), 16-bit, with or without the placeable header. Records
//! store their parameters last-first (`y` before `x`, `bottom` before `left`). Objects
//! take the lowest free slot of the object table, as GDI does, and every kind of object
//! (palettes and regions too) uses a slot.

use swirvo_core::EngineError;

use super::Picture;
use super::dib;
use super::emr::ansi;
use super::gdi::{ArcKind, GBrush, GFont, GPen, Obj, Player};
use crate::docxlayout::props::Rgb;

const PLACEABLE: u32 = 0x9AC6_CDD7;

struct R<'a>(&'a [u8]);

impl R<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        Some(u16::from_le_bytes(self.0.get(o..o + 2)?.try_into().ok()?))
    }
    /// A signed 16-bit parameter as f32.
    fn s(&self, o: usize) -> Option<f32> {
        self.u16(o).map(|v| v as i16 as f32)
    }
    fn u32(&self, o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(self.0.get(o..o + 4)?.try_into().ok()?))
    }
    fn color(&self, o: usize) -> Option<Rgb> {
        let b = self.0.get(o..o + 3)?;
        Some(Rgb(b[0], b[1], b[2]))
    }
    fn points(&self, o: usize, n: usize) -> Option<Vec<(f32, f32)>> {
        if o.checked_add(n.checked_mul(4)?)? > self.0.len() {
            return None;
        }
        Some(
            (0..n)
                .map(|i| {
                    (
                        self.s(o + i * 4).unwrap_or(0.0),
                        self.s(o + i * 4 + 2).unwrap_or(0.0),
                    )
                })
                .collect(),
        )
    }
}

pub(crate) fn is_wmf(b: &[u8]) -> bool {
    let r = R(b);
    if r.u32(0) == Some(PLACEABLE) {
        return true;
    }
    matches!(r.u16(0), Some(1 | 2))
        && r.u16(2) == Some(9)
        && matches!(r.u16(4), Some(0x0100 | 0x0300))
}

/// The first window origin and extent set before anything is drawn (a WMF without a
/// placeable header has no other size).
fn first_window(b: &[u8], start: usize) -> ((f32, f32), Option<(f32, f32)>) {
    let r = R(b);
    let (mut org, mut ext) = ((0.0, 0.0), None);
    let mut off = start;
    for _ in 0..10_000 {
        let Some(words) = r.u32(off) else { break };
        let Some(func) = r.u16(off + 4) else { break };
        let size = words as usize * 2;
        if size < 6 || off + size > b.len() || func == 0 {
            break;
        }
        match func {
            0x020B => org = (r.s(off + 8).unwrap_or(0.0), r.s(off + 6).unwrap_or(0.0)),
            0x020C => {
                let e = (r.s(off + 8).unwrap_or(0.0), r.s(off + 6).unwrap_or(0.0));
                if e.0 != 0.0 && e.1 != 0.0 {
                    ext = Some(e);
                    break;
                }
            }
            _ => {}
        }
        off += size;
    }
    (org, ext)
}

pub(crate) fn parse(b: &[u8]) -> Result<Picture, EngineError> {
    if !is_wmf(b) {
        return Err(EngineError::Decode("not a WMF picture".into()));
    }
    let h = R(b);
    let bad = || EngineError::Decode("damaged WMF header".into());
    let (start, bbox, inch) = if h.u32(0) == Some(PLACEABLE) {
        let l = h.s(6).ok_or_else(bad)?;
        let t = h.s(8).ok_or_else(bad)?;
        let r = h.s(10).ok_or_else(bad)?;
        let bt = h.s(12).ok_or_else(bad)?;
        let inch = h.u16(14).filter(|&v| v > 0).unwrap_or(1440) as f32;
        (22, Some([l.min(r), t.min(bt), l.max(r), t.max(bt)]), inch)
    } else {
        (0, None, 0.0)
    };
    let hdr_words = h.u16(start + 2).ok_or_else(bad)? as usize;
    let n_objects = h.u16(start + 10).ok_or_else(bad)? as usize;
    let rec0 = start + hdr_words.max(9) * 2;
    let (org, ext) = first_window(b, rec0);
    // The picture's box in metafile units, and how many of those make an inch.
    let (bx, inch) = match (bbox, ext) {
        (Some(bx), _) if bx[2] > bx[0] && bx[3] > bx[1] => (bx, inch),
        (_, Some(e)) => {
            let (x0, x1) = (org.0.min(org.0 + e.0), org.0.max(org.0 + e.0));
            let (y0, y1) = (org.1.min(org.1 + e.1), org.1.max(org.1 + e.1));
            // No stated resolution: twips for big extents (Office), else screen pixels.
            let guess = if (x1 - x0).max(y1 - y0) > 3000.0 {
                1440.0
            } else {
                96.0
            };
            ([x0, y0, x1, y1], if inch > 0.0 { inch } else { guess })
        }
        _ => (
            [0.0, 0.0, 1000.0, 1000.0],
            if inch > 0.0 { inch } else { 1440.0 },
        ),
    };
    let s = 72.0 / inch;
    let dev2pic = [s, 0.0, 0.0, s, -bx[0] * s, -bx[1] * s];
    let w = ((bx[2] - bx[0]) * s).clamp(1.0, 14_400.0);
    let hgt = ((bx[3] - bx[1]) * s).clamp(1.0, 14_400.0);
    let mut p = Player::new(dev2pic, w, hgt, (inch / 25.4, inch / 25.4));
    p.set_initial_window((bx[0], bx[1]), (bx[2] - bx[0], bx[3] - bx[1]));

    let mut table: Vec<bool> = vec![false; n_objects.min(65_535)];
    let mut off = rec0;
    let mut records = 0usize;
    while off + 6 <= b.len() && records < 5_000_000 && !p.full() {
        let words = h.u32(off).unwrap_or(0) as usize;
        let func = h.u16(off + 4).unwrap_or(0);
        let size = words.saturating_mul(2);
        if func == 0 || size < 6 || off + size > b.len() {
            break;
        }
        let _ = record(&mut p, &mut table, func, &R(&b[off..off + size]));
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

/// Take the lowest free object slot.
fn slot(table: &mut Vec<bool>) -> u32 {
    match table.iter().position(|used| !used) {
        Some(i) => {
            table[i] = true;
            i as u32
        }
        None => {
            if table.len() < 65_535 {
                table.push(true);
            }
            table.len() as u32 - 1
        }
    }
}

fn text_of(bytes: &[u8], charset: u8) -> String {
    bytes.iter().map(|&c| ansi(c, charset)).collect()
}

fn record(p: &mut Player, table: &mut Vec<bool>, func: u16, r: &R<'_>) -> Option<()> {
    let s = |o: usize| r.s(o);
    match func {
        0x001E => p.save(),
        0x0102 => p.set_bk_mode(r.u16(6)? as u32),
        0x0103 => p.set_map_mode(r.u16(6)? as u32),
        0x0106 => p.set_fill_mode(r.u16(6)? as u32),
        0x0127 => p.restore(s(6)? as i32),
        0x012C => {
            let i = r.u16(6)? as u32;
            p.select(i);
        }
        0x012D => p.select(r.u16(6)? as u32),
        0x012E => p.set_text_align(r.u16(6)? as u32),
        0x01F0 => {
            let i = r.u16(6)? as usize;
            p.delete(i as u32);
            if let Some(u) = table.get_mut(i) {
                *u = false;
            }
        }
        0x0201 => p.set_bk_color(r.color(6)?),
        0x0209 => p.set_text_color(r.color(6)?),
        0x020B => p.set_window_org(s(8)?, s(6)?),
        0x020C => p.set_window_ext(s(8)?, s(6)?),
        0x020D => p.set_viewport_org(s(8)?, s(6)?),
        0x020E => p.set_viewport_ext(s(8)?, s(6)?),
        0x020F => p.offset_window_org(s(8)?, s(6)?),
        0x0211 => p.offset_viewport_org(s(8)?, s(6)?),
        0x0410 => p.scale_window_ext(s(12)?, s(10)?, s(8)?, s(6)?),
        0x0412 => p.scale_viewport_ext(s(12)?, s(10)?, s(8)?, s(6)?),
        0x0213 => p.line_to(s(8)?, s(6)?),
        0x0214 => p.move_to(s(8)?, s(6)?),
        0x0415 => p.exclude_clip(s(12)?, s(10)?, s(8)?, s(6)?),
        0x0416 => p.intersect_clip(s(12)?, s(10)?, s(8)?, s(6)?),
        0x0418 => p.ellipse(s(12)?, s(10)?, s(8)?, s(6)?),
        0x041B => p.rectangle(s(12)?, s(10)?, s(8)?, s(6)?),
        0x041F => p.set_pixel(s(12)?, s(10)?, r.color(6)?),
        0x061C => p.round_rect(s(16)?, s(14)?, s(12)?, s(10)?, s(8)?, s(6)?),
        0x061D => p.pat_blt(s(16)?, s(14)?, s(12)?, s(10)?, r.u32(6)?),
        0x0817 | 0x081A | 0x0830 => {
            let kind = match func {
                0x0817 => ArcKind::Arc,
                0x081A => ArcKind::Pie,
                _ => ArcKind::Chord,
            };
            p.arc(
                kind,
                [s(20)?, s(18)?, s(16)?, s(14)?],
                (s(12)?, s(10)?),
                (s(8)?, s(6)?),
            );
        }
        0x0324 | 0x0325 => {
            let n = r.u16(6)? as usize;
            let pts = r.points(8, n)?;
            p.poly(&pts, func == 0x0324, false);
        }
        0x0538 => {
            let n = r.u16(6)? as usize;
            let counts: Vec<usize> = (0..n)
                .map(|i| r.u16(8 + i * 2).map(|c| c as usize))
                .collect::<Option<_>>()?;
            let total: usize = counts.iter().sum();
            let pts = r.points(8 + n * 2, total)?;
            let mut polys = Vec::with_capacity(n);
            let mut at = 0;
            for c in counts {
                polys.push(pts[at..at + c].to_vec());
                at += c;
            }
            p.polypoly(&polys, true);
        }
        0x02FA => {
            let i = slot(table);
            let width = s(8)?.abs();
            p.create(
                i,
                Obj::Pen(GPen {
                    style: r.u16(6)? as u32,
                    width,
                    color: r.color(12)?,
                    cosmetic: width == 0.0,
                    user: Vec::new(),
                }),
            );
        }
        0x02FC => {
            let i = slot(table);
            let style = r.u16(6)?;
            let color = r.color(8)?;
            let hatch = r.u16(12).unwrap_or(0) as u32;
            let b = match style {
                1 => GBrush::Null,
                2 => GBrush::Hatch(color, hatch),
                _ => GBrush::Solid(color),
            };
            p.create(i, Obj::Brush(b));
        }
        0x02FB => {
            let i = slot(table);
            let name: Vec<u8> =
                r.0.get(24..)
                    .unwrap_or(&[])
                    .iter()
                    .take(32)
                    .take_while(|&&c| c != 0)
                    .copied()
                    .collect();
            let charset = r.0.get(19).copied().unwrap_or(0);
            p.create(
                i,
                Obj::Font(GFont {
                    height: s(6)?,
                    escapement: s(10).unwrap_or(0.0),
                    weight: s(14).unwrap_or(400.0) as i32,
                    italic: r.0.get(16).is_some_and(|&v| v != 0),
                    underline: r.0.get(17).is_some_and(|&v| v != 0),
                    strike: r.0.get(18).is_some_and(|&v| v != 0),
                    charset,
                    face: text_of(&name, 0),
                }),
            );
        }
        // Palettes and pattern brushes also take slots.
        0x00F7 | 0x01F9 => {
            let i = slot(table);
            let o = if func == 0x01F9 {
                Obj::Brush(GBrush::Solid(Rgb(128, 128, 128)))
            } else {
                Obj::Other
            };
            p.create(i, o);
        }
        0x0142 => {
            let i = slot(table);
            let c =
                r.0.get(10..)
                    .and_then(dib::split)
                    .and_then(|(bmi, bits)| dib::decode(bmi, bits, false))
                    .map_or([128, 128, 128], |img| dib::average(&img));
            p.create(i, Obj::Brush(GBrush::Solid(Rgb(c[0], c[1], c[2]))));
        }
        0x06FF => {
            let i = slot(table);
            let bounds = (|| Some([s(20)?, s(22)?, s(24)?, s(26)?]))();
            p.create(i, Obj::Region(bounds));
        }
        0x0521 => {
            let n = r.u16(6)? as usize;
            let bytes = r.0.get(8..8 + n)?;
            let at = 8 + n.div_ceil(2) * 2;
            let text = text_of(bytes, p.font_charset());
            p.text(s(at + 2)?, s(at)?, text, Vec::new(), None, None);
        }
        0x0A32 => {
            let (y, x) = (s(6)?, s(8)?);
            let n = r.u16(10)? as usize;
            let opts = r.u16(12)?;
            let has_rect = opts & 6 != 0;
            let rect = if has_rect {
                Some([s(14)?, s(16)?, s(18)?, s(20)?])
            } else {
                None
            };
            let at = if has_rect { 22 } else { 14 };
            let bytes = r.0.get(at..at + n)?;
            let text = text_of(bytes, p.font_charset());
            let dx_at = at + n.div_ceil(2) * 2;
            let dx: Vec<f32> = if r.0.len() >= dx_at + n * 2 {
                (0..n).filter_map(|i| s(dx_at + i * 2)).collect()
            } else {
                Vec::new()
            };
            let opaque = (opts & 2 != 0).then_some(rect).flatten();
            let clip = (opts & 4 != 0).then_some(rect).flatten();
            p.text(x, y, text, dx, opaque, clip);
        }
        // DIBBITBLT, DIBSTRETCHBLT, STRETCHDIB, SETDIBTODEV.
        0x0940 | 0x0B41 | 0x0F43 | 0x0D33 => blit(p, func, r)?,
        _ => {}
    }
    Some(())
}

fn blit(p: &mut Player, func: u16, r: &R<'_>) -> Option<()> {
    let s = |o: usize| r.s(o);
    let size = r.0.len();
    // Without a bitmap the record is just its fixed fields: then it fills with the brush.
    let no_bitmap = |fixed: usize| size <= fixed;
    match func {
        0x0940 => {
            let rop = r.u32(6)?;
            if no_bitmap(24) {
                p.pat_blt(s(22)?, s(20)?, s(18)?, s(16)?, rop);
                return Some(());
            }
            let img = packed(r, 22)?;
            let (sy, sx, h, w) = (s(10)?, s(12)?, s(14)?, s(16)?);
            let img = sub(img, sx, sy, w, h)?;
            p.blit(s(20)?, s(18)?, w, h, img);
        }
        0x0B41 => {
            let rop = r.u32(6)?;
            if no_bitmap(30) {
                p.pat_blt(s(26)?, s(24)?, s(22)?, s(20)?, rop);
                return Some(());
            }
            let (sh, sw, sy, sx) = (s(10)?, s(12)?, s(14)?, s(16)?);
            let (dh, dw, dy, dx) = (s(18)?, s(20)?, s(22)?, s(24)?);
            let img = sub(packed(r, 26)?, sx, sy, sw, sh)?;
            p.blit(dx, dy, dw, dh, img);
        }
        0x0F43 => {
            let (sh, sw, sy, sx) = (s(12)?, s(14)?, s(16)?, s(18)?);
            let (dh, dw, dy, dx) = (s(20)?, s(22)?, s(24)?, s(26)?);
            let img = sub(packed(r, 28)?, sx, sy, sw, sh)?;
            p.blit(dx, dy, dw, dh, img);
        }
        _ => {
            let (h, w, dy, dx) = (s(16)?, s(18)?, s(20)?, s(22)?);
            let img = packed(r, 24)?;
            p.blit(dx, dy, w, h, img);
        }
    }
    Some(())
}

fn packed(r: &R<'_>, at: usize) -> Option<image::RgbaImage> {
    let (bmi, bits) = dib::split(r.0.get(at..)?)?;
    dib::decode(bmi, bits, false)
}

/// The source rectangle (rows counted from the top of the bitmap as decoded).
fn sub(img: image::RgbaImage, sx: f32, sy: f32, sw: f32, sh: f32) -> Option<image::RgbaImage> {
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    let (sw, sh) = (sw.abs(), sh.abs());
    if sw <= 0.0 || sh <= 0.0 || (sx <= 0.0 && sy <= 0.0 && sw >= iw && sh >= ih) {
        return Some(img);
    }
    let x0 = sx.clamp(0.0, iw);
    let y0 = sy.clamp(0.0, ih);
    let (x1, y1) = ((x0 + sw).min(iw), (y0 + sh).min(ih));
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
