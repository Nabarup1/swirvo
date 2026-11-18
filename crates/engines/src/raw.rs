//! Camera RAW → JPG from the camera's own embedded preview (docs/12 §8). Every common RAW
//! format (DNG, CR2, CR3, NEF, ARW, ORF, RW2, RAF, PEF…) carries a full- or near-full-size
//! JPEG rendered in-camera. We find the largest one by walking JPEG structure (not by
//! trusting offsets), skip lossless-JPEG raw data (SOF3), and apply the TIFF orientation.
//! Without rotation, the JPEG is copied byte for byte.
//!
//! For a true RAW develop, Windows' Raw Image Extension is used when present (os/wic.rs).

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

pub const FORMATS: &[&str] = &["dng", "cr2", "cr3", "nef", "arw"];

/// Previews narrower than this are thumbnails, not worth converting.
const MIN_WIDTH: u16 = 640;

pub struct RawEngine;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Embedded {
    pub start: usize,
    pub end: usize,
    pub width: u16,
    pub height: u16,
}

/// Walk one JPEG starting at `start` (an SOI). Returns its end and frame size if it's a
/// baseline/progressive JPEG we can decode.
fn walk(b: &[u8], start: usize) -> Option<Embedded> {
    let mut i = start + 2;
    let mut size = None;
    let mut decodable = false;
    loop {
        // Find the next marker.
        while *b.get(i)? != 0xFF {
            i += 1;
        }
        while *b.get(i)? == 0xFF {
            i += 1;
        }
        let m = *b.get(i)?;
        i += 1;
        match m {
            0xD9 => {
                let (width, height) = size?;
                return decodable.then_some(Embedded {
                    start,
                    end: i,
                    width,
                    height,
                });
            }
            0xD0..=0xD7 | 0x01 => continue,
            _ => {}
        }
        let len = u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as usize;
        if len < 2 {
            return None;
        }
        if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([*b.get(i + 3)?, *b.get(i + 4)?]);
            let w = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]);
            size = Some((w, h));
            decodable = matches!(m, 0xC0..=0xC2);
        }
        i += len;
        if m == 0xDA {
            // Entropy-coded data: skip to the next real marker (not FF00, not RSTn).
            loop {
                if *b.get(i)? == 0xFF {
                    let n = *b.get(i + 1)?;
                    if n == 0x00 || (0xD0..=0xD7).contains(&n) || n == 0xFF {
                        i += 2;
                        continue;
                    }
                    break;
                }
                i += 1;
            }
        }
    }
}

/// All decodable embedded JPEGs at least `MIN_WIDTH` wide, largest first.
pub fn previews(b: &[u8]) -> Vec<Embedded> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 < b.len() {
        if b[i] == 0xFF
            && b[i + 1] == 0xD8
            && b[i + 2] == 0xFF
            && let Some(e) = walk(b, i)
        {
            if e.width.max(e.height) >= MIN_WIDTH {
                out.push(e);
            }
            i = e.end;
            continue;
        }
        i += 1;
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.width as u32 * e.height as u32));
    out
}

/// EXIF orientation (1–8) from the first TIFF header in the file's first 256 KB (the RAW's
/// own IFD0 for TIFF-based formats; the CMT1 box for CR3).
pub fn orientation(b: &[u8]) -> u16 {
    let head = &b[..b.len().min(256 << 10)];
    for i in 0..head.len().saturating_sub(8) {
        let le = &head[i..i + 4] == b"II*\0";
        let be = &head[i..i + 4] == b"MM\0*";
        if !(le || be) {
            continue;
        }
        let rd16 = |o: usize| -> Option<u16> {
            let s = head.get(i + o..i + o + 2)?;
            Some(if le {
                u16::from_le_bytes([s[0], s[1]])
            } else {
                u16::from_be_bytes([s[0], s[1]])
            })
        };
        let rd32 = |o: usize| -> Option<u32> {
            let s = head.get(i + o..i + o + 4)?;
            Some(if le {
                u32::from_le_bytes([s[0], s[1], s[2], s[3]])
            } else {
                u32::from_be_bytes([s[0], s[1], s[2], s[3]])
            })
        };
        let Some(ifd) = rd32(4).map(|v| v as usize) else {
            continue;
        };
        let Some(n) = rd16(ifd) else { continue };
        for e in 0..n.min(200) as usize {
            let at = ifd + 2 + e * 12;
            if rd16(at) == Some(0x0112) {
                return rd16(at + 8).filter(|v| (1..=8).contains(v)).unwrap_or(1);
            }
        }
        return 1;
    }
    1
}

fn orient(img: image::DynamicImage, o: u16) -> image::DynamicImage {
    match o {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

impl Engine for RawEngine {
    fn id(&self) -> &'static str {
        "raw-preview"
    }

    fn capabilities(&self) -> Vec<Capability> {
        FORMATS
            .iter()
            .map(|f| {
                Capability::new("raw-preview", f, "jpg")
                    .loss(12)
                    .time(60, 15)
                    .note("the camera's embedded JPEG preview")
            })
            .collect()
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let b = std::fs::read(req.input)?;
        let list = previews(&b);
        let best = list.first().ok_or_else(|| {
            EngineError::Decode("this RAW file has no full-size preview inside".into())
        })?;
        let jpeg = &b[best.start..best.end];
        let o = orientation(&b);
        if o == 1 {
            std::fs::write(req.output, jpeg)?;
            return Ok(());
        }
        let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
            .map_err(EngineError::decode)?;
        crate::image::save_with_quality(&orient(img, o), "jpg", req.output, req.preset, 92)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 200, 30]));
        let mut v = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Jpeg)
            .unwrap();
        v
    }

    #[test]
    fn finds_the_largest_embedded_jpeg() {
        // A fake TIFF-based RAW: header with orientation 6, junk, a thumbnail, the preview.
        let mut f = Vec::new();
        f.extend_from_slice(b"II*\0");
        f.extend_from_slice(&8u32.to_le_bytes());
        f.extend_from_slice(&1u16.to_le_bytes());
        f.extend_from_slice(&0x0112u16.to_le_bytes());
        f.extend_from_slice(&3u16.to_le_bytes());
        f.extend_from_slice(&1u32.to_le_bytes());
        f.extend_from_slice(&6u16.to_le_bytes());
        f.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        f.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0x00, 0x13, 0x37]); // bogus SOI
        let thumb = jpeg(160, 120);
        f.extend_from_slice(&thumb);
        f.extend_from_slice(&[7; 333]);
        let big = jpeg(1200, 800);
        let at = f.len();
        f.extend_from_slice(&big);
        f.extend_from_slice(&[9; 100]);
        let p = previews(&f);
        assert_eq!(p.len(), 1, "the 160 px thumbnail is skipped");
        assert_eq!((p[0].width, p[0].height), (1200, 800));
        assert_eq!(&f[p[0].start..p[0].end], &big[..]);
        assert_eq!(p[0].start, at);
        assert_eq!(orientation(&f), 6);

        let d = tempfile::tempdir().unwrap();
        let (src, out) = (d.path().join("a.nef"), d.path().join("a.jpg"));
        std::fs::write(&src, &f).unwrap();
        RawEngine
            .run(&StepRequest {
                input: &src,
                output: &out,
                from: "nef",
                to: "jpg",
                preset: swirvo_core::Preset::Balanced,
                pdf: Default::default(),
                cancel: &swirvo_core::CancelToken::new(),
            })
            .unwrap();
        let o = image::open(&out).unwrap();
        assert_eq!((o.width(), o.height()), (800, 1200), "rotated upright");
    }
}
