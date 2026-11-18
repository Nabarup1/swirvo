//! Photoshop (PSD/PSB) → PNG: the flattened composite image every PSD carries (when saved
//! with "Maximize compatibility", Photoshop's default). Layers are not re-rendered.
//!
//! Supported: 1/8/16-bit; Bitmap, Grayscale, Indexed, RGB, CMYK, Duotone (as gray);
//! raw, PackBits RLE, ZIP and ZIP-with-prediction image data. A transparency channel is
//! un-matted from the white background Photoshop composites onto.

use std::io::Read;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

/// Largest composite we decode (pixels).
const MAX_PIXELS: u64 = 300_000_000;

pub struct PsdEngine;

struct Cur<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cur<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], EngineError> {
        let end = self
            .i
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| EngineError::Decode("this PSD is truncated".into()))?;
        let s = &self.b[self.i..end];
        self.i = end;
        Ok(s)
    }
    fn u16(&mut self) -> Result<u16, EngineError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, EngineError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn u64(&mut self) -> Result<u64, EngineError> {
        let s = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(s);
        Ok(u64::from_be_bytes(a))
    }
    fn skip_section(&mut self, long: bool) -> Result<(), EngineError> {
        let n = if long {
            self.u64()?
        } else {
            self.u32()? as u64
        };
        self.take(usize::try_from(n).map_err(|_| EngineError::Decode("bad PSD section".into()))?)?;
        Ok(())
    }
}

/// PackBits: n ≥ 0 → copy n+1 bytes; n < 0 (≠ −128) → repeat next byte 1−n times.
fn packbits(src: &[u8], out: &mut Vec<u8>, want: usize) -> Result<(), EngineError> {
    let start = out.len();
    let mut i = 0;
    while i < src.len() && out.len() - start < want {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = n as usize + 1;
            let end = (i + len).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        } else if n != -128 {
            let len = (1 - n as i32) as usize;
            let v = *src
                .get(i)
                .ok_or_else(|| EngineError::Decode("bad RLE data in this PSD".into()))?;
            i += 1;
            out.extend(std::iter::repeat_n(v, len));
        }
    }
    out.resize(start + want, 0);
    Ok(())
}

/// Decode the composite to straight-alpha RGBA8.
pub fn decode(bytes: &[u8]) -> Result<image::RgbaImage, EngineError> {
    let mut c = Cur { b: bytes, i: 0 };
    if c.take(4)? != b"8BPS" {
        return Err(EngineError::Decode("not a Photoshop file".into()));
    }
    let version = c.u16()?;
    let long = match version {
        1 => false,
        2 => true,
        _ => return Err(EngineError::Decode("unknown PSD version".into())),
    };
    c.take(6)?;
    let channels = c.u16()? as usize;
    let h = c.u32()? as usize;
    let w = c.u32()? as usize;
    let depth = c.u16()?;
    let mode = c.u16()?;
    if w == 0 || h == 0 || channels == 0 || channels > 56 {
        return Err(EngineError::Decode("bad PSD header".into()));
    }
    if (w as u64) * (h as u64) > MAX_PIXELS {
        return Err(EngineError::Invalid(format!(
            "{w}×{h} is too large to convert"
        )));
    }
    if !matches!(depth, 1 | 8 | 16) {
        return Err(EngineError::Invalid(format!(
            "{depth}-bit Photoshop files aren't supported yet"
        )));
    }
    let palette = {
        let n = c.u32()? as usize;
        c.take(n)?.to_vec()
    };
    c.skip_section(false)?; // image resources (always a 4-byte length)
    c.skip_section(long)?; // layers and masks
    let compression = c.u16()?;
    let row_bytes = match depth {
        1 => w.div_ceil(8),
        8 => w,
        _ => w * 2,
    };
    let plane = row_bytes * h;
    let base = match mode {
        0 | 1 | 2 | 8 => 1,
        3 => 3,
        4 => 4,
        9 => {
            return Err(EngineError::Invalid(
                "Lab Photoshop files aren't supported yet".into(),
            ));
        }
        _ => channels.min(3),
    };
    let alpha = channels > base && matches!(mode, 1 | 3 | 4 | 8);
    let used = (base + usize::from(alpha)).min(channels);
    // Planar data for the channels we use.
    let mut data = Vec::with_capacity(plane * used);
    match compression {
        0 => data.extend_from_slice(c.take(plane * used)?),
        1 => {
            let count_size = if long { 4 } else { 2 };
            let counts_raw = c.take(channels * h * count_size)?;
            let counts: Vec<usize> = counts_raw
                .chunks_exact(count_size)
                .map(|b| {
                    if long {
                        u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
                    } else {
                        u16::from_be_bytes([b[0], b[1]]) as usize
                    }
                })
                .collect();
            for ch in 0..used {
                for row in 0..h {
                    let n = counts[ch * h + row];
                    packbits(c.take(n)?, &mut data, row_bytes)?;
                }
            }
        }
        2 | 3 => {
            let rest = &bytes[c.i..];
            let mut z = Vec::new();
            flate2::read::ZlibDecoder::new(rest)
                .take((plane * channels) as u64 + 1)
                .read_to_end(&mut z)
                .map_err(EngineError::decode)?;
            if z.len() < plane * used {
                return Err(EngineError::Decode(
                    "this PSD's image data is truncated".into(),
                ));
            }
            if compression == 3 {
                // Prediction: each sample is a delta from its left neighbour, per row.
                for row in z.chunks_exact_mut(row_bytes).take(h * used) {
                    if depth == 16 {
                        let mut prev = 0u16;
                        for px in row.as_chunks_mut::<2>().0 {
                            let v = prev.wrapping_add(u16::from_be_bytes(*px));
                            *px = v.to_be_bytes();
                            prev = v;
                        }
                    } else {
                        for i in 1..row.len() {
                            row[i] = row[i].wrapping_add(row[i - 1]);
                        }
                    }
                }
            }
            data.extend_from_slice(&z[..plane * used]);
        }
        _ => return Err(EngineError::Decode("unknown PSD compression".into())),
    }
    let sample = |ch: usize, x: usize, y: usize| -> u8 {
        let p = &data[ch * plane + y * row_bytes..];
        match depth {
            1 => {
                // 1 = black in Bitmap mode.
                if p[x / 8] & (0x80 >> (x % 8)) != 0 {
                    0
                } else {
                    255
                }
            }
            8 => p[x],
            _ => p[x * 2],
        }
    };
    let mut img = image::RgbaImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = match mode {
                3 => (sample(0, x, y), sample(1, x, y), sample(2, x, y)),
                4 => {
                    // PSD stores CMYK inverted (255 = no ink).
                    let k = sample(3, x, y) as u32;
                    let f = |v: u8| ((v as u32 * k + 127) / 255) as u8;
                    (f(sample(0, x, y)), f(sample(1, x, y)), f(sample(2, x, y)))
                }
                2 => {
                    let i = sample(0, x, y) as usize;
                    let at = |o: usize| palette.get(o * 256 + i).copied().unwrap_or(0);
                    (at(0), at(1), at(2))
                }
                _ => {
                    let v = sample(0, x, y);
                    (v, v, v)
                }
            };
            let a = if alpha { sample(base, x, y) } else { 255 };
            // Un-matte from white: c = c' − (1 − a)·255, rescaled by a.
            let un = |c: u8| -> u8 {
                if a == 255 || a == 0 {
                    c
                } else {
                    let v = (c as i32 - (255 - a as i32)) * 255 / a as i32;
                    v.clamp(0, 255) as u8
                }
            };
            img.put_pixel(x as u32, y as u32, image::Rgba([un(r), un(g), un(b), a]));
        }
    }
    Ok(img)
}

impl Engine for PsdEngine {
    fn id(&self) -> &'static str {
        "psd"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("psd", "psd", "png")
                .loss(3)
                .time(80, 60)
                .note("flattened image; layers aren't kept"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let bytes = std::fs::read(req.input)?;
        let img = decode(&bytes)?;
        req.cancel.check()?;
        image::DynamicImage::ImageRgba8(img)
            .save_with_format(req.output, image::ImageFormat::Png)
            .map_err(EngineError::encode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal RGB+alpha PSD with RLE composite data.
    fn psd(w: u32, h: u32, rle: bool) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"8BPS");
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&[0; 6]);
        v.extend_from_slice(&4u16.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&8u16.to_be_bytes());
        v.extend_from_slice(&3u16.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // color mode data
        v.extend_from_slice(&0u32.to_be_bytes()); // resources
        v.extend_from_slice(&0u32.to_be_bytes()); // layers
        // Channels: R=200, G=100, B=50 (matted on white where alpha < 255), A.
        let alpha = |x: u32| if x == 0 { 128u8 } else { 255 };
        let matte = |c: u8, a: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        let planes: Vec<Vec<u8>> = vec![
            (0..w).map(|x| matte(200, alpha(x))).collect(),
            (0..w).map(|x| matte(100, alpha(x))).collect(),
            (0..w).map(|x| matte(50, alpha(x))).collect(),
            (0..w).map(alpha).collect(),
        ];
        if rle {
            v.extend_from_slice(&1u16.to_be_bytes());
            // Each row as one literal run.
            for _ in 0..4 * h {
                v.extend_from_slice(&((w + 1) as u16).to_be_bytes());
            }
            for p in &planes {
                for _ in 0..h {
                    v.push((w - 1) as u8);
                    v.extend_from_slice(p);
                }
            }
        } else {
            v.extend_from_slice(&0u16.to_be_bytes());
            for p in &planes {
                for _ in 0..h {
                    v.extend_from_slice(p);
                }
            }
        }
        v
    }

    #[test]
    fn decodes_rgb_with_alpha() {
        for rle in [false, true] {
            let img = decode(&psd(5, 3, rle)).unwrap();
            assert_eq!(img.dimensions(), (5, 3));
            assert_eq!(img.get_pixel(3, 1).0, [200, 100, 50, 255]);
            let p = img.get_pixel(0, 2).0;
            assert_eq!(p[3], 128);
            assert!(
                (p[0] as i32 - 200).abs() <= 2 && (p[2] as i32 - 50).abs() <= 2,
                "{p:?}"
            );
        }
        assert!(decode(b"8BPS").is_err());
        let mut t = psd(5, 3, true);
        t.truncate(60);
        assert!(decode(&t).is_err());
    }

    #[test]
    fn packbits_runs() {
        let mut out = Vec::new();
        packbits(&[2, 1, 2, 3, 0xFE, 9], &mut out, 6).unwrap();
        assert_eq!(out, [1, 2, 3, 9, 9, 9]);
    }
}
