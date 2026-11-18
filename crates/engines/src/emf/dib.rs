//! Device-independent bitmaps: the pixels inside metafile blits and pattern brushes.
//!
//! Uncompressed 1, 4, 8, 16, 24 and 32-bit DIBs (with or without bit-field masks) are
//! decoded here, so the fourth byte of a 32-bit pixel is only alpha when the record says
//! so (plain blits often leave it 0). RLE, JPEG and PNG bitmaps go through the `image`
//! crate. Every size is checked against the data before anything is allocated.

use image::RgbaImage;

/// Largest bitmap decoded, in pixels.
pub(crate) const MAX_PIXELS: u64 = 40_000_000;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    u32_at(b, o).map(|v| v as i32)
}

struct Header {
    size: usize,
    w: u32,
    h: u32,
    bottom_up: bool,
    bpp: u16,
    compression: u32,
    colors: usize,
    core: bool,
}

fn header(bmi: &[u8]) -> Option<Header> {
    let size = u32_at(bmi, 0)? as usize;
    if size == 12 {
        let w = u16_at(bmi, 4)? as u32;
        let h = u16_at(bmi, 6)? as u32;
        let bpp = u16_at(bmi, 10)?;
        return Some(Header {
            size,
            w,
            h,
            bottom_up: true,
            bpp,
            compression: 0,
            colors: if bpp <= 8 { 1 << bpp } else { 0 },
            core: true,
        });
    }
    if !(40..=1024).contains(&size) {
        return None;
    }
    let w = i32_at(bmi, 4)?;
    let h = i32_at(bmi, 8)?;
    let bpp = u16_at(bmi, 14)?;
    let compression = u32_at(bmi, 16)?;
    let used = u32_at(bmi, 32)? as usize;
    let colors = if bpp <= 8 {
        if used == 0 || used > 256 {
            1 << bpp
        } else {
            used
        }
    } else {
        used.min(256)
    };
    Some(Header {
        size,
        w: w.unsigned_abs(),
        h: h.unsigned_abs(),
        bottom_up: h > 0,
        bpp,
        compression,
        colors,
        core: false,
    })
}

/// Bytes of masks stored after a 40-byte header (BI_BITFIELDS, BI_ALPHABITFIELDS).
fn mask_bytes(h: &Header) -> usize {
    match (h.size, h.compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    }
}

/// Split a packed DIB (header, colour table, pixels: as WMF stores it) into the
/// BITMAPINFO and the pixels.
pub(crate) fn split(packed: &[u8]) -> Option<(&[u8], &[u8])> {
    let h = header(packed)?;
    let entry = if h.core { 3 } else { 4 };
    let info = h.size + mask_bytes(&h) + h.colors * entry;
    if info > packed.len() {
        return None;
    }
    Some(packed.split_at(info))
}

fn masks(bmi: &[u8], h: &Header) -> Option<[u32; 4]> {
    // Right after the 40-byte part, whether as extra fields (V4/V5) or appended masks.
    let at = 40;
    let r = u32_at(bmi, at)?;
    let g = u32_at(bmi, at + 4)?;
    let b = u32_at(bmi, at + 8)?;
    let a = if h.compression == 6 || h.size >= 56 {
        u32_at(bmi, at + 12).unwrap_or(0)
    } else {
        0
    };
    Some([r, g, b, a])
}

fn channel(v: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let max = mask >> shift;
    let x = (v & mask) >> shift;
    ((x as u64 * 255 + max as u64 / 2) / max as u64) as u8
}

/// Decode `bits` described by `bmi`. With `alpha`, a 32-bit bitmap's fourth byte is its
/// (premultiplied) alpha, as in AlphaBlend; otherwise every pixel is opaque.
pub(crate) fn decode(bmi: &[u8], bits: &[u8], alpha: bool) -> Option<RgbaImage> {
    let h = header(bmi)?;
    if h.w == 0 || h.h == 0 || h.w as u64 * h.h as u64 > MAX_PIXELS {
        return None;
    }
    match h.compression {
        // RLE: let the BMP decoder do it, behind a synthetic file header.
        1 | 2 => return via_bmp(bmi, bits),
        4 | 5 => return image::load_from_memory(bits).ok().map(|i| i.to_rgba8()),
        0 | 3 | 6 => {}
        _ => return None,
    }
    let entry = if h.core { 3 } else { 4 };
    let pal_at = h.size + mask_bytes(&h);
    let palette: Vec<[u8; 3]> = (0..h.colors)
        .map(|i| {
            let o = pal_at + i * entry;
            match bmi.get(o..o + 3) {
                Some(p) => [p[2], p[1], p[0]],
                None => {
                    // A short table: a gray ramp keeps 1-bit masks black and white.
                    let v = (i * 255 / (h.colors - 1).max(1)) as u8;
                    [v, v, v]
                }
            }
        })
        .collect();
    let m = if matches!(h.compression, 3 | 6) {
        masks(bmi, &h)?
    } else if h.bpp == 16 {
        [0x7C00, 0x03E0, 0x001F, 0]
    } else {
        [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0]
    };
    if !matches!(h.bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return None;
    }
    let stride = (h.w as usize * h.bpp as usize).div_ceil(32) * 4;
    let mut img = RgbaImage::new(h.w, h.h);
    let rows = (bits.len() / stride.max(1)).min(h.h as usize);
    for r in 0..rows {
        let line = &bits[r * stride..(r + 1) * stride];
        let y = if h.bottom_up {
            h.h - 1 - r as u32
        } else {
            r as u32
        };
        for x in 0..h.w {
            let xi = x as usize;
            let px = match h.bpp {
                1 | 4 | 8 => {
                    let bpp = h.bpp as usize;
                    let bit = xi * bpp;
                    let byte = line[bit / 8];
                    let shift = 8 - bpp - (bit % 8);
                    let idx = ((byte >> shift) as usize) & ((1 << bpp) - 1);
                    let c = palette.get(idx).copied().unwrap_or([0, 0, 0]);
                    [c[0], c[1], c[2], 255]
                }
                16 => {
                    let v = u16::from_le_bytes([line[xi * 2], line[xi * 2 + 1]]) as u32;
                    [channel(v, m[0]), channel(v, m[1]), channel(v, m[2]), 255]
                }
                24 => {
                    let p = &line[xi * 3..xi * 3 + 3];
                    [p[2], p[1], p[0], 255]
                }
                _ => {
                    let p = &line[xi * 4..xi * 4 + 4];
                    let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                    let (r, g, b) = (channel(v, m[0]), channel(v, m[1]), channel(v, m[2]));
                    let a = if alpha {
                        if m[3] != 0 { channel(v, m[3]) } else { p[3] }
                    } else {
                        255
                    };
                    if alpha && a < 255 {
                        // Premultiplied in the file; straight for the PDF.
                        let un = |c: u8| {
                            if a == 0 {
                                0
                            } else {
                                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
                            }
                        };
                        [un(r), un(g), un(b), a]
                    } else {
                        [r, g, b, a]
                    }
                }
            };
            img.put_pixel(x, y, image::Rgba(px));
        }
    }
    Some(img)
}

fn via_bmp(bmi: &[u8], bits: &[u8]) -> Option<RgbaImage> {
    let off = 14 + bmi.len();
    let total = off.checked_add(bits.len())?;
    let mut file = Vec::with_capacity(total);
    file.extend(b"BM");
    file.extend((total as u32).to_le_bytes());
    file.extend([0u8; 4]);
    file.extend((off as u32).to_le_bytes());
    file.extend(bmi);
    file.extend(bits);
    image::load_from_memory_with_format(&file, image::ImageFormat::Bmp)
        .ok()
        .map(|i| i.to_rgba8())
}

/// The average colour of a bitmap (pattern brushes are filled with it).
pub(crate) fn average(img: &RgbaImage) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for p in img
        .pixels()
        .step_by(((img.width() * img.height()) as usize / 4096).max(1))
    {
        for (s, c) in sum.iter_mut().zip(p.0) {
            *s += c as u64;
        }
        n += 1;
    }
    if n == 0 {
        return [128, 128, 128];
    }
    sum.map(|s| (s / n) as u8)
}
