//! Raster images through the pure-Rust `image` crate (the Core fallback; libvips and the
//! HEIC/AVIF/JXL codecs come with the native image engine, docs/05 §3).
//!
//! Pipeline: decode → apply EXIF orientation → (flatten alpha for JPEG) → encode.

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};

use image::codecs::gif::GifEncoder;
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType as Resize;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageFormat, ImageReader, Limits, RgbaImage,
};
use swirvo_core::{Capability, Engine, EngineError, FastPath, Preset, StepRequest};

use crate::pdfw::{self, PdfImage, Placement};

pub struct ImageEngine;

const DECODE: &[&str] = &[
    "jpg", "png", "webp", "gif", "bmp", "tiff", "ico", "tga", "qoi", "pnm", "jxl",
];
/// Raster formats this engine reads (image tools accept exactly these).
pub const RASTER: &[&str] = DECODE;
/// Raster formats this engine can write back (image tools keep the source format).
pub const WRITABLE: &[&str] = &[
    "jpg", "png", "webp", "gif", "bmp", "tiff", "ico", "tga", "qoi", "pnm",
];
const ENCODE: &[&str] = &[
    "jpg", "png", "webp", "gif", "bmp", "tiff", "ico", "tga", "qoi", "pnm", "pdf",
];
/// Sources whose files can carry transparency.
const ALPHA_CAPABLE: &[&str] = &["png", "webp", "gif", "bmp", "tiff", "ico", "tga", "qoi"];
/// Decoded-pixel cap: blocks decompression bombs (docs/04 §21).
const MAX_PIXELS: u64 = 268_000_000;

fn image_format(key: &str) -> Option<ImageFormat> {
    Some(match key {
        "jpg" => ImageFormat::Jpeg,
        "png" => ImageFormat::Png,
        "webp" => ImageFormat::WebP,
        "gif" => ImageFormat::Gif,
        "bmp" => ImageFormat::Bmp,
        "tiff" => ImageFormat::Tiff,
        "ico" => ImageFormat::Ico,
        "tga" => ImageFormat::Tga,
        "qoi" => ImageFormat::Qoi,
        "pnm" => ImageFormat::Pnm,
        _ => return None,
    })
}

impl Engine for ImageEngine {
    fn id(&self) -> &'static str {
        "image"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in DECODE {
            for &to in ENCODE {
                if from == to {
                    continue;
                }
                let cap = Capability::new("image", from, to).time(15, 35);
                caps.push(match (from, to) {
                    ("jpg", "pdf") => cap
                        .time(5, 2)
                        .fast(FastPath::Passthrough)
                        .note("JPEG embedded as is, rotation via page matrix"),
                    (_, "pdf") => cap.note("lossless Flate RGB, alpha as soft mask"),
                    (_, "jpg") => cap.loss(30).note("flattens transparency onto white"),
                    (_, "gif") => cap.loss(15).note("256-color palette"),
                    (f, "pnm") if ALPHA_CAPABLE.contains(&f) => cap
                        .loss(5)
                        .note("binary PPM: flattens transparency onto white"),
                    (_, "webp") => cap.note("lossless WebP"),
                    (_, "ico") => cap.note("multi-size icon, 16–256 px"),
                    _ => cap,
                });
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        req.cancel.check()?;
        if req.from == "jpg"
            && req.to == "pdf"
            && let Some(()) = jpeg_to_pdf_passthrough(req)?
        {
            return Ok(());
        }
        let img = load(req.input, req.from)?;
        req.cancel.check()?;
        save(&img, req.to, req.output, req.preset)
    }
}

/// Decode `path` (format `key`), with EXIF orientation applied and the pixel cap enforced.
pub fn load(path: &std::path::Path, key: &str) -> Result<DynamicImage, EngineError> {
    if key == "jxl" {
        let dec = jxl_oxide::integration::JxlDecoder::new(BufReader::new(File::open(path)?))
            .map_err(EngineError::decode)?;
        let (w, h) = dec.dimensions();
        if w as u64 * h as u64 > MAX_PIXELS {
            return Err(EngineError::Invalid(format!(
                "{w}×{h} is over the {MAX_PIXELS}-pixel safety limit"
            )));
        }
        return DynamicImage::from_decoder(dec).map_err(EngineError::decode);
    }
    let fmt = image_format(key)
        .ok_or_else(|| EngineError::Invalid(format!("image engine can't read {key}")))?;
    let mut reader = ImageReader::with_format(BufReader::new(File::open(path)?), fmt);
    let mut limits = Limits::default();
    limits.max_alloc = Some(4 * MAX_PIXELS * 2);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(EngineError::decode)?;
    let (w, h) = decoder.dimensions();
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(EngineError::Invalid(format!(
            "{w}×{h} is over the {MAX_PIXELS}-pixel safety limit"
        )));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(EngineError::decode)?;
    img.apply_orientation(orientation);
    Ok(img)
}

/// Composite onto white: JPEG and opaque formats can't store alpha.
fn flatten(img: &DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.to_rgb8();
    }
    let rgba = img.to_rgba8();
    let mut out = image::RgbImage::new(rgba.width(), rgba.height());
    for (o, p) in out.pixels_mut().zip(rgba.pixels()) {
        let a = p[3] as u32;
        for c in 0..3 {
            o[c] = ((p[c] as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        }
    }
    out
}

/// 8-bit RGB or RGBA, whichever keeps the information.
fn to_8bit(img: &DynamicImage) -> DynamicImage {
    if img.color().has_alpha() {
        DynamicImage::ImageRgba8(img.to_rgba8())
    } else {
        DynamicImage::ImageRgb8(img.to_rgb8())
    }
}

/// Encode `img` as `to` into `out`. `preset` picks JPEG quality and PNG effort.
pub fn save(
    img: &DynamicImage,
    to: &str,
    out: &std::path::Path,
    preset: Preset,
) -> Result<(), EngineError> {
    save_with_quality(img, to, out, preset, preset.quality())
}

pub fn save_with_quality(
    img: &DynamicImage,
    to: &str,
    out: &std::path::Path,
    preset: Preset,
    jpeg_quality: u8,
) -> Result<(), EngineError> {
    let enc = EngineError::encode;
    let unsupported = || EngineError::Invalid(format!("image engine can't write {to}"));
    let file = File::create(out)?;
    let mut w = BufWriter::new(file);
    match to {
        "jpg" => {
            let rgb = flatten(img);
            JpegEncoder::new_with_quality(&mut w, jpeg_quality)
                .encode_image(&rgb)
                .map_err(enc)?;
        }
        "png" => {
            let compression = match preset {
                Preset::Fastest => CompressionType::Fast,
                Preset::Best | Preset::Smallest => CompressionType::Best,
                Preset::Balanced => CompressionType::Default,
            };
            // PNG keeps 16-bit depth when the source has it.
            let keep = match img {
                DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
                    DynamicImage::ImageRgba16(img.to_rgba16())
                }
                other => other.clone(),
            };
            keep.write_with_encoder(PngEncoder::new_with_quality(
                &mut w,
                compression,
                FilterType::Adaptive,
            ))
            .map_err(enc)?;
        }
        "webp" => to_8bit(img)
            .write_with_encoder(WebPEncoder::new_lossless(&mut w))
            .map_err(enc)?,
        "gif" => {
            let mut g = GifEncoder::new_with_speed(&mut w, 10);
            g.encode_frame(image::Frame::new(img.to_rgba8()))
                .map_err(enc)?;
        }
        "bmp" | "tga" | "qoi" => {
            let fmt = image_format(to).ok_or_else(unsupported)?;
            to_8bit(img).write_to(&mut w, fmt).map_err(enc)?;
        }
        "tiff" => {
            drop(w);
            let mut f = BufWriter::new(File::create(out)?);
            img.write_to(&mut f, ImageFormat::Tiff).map_err(enc)?;
            f.flush()?;
            return Ok(());
        }
        "pnm" => {
            let rgb = DynamicImage::ImageRgb8(flatten(img));
            rgb.write_with_encoder(
                PnmEncoder::new(&mut w).with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary)),
            )
            .map_err(enc)?;
        }
        "ico" => write_ico(img, &mut w)?,
        "pdf" => {
            let rgba = img.to_rgba8();
            let (wd, ht) = rgba.dimensions();
            let mut rgb = Vec::with_capacity((wd * ht * 3) as usize);
            let mut alpha = Vec::with_capacity((wd * ht) as usize);
            let mut opaque = true;
            for p in rgba.pixels() {
                rgb.extend_from_slice(&p.0[..3]);
                alpha.push(p[3]);
                opaque &= p[3] == 255;
            }
            let pdf = pdfw::write_pdf(
                &PdfImage::Raw {
                    rgb: &rgb,
                    alpha: (!opaque).then_some(&alpha[..]),
                    width: wd,
                    height: ht,
                },
                Placement::Normal,
            )?;
            w.write_all(&pdf)?;
        }
        _ => return Err(unsupported()),
    }
    w.flush()?;
    Ok(())
}

/// Seven sizes (16–256 px) no larger than the source, each stored as PNG.
fn write_ico(img: &DynamicImage, w: &mut impl Write) -> Result<(), EngineError> {
    let side = img.width().max(img.height());
    let mut frames = Vec::new();
    for size in [16u32, 24, 32, 48, 64, 128, 256] {
        if size > side && !frames.is_empty() {
            break;
        }
        let scaled = img.resize(size, size, Resize::Lanczos3).to_rgba8();
        // Center on a square transparent canvas so non-square images aren't stretched.
        let mut canvas = RgbaImage::new(size, size);
        let (x, y) = ((size - scaled.width()) / 2, (size - scaled.height()) / 2);
        image::imageops::overlay(&mut canvas, &scaled, x as i64, y as i64);
        frames.push(
            IcoFrame::as_png(canvas.as_raw(), size, size, ExtendedColorType::Rgba8)
                .map_err(EngineError::encode)?,
        );
    }
    IcoEncoder::new(w)
        .encode_images(&frames)
        .map_err(EngineError::encode)
}

/// JPEG → PDF without re-encoding. Returns `Ok(None)` when the passthrough can't express
/// the file (CMYK, or mirrored+rotated EXIF orientations), so the caller decodes instead.
fn jpeg_to_pdf_passthrough(req: &StepRequest<'_>) -> Result<Option<()>, EngineError> {
    let data = std::fs::read(req.input)?;
    let Some((w, h, components)) = pdfw::jpeg_info(&data) else {
        return Ok(None);
    };
    if !matches!(components, 1 | 3) {
        return Ok(None);
    }
    let orientation = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(&data))
        .ok()
        .and_then(|mut d| d.orientation().ok())
        .unwrap_or(Orientation::NoTransforms);
    let placement = match orientation {
        Orientation::NoTransforms => Placement::Normal,
        Orientation::FlipHorizontal => Placement::FlipH,
        Orientation::Rotate180 => Placement::Rotate180,
        Orientation::FlipVertical => Placement::FlipV,
        Orientation::Rotate90 => Placement::Rotate90,
        Orientation::Rotate270 => Placement::Rotate270,
        _ => return Ok(None),
    };
    let pdf = pdfw::write_pdf(
        &PdfImage::Jpeg {
            data: &data,
            width: w,
            height: h,
            components,
        },
        placement,
    )?;
    std::fs::write(req.output, pdf)?;
    Ok(Some(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_core::CancelToken;

    fn sample(path: &std::path::Path) {
        let mut img = RgbaImage::new(40, 30);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba([
                (x * 6) as u8,
                (y * 8) as u8,
                120,
                if x < 5 { 0 } else { 255 },
            ]);
        }
        img.save(path).unwrap();
    }

    fn run(from: &str, to: &str, input: &std::path::Path, output: &std::path::Path) {
        let cancel = CancelToken::new();
        let req = StepRequest {
            input,
            output,
            from,
            to,
            preset: Preset::Balanced,
            pdf: Default::default(),
            cancel: &cancel,
        };
        ImageEngine.run(&req).unwrap();
    }

    #[test]
    fn png_to_everything_round_trips_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s.png");
        sample(&src);
        for to in ["jpg", "webp", "gif", "bmp", "tiff", "tga", "qoi", "pnm"] {
            let out = dir.path().join(format!("o.{to}"));
            run("png", to, &src, &out);
            let back = image::open(&out).unwrap_or_else(|e| panic!("{to}: {e}"));
            assert_eq!((back.width(), back.height()), (40, 30), "{to}");
        }
    }

    #[test]
    fn alpha_dropping_targets_are_marked_lossy() {
        let caps = ImageEngine.capabilities();
        let loss = |f: &str, t: &str| {
            caps.iter()
                .find(|c| c.from == f && c.to == t)
                .map(|c| c.loss)
        };
        assert!(loss("png", "pnm") > Some(0), "PPM has no alpha channel");
        assert_eq!(loss("jpg", "pnm"), Some(0));
        assert_eq!(loss("png", "webp"), Some(0));
        assert!(loss("png", "jpg") > Some(0));
    }

    #[test]
    fn jpg_flattens_alpha_to_white() {
        let dir = tempfile::tempdir().unwrap();
        let (src, out) = (dir.path().join("s.png"), dir.path().join("o.jpg"));
        sample(&src);
        run("png", "jpg", &src, &out);
        let px = image::open(&out).unwrap().to_rgb8().get_pixel(1, 1).0;
        assert!(
            px.iter().all(|&c| c > 240),
            "transparent area should be white, got {px:?}"
        );
    }

    #[test]
    fn ico_has_multiple_sizes_and_pdf_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("s.png");
        sample(&src);
        let ico = dir.path().join("o.ico");
        run("png", "ico", &src, &ico);
        let count = u16::from_le_bytes(std::fs::read(&ico).unwrap()[4..6].try_into().unwrap());
        assert!(count >= 2, "expected several icon sizes, got {count}");

        let pdf = dir.path().join("o.pdf");
        run("png", "pdf", &src, &pdf);
        let bytes = std::fs::read(&pdf).unwrap();
        assert!(bytes.starts_with(b"%PDF-1.7") && bytes.ends_with(b"%%EOF\n"));
        assert!(
            bytes.windows(6).any(|w| w == b"/SMask"),
            "alpha should become a soft mask"
        );

        let jpg = dir.path().join("s.jpg");
        run("png", "jpg", &src, &jpg);
        let pdf2 = dir.path().join("j.pdf");
        run("jpg", "pdf", &jpg, &pdf2);
        let pdf2 = std::fs::read(&pdf2).unwrap();
        let jpeg = std::fs::read(&jpg).unwrap();
        assert!(
            pdf2.windows(jpeg.len()).any(|w| w == jpeg.as_slice()),
            "JPEG must be embedded byte-for-byte"
        );
    }
}
