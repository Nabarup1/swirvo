//! Make PDFs smaller without making them worse than needed.
//!
//! 1. **Images** are the bulk of most large PDFs. Each image's *displayed* size is measured
//!    from the page content (the transformation matrix at every `Do`), so an image is only
//!    downsampled when it really has more pixels than the target resolution needs where it
//!    appears. Photos are re-encoded as JPEG; screenshots and diagrams (few colours) stay
//!    lossless so their text stays crisp. A new encoding is kept only when it's clearly
//!    smaller.
//! 2. **Duplicates**: identical streams (the same font or logo embedded again by merging)
//!    are stored once.
//! 3. **Leftovers**: page thumbnails and editors' private data (`PieceInfo`) are dropped,
//!    unreferenced objects removed, uncompressed streams deflated, and objects packed into
//!    compressed object streams.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Cursor;
use std::path::Path;

use lopdf::content::Content;
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use swirvo_core::{CancelToken, EngineError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// Print quality: 220 dpi, JPEG 85.
    Light,
    /// Screen reading: 150 dpi, JPEG 75.
    #[default]
    Balanced,
    /// Email: 100 dpi, JPEG 60.
    Strong,
    /// Last resort for size limits: 72 dpi, JPEG 45.
    Extreme,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s.to_ascii_lowercase().as_str() {
            "light" | "print" | "high" => Level::Light,
            "balanced" | "screen" | "medium" => Level::Balanced,
            "strong" | "email" | "small" => Level::Strong,
            "extreme" | "smallest" => Level::Extreme,
            _ => return None,
        })
    }

    fn dpi(self) -> f32 {
        match self {
            Level::Light => 220.0,
            Level::Balanced => 150.0,
            Level::Strong => 100.0,
            Level::Extreme => 72.0,
        }
    }

    fn quality(self) -> u8 {
        match self {
            Level::Light => 85,
            Level::Balanced => 75,
            Level::Strong => 60,
            Level::Extreme => 45,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub before: u64,
    pub after: u64,
    pub images: usize,
    pub duplicates: usize,
}

/// Compress `input` into `out` at `level`. Returns sizes and what changed.
pub fn compress(
    input: &Path,
    out: &Path,
    level: Level,
    cancel: &CancelToken,
) -> Result<Report, EngineError> {
    let bytes = std::fs::read(input)?;
    let mut doc = super::open_bytes(&bytes, None)?;
    let mut r = Report {
        before: bytes.len() as u64,
        ..Report::default()
    };
    r.images = recompress_images(&mut doc, level, cancel)?;
    cancel.check()?;
    strip_leftovers(&mut doc);
    r.duplicates = dedupe_streams(&mut doc);
    doc.prune_objects();
    doc.compress();
    super::save(&mut doc, out, true)?;
    r.after = std::fs::metadata(out)?.len();
    Ok(r)
}

/// Compress until the file is at most `max_bytes`, trying stronger levels in turn. Returns
/// the report of the attempt written to `out` (the smallest one, even if still too big).
pub fn compress_to(
    input: &Path,
    out: &Path,
    max_bytes: u64,
    cancel: &CancelToken,
) -> Result<(Report, bool), EngineError> {
    let mut best: Option<(Report, Vec<u8>)> = None;
    for level in [Level::Balanced, Level::Strong, Level::Extreme] {
        let r = compress(input, out, level, cancel)?;
        if r.after <= max_bytes {
            return Ok((r, true));
        }
        if best.as_ref().is_none_or(|b| r.after < b.0.after) {
            best = Some((r, std::fs::read(out)?));
        }
    }
    let (r, data) = best.ok_or_else(|| EngineError::Encode("nothing written".into()))?;
    std::fs::write(out, data)?;
    Ok((r, false))
}

// ------------------------------------------------------------ images

type M = [f32; 6];

fn mul(a: M, b: M) -> M {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn matrix(o: Option<&Object>) -> M {
    match o.and_then(|o| o.as_array().ok()) {
        Some(a) if a.len() == 6 => [
            num(&a[0]),
            num(&a[1]),
            num(&a[2]),
            num(&a[3]),
            num(&a[4]),
            num(&a[5]),
        ],
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    }
}

fn dict_of<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    match super::deref(doc, o) {
        Object::Dictionary(d) => Some(d),
        Object::Stream(s) => Some(&s.dict),
        _ => None,
    }
}

/// Largest displayed size (points, width × height) of each image XObject.
fn displayed_sizes(doc: &Document) -> HashMap<ObjectId, (f32, f32)> {
    let mut sizes = HashMap::new();
    for page in super::pages(doc) {
        let Ok(content) = doc.get_page_content(page) else {
            continue;
        };
        let res = super::inherited(doc, page, b"Resources").and_then(|r| dict_of(doc, r));
        walk(
            doc,
            &content,
            res,
            [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            0,
            &mut sizes,
        );
    }
    sizes
}

fn walk(
    doc: &Document,
    content: &[u8],
    res: Option<&Dictionary>,
    base: M,
    depth: u32,
    sizes: &mut HashMap<ObjectId, (f32, f32)>,
) {
    if depth > 12 {
        return;
    }
    let Ok(ops) = Content::decode(content) else {
        return;
    };
    let xobjects = res
        .and_then(|r| r.get(b"XObject").ok())
        .and_then(|x| dict_of(doc, x));
    let mut stack = vec![base];
    for op in ops.operations {
        match op.operator.as_str() {
            "q" => {
                let top = *stack.last().unwrap_or(&base);
                stack.push(top);
            }
            "Q" => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            "cm" if op.operands.len() == 6 => {
                let m: M = std::array::from_fn(|i| num(&op.operands[i]));
                if let Some(top) = stack.last_mut() {
                    *top = mul(m, *top);
                }
            }
            "Do" => {
                let ctm = *stack.last().unwrap_or(&base);
                let Some(name) = op.operands.first().and_then(|o| o.as_name().ok()) else {
                    continue;
                };
                let Some(Object::Reference(id)) = xobjects.and_then(|x| x.get(name).ok()) else {
                    continue;
                };
                let Ok(Object::Stream(st)) = doc.get_object(*id) else {
                    continue;
                };
                match st.dict.get(b"Subtype").and_then(Object::as_name) {
                    Ok(b"Image") => {
                        let w = (ctm[0] * ctm[0] + ctm[1] * ctm[1]).sqrt();
                        let h = (ctm[2] * ctm[2] + ctm[3] * ctm[3]).sqrt();
                        let e = sizes.entry(*id).or_insert((0.0, 0.0));
                        e.0 = e.0.max(w);
                        e.1 = e.1.max(h);
                    }
                    Ok(b"Form") => {
                        let m = mul(matrix(st.dict.get(b"Matrix").ok()), ctm);
                        let fres = st
                            .dict
                            .get(b"Resources")
                            .ok()
                            .and_then(|r| dict_of(doc, r))
                            .or(res);
                        if let Ok(c) = st.decompressed_content() {
                            walk(doc, &c, fres, m, depth + 1, sizes);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// Colour channels of an image we can safely re-encode (Gray or RGB, 8 bits, no masking
/// tricks), or None.
fn channels(doc: &Document, d: &Dictionary) -> Option<u8> {
    if d.get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
        || d.has(b"Mask")
        || d.has(b"Decode")
        || d.get(b"BitsPerComponent").and_then(Object::as_i64).ok()? != 8
    {
        return None;
    }
    let cs = super::deref(doc, d.get(b"ColorSpace").ok()?);
    match cs {
        Object::Name(n) if n == b"DeviceRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" => Some(1),
        Object::Array(a) if a.first().and_then(|x| x.as_name().ok()) == Some(b"ICCBased") => {
            let icc = a.get(1).and_then(|x| x.as_reference().ok())?;
            let n = doc
                .get_object(icc)
                .ok()?
                .as_stream()
                .ok()?
                .dict
                .get(b"N")
                .and_then(Object::as_i64)
                .ok()?;
            match n {
                1 => Some(1),
                3 => Some(3),
                _ => None,
            }
        }
        _ => None,
    }
}

fn filters(st: &Stream) -> Vec<Vec<u8>> {
    st.filters()
        .map(|v| v.into_iter().map(<[u8]>::to_vec).collect())
        .unwrap_or_default()
}

/// Decode an image stream to pixels, when it's one of the simple kinds.
fn decode(st: &Stream, ch: u8) -> Option<image::DynamicImage> {
    let w = st.dict.get(b"Width").and_then(Object::as_i64).ok()? as u32;
    let h = st.dict.get(b"Height").and_then(Object::as_i64).ok()? as u32;
    if w == 0 || h == 0 || (w as u64) * (h as u64) > 80_000_000 {
        return None;
    }
    let f = filters(st);
    let img = if f == [b"DCTDecode".to_vec()] {
        let d = image::load_from_memory_with_format(&st.content, image::ImageFormat::Jpeg).ok()?;
        if (d.width(), d.height()) != (w, h) {
            return None;
        }
        d
    } else if f.iter().all(|x| x == b"FlateDecode" || x == b"LZWDecode") {
        let raw = st.decompressed_content().ok()?;
        let need = (w * h * ch as u32) as usize;
        if raw.len() < need {
            return None;
        }
        let raw = raw[..need].to_vec();
        if ch == 3 {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, raw)?)
        } else {
            image::DynamicImage::ImageLuma8(image::GrayImage::from_raw(w, h, raw)?)
        }
    } else {
        return None;
    };
    Some(if ch == 1 {
        image::DynamicImage::ImageLuma8(img.to_luma8())
    } else {
        image::DynamicImage::ImageRgb8(img.to_rgb8())
    })
}

/// Photos have many colours; screenshots, charts and scans of text have few.
pub(crate) fn photographic(img: &image::DynamicImage) -> bool {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let step = ((w as u64 * h as u64 / 20_000).max(1) as f64).sqrt().ceil() as u32;
    let mut seen = HashSet::new();
    for y in (0..h).step_by(step as usize) {
        for x in (0..w).step_by(step as usize) {
            let p = rgb.get_pixel(x, y);
            // Quantise a little so sensor noise on flat areas doesn't count.
            seen.insert((p[0] >> 2, p[1] >> 2, p[2] >> 2));
            if seen.len() > 2_000 {
                return true;
            }
        }
    }
    false
}

fn recompress_images(
    doc: &mut Document,
    level: Level,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let shown = displayed_sizes(doc);
    let ids: Vec<ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| match o {
            Object::Stream(s)
                if s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image") =>
            {
                Some(*id)
            }
            _ => None,
        })
        .collect();
    let mut changed = 0;
    for id in ids {
        cancel.check()?;
        let Ok(Object::Stream(st)) = doc.get_object(id) else {
            continue;
        };
        let Some(ch) = channels(doc, &st.dict) else {
            continue;
        };
        let was_jpeg = filters(st) == [b"DCTDecode".to_vec()];
        let old_len = st.content.len();
        // Small images (icons, bullets) aren't worth it and suffer most from JPEG.
        if old_len < 16 * 1024 {
            continue;
        }
        let Some(img) = decode(st, ch) else { continue };
        let (w, h) = (img.width(), img.height());
        // Target pixel size from where it's shown; unknown placement keeps the size.
        let scale = shown.get(&id).map_or(1.0, |&(sw, sh)| {
            let dpi = (w as f32 / (sw / 72.0).max(0.01)).min(h as f32 / (sh / 72.0).max(0.01));
            if dpi > level.dpi() * 1.3 {
                level.dpi() / dpi
            } else {
                1.0
            }
        });
        let img = if scale < 1.0 {
            let nw = ((w as f32 * scale).round() as u32).max(1);
            let nh = ((h as f32 * scale).round() as u32).max(1);
            img.resize_exact(nw, nh, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        let jpeg = was_jpeg || photographic(&img);
        if scale >= 1.0 && !jpeg {
            continue; // Lossless and already the right size: deflate handles it later.
        }
        let mut dict = st.dict.clone();
        let data = if jpeg {
            let mut buf = Vec::new();
            let mut cur = Cursor::new(&mut buf);
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cur, level.quality());
            img.write_with_encoder(enc).map_err(EngineError::encode)?;
            dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            buf
        } else {
            let raw = if ch == 3 {
                img.to_rgb8().into_raw()
            } else {
                img.to_luma8().into_raw()
            };
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
            std::io::Write::write_all(&mut z, &raw)?;
            dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            z.finish()?
        };
        if data.len() as f32 > old_len as f32 * 0.9 {
            continue;
        }
        dict.remove(b"DecodeParms");
        dict.set("Width", Object::Integer(img.width() as i64));
        dict.set("Height", Object::Integer(img.height() as i64));
        dict.set("BitsPerComponent", Object::Integer(8));
        dict.set("Length", Object::Integer(data.len() as i64));
        let mut s = Stream::new(dict, data);
        s.allows_compression = false;
        doc.objects.insert(id, Object::Stream(s));
        changed += 1;
    }
    Ok(changed)
}

// ------------------------------------------------------------ the rest

/// Page thumbnails (viewers draw their own) and editors' private data.
fn strip_leftovers(doc: &mut Document) {
    for page in super::pages(doc) {
        if let Ok(d) = doc.get_dictionary_mut(page) {
            d.remove(b"Thumb");
            d.remove(b"PieceInfo");
        }
    }
    if let Ok(c) = doc.catalog_mut() {
        c.remove(b"PieceInfo");
    }
}

/// Store identical streams once. Returns how many copies were dropped.
fn dedupe_streams(doc: &mut Document) -> usize {
    let mut first: HashMap<(u64, usize), Vec<ObjectId>> = HashMap::new();
    let mut map: HashMap<ObjectId, ObjectId> = HashMap::new();
    for (id, o) in &doc.objects {
        let Object::Stream(s) = o else { continue };
        if s.content.len() < 64 {
            continue;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.content.hash(&mut h);
        let mut keys: Vec<String> = s
            .dict
            .iter()
            .filter(|(k, _)| k.as_slice() != b"Length")
            .map(|(k, v)| format!("{}={v:?}", String::from_utf8_lossy(k)))
            .collect();
        keys.sort();
        keys.hash(&mut h);
        let bucket = first.entry((h.finish(), s.content.len())).or_default();
        // Confirm byte equality against the bucket's members (hash collisions are possible).
        let same = bucket.iter().find(|other| match doc.objects.get(other) {
            Some(Object::Stream(o)) => o.content == s.content && o.dict == s.dict,
            _ => false,
        });
        match same {
            Some(&keep) => {
                map.insert(*id, keep);
            }
            None => bucket.push(*id),
        }
    }
    if map.is_empty() {
        return 0;
    }
    fn fix(o: &mut Object, map: &HashMap<ObjectId, ObjectId>) {
        match o {
            Object::Reference(r) => {
                if let Some(n) = map.get(r) {
                    *r = *n;
                }
            }
            Object::Array(a) => a.iter_mut().for_each(|x| fix(x, map)),
            Object::Dictionary(d) => d.iter_mut().for_each(|(_, v)| fix(v, map)),
            Object::Stream(s) => s.dict.iter_mut().for_each(|(_, v)| fix(v, map)),
            _ => {}
        }
    }
    for o in doc.objects.values_mut() {
        fix(o, &map);
    }
    doc.trailer.iter_mut().for_each(|(_, v)| fix(v, &map));
    for id in map.keys() {
        doc.objects.remove(id);
    }
    map.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PDF with a big noisy photo drawn small (twice: once per page, embedded twice).
    fn photo_pdf(path: &Path) {
        let mut img = image::RgbImage::new(2400, 1600);
        let mut seed = 12345u32;
        for (x, y, p) in img.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let n = (seed >> 24) as u8 / 8;
            *p = image::Rgb([
                ((x / 10) as u8).wrapping_add(n),
                ((y / 7) as u8).wrapping_add(n),
                (((x + y) / 13) as u8).wrapping_add(n),
            ]);
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut doc = krilla::Document::new();
        for _ in 0..2 {
            let mut page =
                doc.start_page_with(krilla::page::PageSettings::from_wh(595.0, 842.0).unwrap());
            let mut s = page.surface();
            let im = krilla::image::Image::from_png(png.clone().into(), true).unwrap();
            // 300 × 200 pt: the 2400 px image is shown at 576 dpi.
            s.draw_image(im, krilla::geom::Size::from_wh(300.0, 200.0).unwrap());
            s.finish();
            page.finish();
        }
        std::fs::write(path, doc.finish().unwrap()).unwrap();
    }

    #[test]
    fn shrinks_oversized_photos_and_keeps_pages() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("p.pdf");
        photo_pdf(&src);
        let out = d.path().join("c.pdf");
        let r = compress(&src, &out, Level::Balanced, &CancelToken::new()).unwrap();
        assert!(r.images >= 1, "{r:?}");
        assert!(r.after * 4 < r.before, "{r:?}");
        let doc = lopdf::Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        // 300 pt at 150 dpi ≈ 625 px wide.
        let widths: Vec<i64> = doc
            .objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Image"))
            .filter_map(|s| s.dict.get(b"Width").and_then(Object::as_i64).ok())
            .collect();
        assert!(
            widths.iter().all(|&w| (600..=650).contains(&w)),
            "{widths:?}"
        );
        // It still renders.
        let pr = crate::pdf_tools::PageRenderer::open(&out).unwrap();
        let img = pr.session(|s| s.render_dpi(0, 36.0)).unwrap();
        assert!(img.pixels().any(|p| p[0] < 200));

        let capped = d.path().join("t.pdf");
        let (r2, ok) = compress_to(&src, &capped, r.after / 2, &CancelToken::new()).unwrap();
        assert!(ok, "{r2:?}");
    }

    #[test]
    fn matrices_compose_like_pdf() {
        let s = [2.0, 0.0, 0.0, 3.0, 0.0, 0.0];
        let t = [1.0, 0.0, 0.0, 1.0, 10.0, 20.0];
        // cm s then cm t: points map through s first.
        assert_eq!(mul(s, t), [2.0, 0.0, 0.0, 3.0, 10.0, 20.0]);
    }
}
