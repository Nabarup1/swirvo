//! A minimal single-page PDF writer for images.
//!
//! * JPEG input is embedded byte-for-byte (`/DCTDecode`): no re-encode, no quality loss.
//!   EXIF rotation is applied with the page's transformation matrix instead of touching
//!   pixels, so even rotated phone photos stay lossless.
//! * Everything else is stored as Flate-compressed RGB with an optional `/SMask` for alpha.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

/// Pixels → points. 96 DPI, so the page shows at the image's natural screen size.
const PT_PER_PX: f64 = 0.75;

pub enum PdfImage<'a> {
    Jpeg {
        data: &'a [u8],
        width: u32,
        height: u32,
        components: u8,
    },
    Raw {
        rgb: &'a [u8],
        alpha: Option<&'a [u8]>,
        width: u32,
        height: u32,
    },
}

/// EXIF orientations we can express as a page matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    Normal,
    FlipH,
    Rotate180,
    FlipV,
    Rotate90,
    Rotate270,
}

fn deflate(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut z = ZlibEncoder::new(Vec::with_capacity(data.len() / 2), Compression::default());
    z.write_all(data)?;
    z.finish()
}

/// Serialize a one-page PDF containing `img`.
pub fn write_pdf(img: &PdfImage<'_>, placement: Placement) -> std::io::Result<Vec<u8>> {
    let (w, h) = match img {
        PdfImage::Jpeg { width, height, .. } | PdfImage::Raw { width, height, .. } => {
            (*width as f64, *height as f64)
        }
    };
    let (sw, sh) = (w * PT_PER_PX, h * PT_PER_PX);
    let (pw, ph) = match placement {
        Placement::Rotate90 | Placement::Rotate270 => (sh, sw),
        _ => (sw, sh),
    };
    // Maps the image's unit square onto the page (derivation in docs/05 §3 notes).
    let m = match placement {
        Placement::Normal => [sw, 0.0, 0.0, sh, 0.0, 0.0],
        Placement::FlipH => [-sw, 0.0, 0.0, sh, sw, 0.0],
        Placement::Rotate180 => [-sw, 0.0, 0.0, -sh, sw, sh],
        Placement::FlipV => [sw, 0.0, 0.0, -sh, 0.0, sh],
        Placement::Rotate90 => [0.0, -sw, sh, 0.0, 0.0, sw],
        Placement::Rotate270 => [0.0, sw, -sh, 0.0, sh, 0.0],
    };

    let mut objects: Vec<Vec<u8>> = Vec::new();
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec());
    objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im0 4 0 R >> >> /Contents 5 0 R >>").into_bytes());

    let stream = |dict: String, data: &[u8]| {
        let mut o = format!("{dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        o.extend_from_slice(data);
        o.extend_from_slice(b"\nendstream");
        o
    };
    let mut smask = None;
    match img {
        PdfImage::Jpeg {
            data,
            width,
            height,
            components,
        } => {
            let cs = match components {
                1 => "/DeviceGray",
                4 => "/DeviceCMYK /Decode [1 0 1 0 1 0 1 0]",
                _ => "/DeviceRGB",
            };
            objects.push(stream(format!("<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace {cs} /BitsPerComponent 8 /Filter /DCTDecode"), data));
        }
        PdfImage::Raw {
            rgb,
            alpha,
            width,
            height,
        } => {
            let mask_ref = if alpha.is_some() { " /SMask 6 0 R" } else { "" };
            objects.push(stream(
                format!("<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode{mask_ref}"),
                &deflate(rgb)?,
            ));
            if let Some(a) = alpha {
                smask = Some(stream(
                    format!(
                        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"
                    ),
                    &deflate(a)?,
                ));
            }
        }
    }
    let content = format!(
        "q {:.4} {:.4} {:.4} {:.4} {:.4} {:.4} cm /Im0 Do Q",
        m[0], m[1], m[2], m[3], m[4], m[5]
    );
    objects.push(stream("<<".to_string(), content.as_bytes()));
    if let Some(s) = smask {
        objects.push(s);
    }

    let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, o) in objects.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj", i + 1)?;
        out.extend_from_slice(o);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1)?;
    for off in offsets {
        writeln!(out, "{off:010} 00000 n ")?;
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )?;
    Ok(out)
}

/// Read width, height and component count from a JPEG's SOF marker without decoding.
pub fn jpeg_info(data: &[u8]) -> Option<(u32, u32, u8)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let is_sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof && i + 9 < data.len() {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((w, h, data[i + 9]));
        }
        if marker == 0xDA {
            return None; // start of scan before any SOF: malformed
        }
        i += 2 + len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xref_offsets_point_at_objects() {
        let rgb = vec![255u8; 3 * 4];
        let pdf = write_pdf(
            &PdfImage::Raw {
                rgb: &rgb,
                alpha: None,
                width: 2,
                height: 2,
            },
            Placement::Normal,
        )
        .unwrap();
        // Work on bytes: the binary comment in the header must not shift offsets.
        let tail_at = pdf.windows(10).rposition(|w| w == b"startxref\n").unwrap() + 10;
        let tail = std::str::from_utf8(&pdf[tail_at..]).unwrap();
        let xref_at: usize = tail.lines().next().unwrap().parse().unwrap();
        assert!(pdf[xref_at..].starts_with(b"xref"));
        let table = std::str::from_utf8(&pdf[xref_at..tail_at]).unwrap();
        for (i, line) in table.lines().skip(3).take(5).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            let expect = format!("{} 0 obj", i + 1);
            assert!(
                pdf[off..].starts_with(expect.as_bytes()),
                "offset {off} doesn't start object {}",
                i + 1
            );
        }
    }
}
