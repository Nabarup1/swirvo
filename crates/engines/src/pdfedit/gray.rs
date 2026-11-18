//! Grayscale: the same PDF with every colour turned to gray, for black-and-white printing
//! and smaller files. Text stays text and vector art stays vector: the colours are
//! rewritten in the page instructions (fills, strokes, colour spaces), the pictures are
//! re-encoded as gray, gradients built from simple functions are converted, and comment
//! colours follow. Spot colours (Separation, DeviceN) and mesh gradients keep theirs.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use swirvo_core::{CancelToken, EngineError};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Grayed {
    pub pictures: usize,
    pub gradients: usize,
    /// Pictures or gradients left in colour (formats this can't rewrite).
    pub kept: usize,
}

/// How a colour space's values become gray.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Conv {
    Gray,
    Rgb,
    Cmyk,
    /// Anything else (patterns, spot colours, Lab, indexed): left alone.
    Keep,
}

fn rgb_gray(r: f32, g: f32, b: f32) -> f32 {
    (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 1.0)
}

fn cmyk_gray(c: f32, m: f32, y: f32, k: f32) -> f32 {
    (1.0 - (0.3 * c + 0.59 * m + 0.11 * y + k)).clamp(0.0, 1.0)
}

fn num(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

fn nums(ops: &[Object]) -> Option<Vec<f32>> {
    ops.iter().map(num).collect()
}

fn real(v: f32) -> Object {
    Object::Real((v * 10_000.0).round() / 10_000.0)
}

/// Gray from `n` component values in space `c`.
fn to_gray(c: Conv, v: &[f32]) -> Option<f32> {
    match (c, v) {
        (Conv::Rgb, [r, g, b]) => Some(rgb_gray(*r, *g, *b)),
        (Conv::Cmyk, [c, m, y, k]) => Some(cmyk_gray(*c, *m, *y, *k)),
        (Conv::Gray, [g]) => Some(*g),
        _ => None,
    }
}

/// What a colour space object is.
fn classify(doc: &Document, cs: &Object) -> Conv {
    match super::deref(doc, cs) {
        Object::Name(n) => match n.as_slice() {
            b"DeviceRGB" | b"RGB" => Conv::Rgb,
            b"DeviceCMYK" | b"CMYK" => Conv::Cmyk,
            b"DeviceGray" | b"G" => Conv::Gray,
            _ => Conv::Keep,
        },
        Object::Array(a) => match a.first().and_then(|x| x.as_name().ok()) {
            Some(b"CalRGB") => Conv::Rgb,
            Some(b"CalGray") => Conv::Gray,
            Some(b"ICCBased") => {
                let n = a
                    .get(1)
                    .map(|s| super::deref(doc, s))
                    .and_then(|s| s.as_stream().ok())
                    .and_then(|s| s.dict.get(b"N").and_then(Object::as_i64).ok());
                match n {
                    Some(1) => Conv::Gray,
                    Some(3) => Conv::Rgb,
                    Some(4) => Conv::Cmyk,
                    _ => Conv::Keep,
                }
            }
            _ => Conv::Keep,
        },
        _ => Conv::Keep,
    }
}

/// Turn every colour in `input` to gray, into `out`.
pub fn grayscale(input: &Path, out: &Path, cancel: &CancelToken) -> Result<Grayed, EngineError> {
    let mut doc = super::open(input, None)?;
    let mut rep = Grayed::default();

    // 1. Content streams with the resources they're drawn with (before any resource
    // changes: the colour-space names must still say what they were).
    let mut jobs: Vec<(ObjectId, Option<Dictionary>)> = Vec::new();
    let mut seen = BTreeSet::new();
    for page in super::pages(&doc) {
        let res = super::inherited(&doc, page, b"Resources")
            .map(|r| super::deref(&doc, r))
            .and_then(|r| r.as_dict().ok())
            .cloned();
        for id in doc.get_page_contents(page) {
            if seen.insert(id) {
                jobs.push((id, res.clone()));
            }
        }
    }
    for (id, o) in &doc.objects {
        if let Object::Stream(st) = o {
            let form = st.dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok())
                == Some(&b"Form"[..])
                || st.dict.get(b"PatternType").and_then(Object::as_i64).ok() == Some(1)
                || st.dict.get(b"CharProcs").is_ok();
            if form && seen.insert(*id) {
                let res = st
                    .dict
                    .get(b"Resources")
                    .ok()
                    .map(|r| super::deref(&doc, r))
                    .and_then(|r| r.as_dict().ok())
                    .cloned();
                jobs.push((*id, res));
            }
        }
    }
    let mut rewritten: HashMap<ObjectId, Vec<u8>> = HashMap::new();
    for (id, res) in &jobs {
        cancel.check()?;
        let Ok(Object::Stream(st)) = doc.get_object(*id) else {
            continue;
        };
        let raw = st
            .decompressed_content()
            .unwrap_or_else(|_| st.content.clone());
        let Ok(mut c) = Content::decode(&raw) else {
            continue;
        };
        if convert_ops(&doc, res.as_ref(), &mut c.operations)
            && let Ok(bytes) = c.encode()
        {
            rewritten.insert(*id, bytes);
        }
    }
    for (id, bytes) in rewritten {
        if let Ok(Object::Stream(st)) = doc.get_object_mut(id) {
            st.set_plain_content(bytes);
            let _ = st.compress();
        }
    }

    // 2. Pictures, gradients, colour-space resources and comment colours.
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    let mut functions_done = BTreeSet::new();
    for id in &ids {
        cancel.check()?;
        let Some(obj) = doc.objects.get(id) else {
            continue;
        };
        match obj {
            Object::Stream(st)
                if st.dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok())
                    == Some(&b"Image"[..]) =>
            {
                match gray_image(&doc, st) {
                    Some(Some(new)) => {
                        doc.objects.insert(*id, Object::Stream(new));
                        rep.pictures += 1;
                    }
                    Some(None) => {}
                    None => rep.kept += 1,
                }
            }
            _ => {}
        }
        // Shadings (as objects of their own, or inline in a pattern's dictionary).
        if let Some(sh) = shading_of(&doc, *id) {
            match gray_shading(&mut doc, sh, &mut functions_done) {
                Some(true) => rep.gradients += 1,
                Some(false) => {}
                None => rep.kept += 1,
            }
        }
    }
    for id in &ids {
        // Colour-space resource entries that name RGB/CMYK spaces become DeviceGray.
        let Some(Object::Dictionary(d)) = doc.objects.get(id).cloned() else {
            continue;
        };
        if let Ok(cs) = d.get(b"ColorSpace")
            && let Object::Dictionary(map) = super::deref(&doc, cs).clone()
        {
            let mut map2 = map.clone();
            let mut changed = false;
            for (k, v) in map.iter() {
                if matches!(classify(&doc, v), Conv::Rgb | Conv::Cmyk) {
                    map2.set(k.clone(), Object::Name(b"DeviceGray".to_vec()));
                    changed = true;
                }
            }
            if changed {
                set_in(&mut doc, *id, b"ColorSpace", Object::Dictionary(map2));
            }
        }
        // Annotation colours.
        for key in [&b"C"[..], b"IC"] {
            if let Ok(Object::Array(a)) = d.get(key)
                && let Some(v) = nums(a)
            {
                let c = match v.len() {
                    3 => Conv::Rgb,
                    4 => Conv::Cmyk,
                    _ => continue,
                };
                if let Some(g) = to_gray(c, &v) {
                    set_in(&mut doc, *id, key, Object::Array(vec![real(g)]));
                }
            }
        }
    }
    doc.prune_objects();
    super::save(&mut doc, out, false)?;
    Ok(rep)
}

/// Set `key` on the dictionary object `id` (or a stream's dictionary).
fn set_in(doc: &mut Document, id: ObjectId, key: &[u8], v: Object) {
    match doc.objects.get_mut(&id) {
        Some(Object::Dictionary(d)) => d.set(key.to_vec(), v),
        Some(Object::Stream(s)) => s.dict.set(key.to_vec(), v),
        _ => {}
    }
}

/// Rewrite colour operators; true if anything changed.
fn convert_ops(doc: &Document, res: Option<&Dictionary>, ops: &mut [Operation]) -> bool {
    let named = |n: &[u8]| -> Conv {
        match n {
            b"DeviceRGB" => Conv::Rgb,
            b"DeviceCMYK" => Conv::Cmyk,
            b"DeviceGray" => Conv::Gray,
            _ => res
                .and_then(|r| r.get(b"ColorSpace").ok())
                .map(|m| super::deref(doc, m))
                .and_then(|m| m.as_dict().ok())
                .and_then(|m| m.get(n).ok())
                .map_or(Conv::Keep, |cs| classify(doc, cs)),
        }
    };
    let mut changed = false;
    // (fill, stroke) colour space, saved by q and restored by Q.
    let mut cur = (Conv::Gray, Conv::Gray);
    let mut stack = Vec::new();
    for op in ops.iter_mut() {
        let stroke = op
            .operator
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        match op.operator.as_str() {
            "q" => stack.push(cur),
            "Q" => cur = stack.pop().unwrap_or((Conv::Gray, Conv::Gray)),
            "rg" | "RG" | "k" | "K" => {
                let c = if op.operator.eq_ignore_ascii_case("rg") {
                    Conv::Rgb
                } else {
                    Conv::Cmyk
                };
                if let Some(g) = nums(&op.operands).and_then(|v| to_gray(c, &v)) {
                    op.operator = if stroke { "G" } else { "g" }.into();
                    op.operands = vec![real(g)];
                    changed = true;
                }
                if stroke {
                    cur.1 = Conv::Gray;
                } else {
                    cur.0 = Conv::Gray;
                }
            }
            "g" | "G" => {
                if stroke {
                    cur.1 = Conv::Gray;
                } else {
                    cur.0 = Conv::Gray;
                }
            }
            "cs" | "CS" => {
                let c = op
                    .operands
                    .first()
                    .and_then(|o| o.as_name().ok())
                    .map_or(Conv::Keep, named);
                if matches!(c, Conv::Rgb | Conv::Cmyk) {
                    op.operands = vec![Object::Name(b"DeviceGray".to_vec())];
                    changed = true;
                }
                if stroke {
                    cur.1 = c;
                } else {
                    cur.0 = c;
                }
            }
            "sc" | "scn" | "SC" | "SCN" => {
                let c = if stroke { cur.1 } else { cur.0 };
                if matches!(c, Conv::Rgb | Conv::Cmyk)
                    && let Some(g) = nums(&op.operands).and_then(|v| to_gray(c, &v))
                {
                    op.operands = vec![real(g)];
                    changed = true;
                }
            }
            _ => {}
        }
    }
    changed
}

/// A gray copy of a picture: Some(Some) rewritten, Some(None) already gray or a mask,
/// None when it can't be decoded (left in colour).
fn gray_image(doc: &Document, st: &Stream) -> Option<Option<Stream>> {
    let d = &st.dict;
    if d.get(b"ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false)
    {
        return Some(None);
    }
    let cs = d.get(b"ColorSpace").ok()?;
    if classify(doc, cs) == Conv::Gray {
        return Some(None);
    }
    let img = super::clean::decode_image(doc, st)?;
    let luma = img.to_luma8();
    let (w, h) = luma.dimensions();
    let jpeg = d
        .get(b"Filter")
        .ok()
        .map(|f| match f {
            Object::Name(n) => n == b"DCTDecode",
            Object::Array(a) => a.iter().any(|x| x.as_name().ok() == Some(b"DCTDecode")),
            _ => false,
        })
        .unwrap_or(false);
    let mut nd = Dictionary::new();
    for (k, v) in d.iter() {
        if !matches!(
            k.as_slice(),
            b"ColorSpace"
                | b"BitsPerComponent"
                | b"Filter"
                | b"DecodeParms"
                | b"Decode"
                | b"Length"
        ) && !(k == b"Mask" && matches!(v, Object::Array(_)))
        {
            nd.set(k.clone(), v.clone());
        }
    }
    nd.set("Width", Object::Integer(w as i64));
    nd.set("Height", Object::Integer(h as i64));
    nd.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
    nd.set("BitsPerComponent", Object::Integer(8));
    if jpeg {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
            .encode_image(&luma)
            .ok()?;
        nd.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        Some(Some(Stream::new(nd, out)))
    } else {
        let mut s = Stream::new(nd, luma.into_raw());
        let _ = s.compress();
        Some(Some(s))
    }
}

/// If object `id` is (or holds, as a shading pattern) a shading dictionary: its location.
#[derive(Clone, Copy)]
enum ShadingAt {
    Object(ObjectId),
    InPattern(ObjectId),
}

fn shading_of(doc: &Document, id: ObjectId) -> Option<ShadingAt> {
    let d = match doc.objects.get(&id)? {
        Object::Dictionary(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return None,
    };
    if d.get(b"ShadingType").is_ok() {
        return Some(ShadingAt::Object(id));
    }
    if d.get(b"PatternType").and_then(Object::as_i64).ok() == Some(2)
        && matches!(d.get(b"Shading"), Ok(Object::Dictionary(_)))
    {
        return Some(ShadingAt::InPattern(id));
    }
    None
}

/// Convert a shading's colour space and function to gray. Some(true) converted,
/// Some(false) already gray, None left in colour.
fn gray_shading(doc: &mut Document, at: ShadingAt, done: &mut BTreeSet<ObjectId>) -> Option<bool> {
    let sh: Dictionary = match at {
        ShadingAt::Object(id) => match doc.objects.get(&id)? {
            Object::Dictionary(d) => d.clone(),
            Object::Stream(s) => s.dict.clone(),
            _ => return None,
        },
        ShadingAt::InPattern(id) => doc
            .get_dictionary(id)
            .ok()?
            .get(b"Shading")
            .ok()?
            .as_dict()
            .ok()?
            .clone(),
    };
    let c = classify(doc, sh.get(b"ColorSpace").ok()?);
    match c {
        Conv::Gray => return Some(false),
        Conv::Keep => return None,
        _ => {}
    }
    let kind = sh.get(b"ShadingType").and_then(Object::as_i64).ok()?;
    if !(1..=3).contains(&kind) {
        return None; // meshes carry colours in their data
    }
    let f = sh.get(b"Function").ok()?.clone();
    let f = gray_function(doc, &f, c, done)?;
    let mut new = sh.clone();
    new.set("Function", f);
    new.set("ColorSpace", Object::Name(b"DeviceGray".to_vec()));
    if let Ok(Object::Array(bg)) = sh.get(b"Background")
        && let Some(g) = nums(bg).and_then(|v| to_gray(c, &v))
    {
        new.set("Background", Object::Array(vec![real(g)]));
    }
    match at {
        ShadingAt::Object(id) => match doc.objects.get_mut(&id)? {
            Object::Dictionary(d) => *d = new,
            Object::Stream(s) => s.dict = new,
            _ => return None,
        },
        ShadingAt::InPattern(id) => {
            set_in(doc, id, b"Shading", Object::Dictionary(new));
        }
    }
    Some(true)
}

/// A gray version of a type 2 (exponential) or type 3 (stitching) function, converted in
/// place when it's an object of its own. The gray of a blend is the blend of the grays,
/// so the gradient stays exact.
fn gray_function(
    doc: &mut Document,
    f: &Object,
    c: Conv,
    done: &mut BTreeSet<ObjectId>,
) -> Option<Object> {
    if let Object::Reference(id) = f {
        if done.contains(id) {
            return Some(f.clone());
        }
        let inner = doc.get_object(*id).ok()?.as_dict().ok()?.clone();
        let g = gray_function_dict(doc, &inner, c, done)?;
        doc.objects.insert(*id, Object::Dictionary(g));
        done.insert(*id);
        return Some(f.clone());
    }
    let d = f.as_dict().ok()?.clone();
    Some(Object::Dictionary(gray_function_dict(doc, &d, c, done)?))
}

fn gray_function_dict(
    doc: &mut Document,
    d: &Dictionary,
    c: Conv,
    done: &mut BTreeSet<ObjectId>,
) -> Option<Dictionary> {
    let n = match c {
        Conv::Rgb => 3,
        Conv::Cmyk => 4,
        _ => return None,
    };
    let mut out = d.clone();
    match d.get(b"FunctionType").and_then(Object::as_i64).ok()? {
        2 => {
            let get = |k: &[u8], default: f32| -> Option<Vec<f32>> {
                match d.get(k) {
                    Ok(Object::Array(a)) => nums(a),
                    _ => Some(vec![default; n]),
                }
            };
            let (c0, c1) = (get(b"C0", 0.0)?, get(b"C1", 1.0)?);
            if c0.len() != n || c1.len() != n {
                return None;
            }
            out.set("C0", Object::Array(vec![real(to_gray(c, &c0)?)]));
            out.set("C1", Object::Array(vec![real(to_gray(c, &c1)?)]));
            out.remove(b"Range");
        }
        3 => {
            let Ok(Object::Array(fs)) = d.get(b"Functions") else {
                return None;
            };
            let mut new = Vec::with_capacity(fs.len());
            for f in fs.clone() {
                new.push(gray_function(doc, &f, c, done)?);
            }
            out.set("Functions", Object::Array(new));
            out.remove(b"Range");
        }
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfedit::stamp;

    #[test]
    fn converts_operators() {
        let doc = Document::with_version("1.7");
        let mut ops = vec![
            Operation::new("rg", vec![1.into(), 0.into(), 0.into()]),
            Operation::new("q", vec![]),
            Operation::new("CS", vec![Object::Name(b"DeviceCMYK".to_vec())]),
            Operation::new("SC", vec![0.into(), 0.into(), 0.into(), 1.into()]),
            Operation::new("Q", vec![]),
            Operation::new("scn", vec![0.5.into()]),
        ];
        assert!(convert_ops(&doc, None, &mut ops));
        assert_eq!(ops[0].operator, "g");
        assert_eq!(num(&ops[0].operands[0]), Some(0.299));
        assert_eq!(ops[2].operands, vec![Object::Name(b"DeviceGray".to_vec())]);
        assert_eq!(num(&ops[3].operands[0]), Some(0.0), "100% black");
        assert_eq!(ops[5].operands.len(), 1, "gray scn stays");
    }

    #[test]
    fn pages_come_out_gray() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("in.pdf");
        super::super::testing::sample(&src, 1, false);
        let mut png = Vec::new();
        let pic = image::RgbImage::from_fn(40, 20, |x, _| {
            image::Rgb([(x * 6) as u8, 200, 255 - (x * 6) as u8])
        });
        image::DynamicImage::ImageRgb8(pic)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let colourful = d.path().join("colour.pdf");
        let png = std::sync::Arc::new(png);
        stamp::stamp(&src, &colourful, None, &CancelToken::new(), |_| {
            vec![
                stamp::Item::Rect {
                    rect: [300.0, 500.0, 400.0, 600.0],
                    color: [220, 30, 30],
                    opacity: 1.0,
                },
                stamp::Item::Image {
                    bytes: png.clone(),
                    rect: [100.0, 300.0, 300.0, 400.0],
                    opacity: 1.0,
                },
            ]
        })
        .unwrap();
        let out = d.path().join("gray.pdf");
        let rep = grayscale(&colourful, &out, &CancelToken::new()).unwrap();
        assert_eq!(rep.pictures, 1, "{rep:?}");
        let img = crate::pdf_tools::PageRenderer::open(&out)
            .unwrap()
            .session(|s| s.render_dpi(0, 36.0))
            .unwrap();
        let mut dark = 0;
        for p in img.pixels() {
            let spread = p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]);
            assert!(spread <= 3, "colour left: {p:?}");
            if p[0] < 160 {
                dark += 1;
            }
        }
        assert!(dark > 100, "the red box is now a gray box");
    }
}
