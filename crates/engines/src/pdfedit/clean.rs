//! Clean-up and contents: remove hidden data, edit the document info, repair, extract the
//! images and attachments, attach files, and build bookmarks from headings.

use std::collections::HashSet;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, decode_text_string, text_string};
use swirvo_core::{CancelToken, EngineError};

use super::deref;

// ------------------------------------------------------------ hidden data

/// What [`sanitize`] removed, for the result message.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Removed {
    pub items: Vec<&'static str>,
}

impl Removed {
    fn note(&mut self, what: &'static str) {
        if !self.items.contains(&what) {
            self.items.push(what);
        }
    }
}

fn is_javascript(doc: &Document, action: &Object) -> bool {
    match deref(doc, action) {
        Object::Dictionary(d) => {
            d.get(b"S").and_then(Object::as_name).ok() == Some(b"JavaScript")
                || d.get(b"Next").is_ok_and(|n| is_javascript(doc, n))
        }
        Object::Array(a) => a.iter().any(|x| is_javascript(doc, x)),
        _ => false,
    }
}

/// Remove what readers don't see but a file can carry: author and editing history
/// (document info, XMP metadata), scripts, embedded files and attachment annotations,
/// automatic actions, thumbnails and editors' private data. Pages look the same.
pub fn sanitize(input: &Path, out: &Path) -> Result<Removed, EngineError> {
    let mut doc = super::open(input, None)?;
    let mut r = Removed::default();
    if doc.trailer.remove(b"Info").is_some() {
        r.note("author and title info");
    }
    let names = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|n| n.as_reference().ok());
    {
        let cat = doc.catalog_mut().map_err(EngineError::encode)?;
        if cat.remove(b"Metadata").is_some() {
            r.note("XMP metadata");
        }
        if cat.remove(b"AA").is_some() {
            r.note("automatic actions");
        }
        if cat.remove(b"PieceInfo").is_some() {
            r.note("editor data");
        }
        if let Ok(Object::Dictionary(n)) = cat.get_mut(b"Names") {
            if n.remove(b"JavaScript").is_some() {
                r.note("scripts");
            }
            if n.remove(b"EmbeddedFiles").is_some() {
                r.note("attached files");
            }
        }
    }
    if let Some(id) = names
        && let Ok(n) = doc.get_dictionary_mut(id)
    {
        if n.remove(b"JavaScript").is_some() {
            r.note("scripts");
        }
        if n.remove(b"EmbeddedFiles").is_some() {
            r.note("attached files");
        }
    }
    let open_js = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"OpenAction").ok())
        .is_some_and(|a| is_javascript(&doc, a));
    if open_js {
        doc.catalog_mut()
            .map_err(EngineError::encode)?
            .remove(b"OpenAction");
        r.note("scripts");
    }
    for page in super::pages(&doc) {
        let annots: Vec<Object> = doc
            .get_dictionary(page)
            .ok()
            .and_then(|d| d.get(b"Annots").ok())
            .map(|a| match deref(&doc, a) {
                Object::Array(v) => v.clone(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        let mut keep = Vec::with_capacity(annots.len());
        for a in annots {
            let d = match deref(&doc, &a) {
                Object::Dictionary(d) => d.clone(),
                _ => {
                    keep.push(a);
                    continue;
                }
            };
            if d.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"FileAttachment") {
                r.note("attached files");
                continue;
            }
            let js = d.get(b"A").is_ok_and(|x| is_javascript(&doc, x)) || d.has(b"AA");
            if js {
                r.note("scripts");
                let mut d = d;
                if d.get(b"A").is_ok_and(|x| is_javascript(&doc, x)) {
                    d.remove(b"A");
                }
                d.remove(b"AA");
                match a {
                    Object::Reference(id) => {
                        doc.objects.insert(id, Object::Dictionary(d));
                        keep.push(a);
                    }
                    _ => keep.push(Object::Dictionary(d)),
                }
            } else {
                keep.push(a);
            }
        }
        if let Ok(p) = doc.get_dictionary_mut(page) {
            for (k, what) in [
                (&b"Thumb"[..], "thumbnails"),
                (b"PieceInfo", "editor data"),
                (b"AA", "automatic actions"),
            ] {
                if p.remove(k).is_some() {
                    r.note(what);
                }
            }
            if p.has(b"Annots") {
                p.set("Annots", Object::Array(keep));
            }
        }
    }
    // Form-level scripts (field calculations, validation, formatting).
    let mut scripted = Vec::new();
    for (id, o) in &doc.objects {
        if let Object::Dictionary(d) = o
            && d.has(b"FT")
            && d.has(b"AA")
        {
            scripted.push(*id);
        }
    }
    for id in scripted {
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.remove(b"AA");
            r.note("scripts");
        }
    }
    doc.prune_objects();
    super::save(&mut doc, out, true)?;
    Ok(r)
}

// ------------------------------------------------------------ document info

#[derive(Debug, Default, Clone)]
pub struct Info {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

/// The document info as stored (Info dictionary).
pub fn read_info(input: &Path) -> Result<Info, EngineError> {
    let doc = super::open(input, None)?;
    let d = doc
        .trailer
        .get(b"Info")
        .ok()
        .map(|o| deref(&doc, o))
        .and_then(|o| o.as_dict().ok());
    let get = |k: &[u8]| {
        d.and_then(|d| d.get(k).ok())
            .and_then(|v| decode_text_string(v).ok())
            .filter(|s| !s.trim().is_empty())
    };
    Ok(Info {
        title: get(b"Title"),
        author: get(b"Author"),
        subject: get(b"Subject"),
        keywords: get(b"Keywords"),
    })
}

/// Set title, author, subject and keywords (`Some("")` clears one, `None` keeps it). The
/// XMP copy is removed so viewers can't show stale values from it.
pub fn set_info(input: &Path, out: &Path, info: &Info) -> Result<(), EngineError> {
    let mut doc = super::open(input, None)?;
    let mut d = doc
        .trailer
        .get(b"Info")
        .ok()
        .map(|o| deref(&doc, o).clone())
        .and_then(|o| o.as_dict().ok().cloned())
        .unwrap_or_default();
    for (k, v) in [
        ("Title", &info.title),
        ("Author", &info.author),
        ("Subject", &info.subject),
        ("Keywords", &info.keywords),
    ] {
        match v.as_deref().map(str::trim) {
            Some("") => {
                d.remove(k.as_bytes());
            }
            Some(s) => d.set(k, text_string(s)),
            None => {}
        }
    }
    let id = doc.add_object(Object::Dictionary(d));
    doc.trailer.set("Info", Object::Reference(id));
    if let Ok(c) = doc.catalog_mut() {
        c.remove(b"Metadata");
    }
    if info.title.as_deref().is_some_and(|t| !t.trim().is_empty())
        && let Ok(c) = doc.catalog_mut()
    {
        // Show the title, not the file name, in the viewer's title bar.
        let mut vp = match c.get(b"ViewerPreferences") {
            Ok(Object::Dictionary(v)) => v.clone(),
            _ => Dictionary::new(),
        };
        vp.set("DisplayDocTitle", Object::Boolean(true));
        c.set("ViewerPreferences", Object::Dictionary(vp));
    }
    super::save(&mut doc, out, false)
}

// ------------------------------------------------------------ repair

/// Rewrite a damaged PDF: lopdf rebuilds a broken cross-reference table and keeps
/// everything; when even that fails, the pages are re-embedded through hayro's tolerant
/// parser (links and form fields may be lost then).
pub fn repair(input: &Path, out: &Path) -> Result<&'static str, EngineError> {
    let bytes = std::fs::read(input)?;
    if let Ok(mut doc) = Document::load_mem(&bytes)
        && !doc.is_encrypted()
        && !doc.get_pages().is_empty()
    {
        super::save(&mut doc, out, false)?;
        return Ok("rewritten with a fresh structure");
    }
    let n = super::assemble::page_count(input)?;
    super::assemble::assemble(
        &[input.to_path_buf()],
        &super::assemble::reorder_plan(&(0..n).collect::<Vec<_>>()),
        out,
        &CancelToken::new(),
    )?;
    Ok("pages recovered into a new file")
}

// ------------------------------------------------------------ images

fn name_list(o: &Object) -> Vec<Vec<u8>> {
    match o {
        Object::Name(n) => vec![n.clone()],
        Object::Array(a) => a
            .iter()
            .filter_map(|x| x.as_name().ok().map(<[u8]>::to_vec))
            .collect(),
        _ => Vec::new(),
    }
}

/// Decode a PDF image XObject to RGBA pixels (common kinds: 1/8-bit gray, RGB, CMYK,
/// indexed, with soft masks).
pub(crate) fn decode_image(doc: &Document, st: &Stream) -> Option<image::DynamicImage> {
    let d = &st.dict;
    let w = d.get(b"Width").and_then(Object::as_i64).ok()? as u32;
    let h = d.get(b"Height").and_then(Object::as_i64).ok()? as u32;
    if w == 0 || h == 0 || (w as u64 * h as u64) > 100_000_000 {
        return None;
    }
    let filters = d.get(b"Filter").map(name_list).unwrap_or_default();
    let mut img = if filters.last().map(Vec::as_slice) == Some(b"DCTDecode") {
        image::load_from_memory_with_format(&st.content, image::ImageFormat::Jpeg).ok()?
    } else if filters.iter().all(|f| {
        matches!(
            f.as_slice(),
            b"FlateDecode" | b"LZWDecode" | b"ASCII85Decode"
        )
    }) {
        let raw = st.decompressed_content().ok()?;
        let bpc = d
            .get(b"BitsPerComponent")
            .and_then(Object::as_i64)
            .unwrap_or(8);
        let cs = d.get(b"ColorSpace").map(|c| deref(doc, c).clone()).ok();
        let mask = d
            .get(b"ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false);
        raw_to_image(doc, &raw, w, h, bpc, cs.as_ref(), mask)?
    } else {
        return None;
    };
    // Soft mask → alpha.
    if let Ok(Object::Reference(sm)) = d.get(b"SMask")
        && let Ok(Object::Stream(ms)) = doc.get_object(*sm)
        && let Some(m) = decode_image(doc, ms)
    {
        let m = m
            .resize_exact(
                img.width(),
                img.height(),
                image::imageops::FilterType::Triangle,
            )
            .to_luma8();
        let mut rgba = img.to_rgba8();
        for (p, a) in rgba.pixels_mut().zip(m.pixels()) {
            p[3] = a[0];
        }
        img = image::DynamicImage::ImageRgba8(rgba);
    }
    Some(img)
}

fn raw_to_image(
    doc: &Document,
    raw: &[u8],
    w: u32,
    h: u32,
    bpc: i64,
    cs: Option<&Object>,
    mask: bool,
) -> Option<image::DynamicImage> {
    let unpack = |comps: u32| -> Option<Vec<u8>> {
        match bpc {
            8 => {
                let need = (w * h * comps) as usize;
                (raw.len() >= need).then(|| raw[..need].to_vec())
            }
            1 | 2 | 4 => {
                let bits = bpc as u32;
                let row = (w * comps * bits).div_ceil(8) as usize;
                if raw.len() < row * h as usize {
                    return None;
                }
                let max = (1u32 << bits) - 1;
                let mut out = Vec::with_capacity((w * h * comps) as usize);
                for y in 0..h as usize {
                    let r = &raw[y * row..(y + 1) * row];
                    for i in 0..(w * comps) as usize {
                        let bit = i * bits as usize;
                        let v = (r[bit / 8] >> (8 - bits as usize - bit % 8)) as u32 & max;
                        out.push((v * 255 / max) as u8);
                    }
                }
                Some(out)
            }
            _ => None,
        }
    };
    if mask {
        let px = unpack(1)?;
        // Stencil masks paint where the bit is 0: show them black on white.
        return image::GrayImage::from_raw(w, h, px.iter().map(|&v| 255 - v).collect())
            .map(image::DynamicImage::ImageLuma8);
    }
    let name = match cs {
        Some(Object::Name(n)) => n.clone(),
        Some(Object::Array(a)) => a
            .first()
            .and_then(|x| x.as_name().ok())
            .unwrap_or(b"")
            .to_vec(),
        _ => b"DeviceGray".to_vec(),
    };
    let icc_n = || -> Option<i64> {
        let Some(Object::Array(a)) = cs else {
            return None;
        };
        let st = deref(doc, a.get(1)?).as_stream().ok()?;
        st.dict.get(b"N").and_then(Object::as_i64).ok()
    };
    let comps = match name.as_slice() {
        b"DeviceGray" | b"CalGray" | b"G" => 1,
        b"DeviceRGB" | b"CalRGB" | b"RGB" => 3,
        b"DeviceCMYK" | b"CMYK" => 4,
        b"ICCBased" => icc_n()? as u32,
        b"Indexed" | b"I" => {
            let Some(Object::Array(a)) = cs else {
                return None;
            };
            let base = deref(doc, a.get(1)?);
            let base_n = match base {
                Object::Name(n) if n == b"DeviceRGB" => 3,
                Object::Name(n) if n == b"DeviceGray" => 1,
                Object::Name(n) if n == b"DeviceCMYK" => 4,
                Object::Array(b)
                    if b.first().and_then(|x| x.as_name().ok()) == Some(b"ICCBased") =>
                {
                    deref(doc, b.get(1)?)
                        .as_stream()
                        .ok()?
                        .dict
                        .get(b"N")
                        .and_then(Object::as_i64)
                        .ok()? as usize
                }
                _ => return None,
            };
            let table = match deref(doc, a.get(3)?) {
                Object::String(s, _) => s.clone(),
                Object::Stream(s) => s.decompressed_content().ok()?,
                _ => return None,
            };
            let idx = match bpc {
                8 => {
                    let need = (w * h) as usize;
                    (raw.len() >= need).then(|| raw[..need].to_vec())?
                }
                _ => {
                    let bits = bpc as u32;
                    let row = (w * bits).div_ceil(8) as usize;
                    if raw.len() < row * h as usize {
                        return None;
                    }
                    let max = (1u32 << bits) - 1;
                    let mut out = Vec::with_capacity((w * h) as usize);
                    for y in 0..h as usize {
                        let r = &raw[y * row..(y + 1) * row];
                        for i in 0..w as usize {
                            let bit = i * bits as usize;
                            out.push(
                                ((r[bit / 8] >> (8 - bits as usize - bit % 8)) as u32 & max) as u8,
                            );
                        }
                    }
                    out
                }
            };
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for &i in &idx {
                let o = i as usize * base_n;
                let c = table.get(o..o + base_n).unwrap_or(&[0, 0, 0, 0][..base_n]);
                rgb.extend(to_rgb(c));
            }
            return image::RgbImage::from_raw(w, h, rgb).map(image::DynamicImage::ImageRgb8);
        }
        _ => return None,
    };
    let px = unpack(comps)?;
    match comps {
        1 => image::GrayImage::from_raw(w, h, px).map(image::DynamicImage::ImageLuma8),
        3 => image::RgbImage::from_raw(w, h, px).map(image::DynamicImage::ImageRgb8),
        4 => {
            let rgb: Vec<u8> = px.chunks(4).flat_map(to_rgb).collect();
            image::RgbImage::from_raw(w, h, rgb).map(image::DynamicImage::ImageRgb8)
        }
        _ => None,
    }
}

fn to_rgb(c: &[u8]) -> [u8; 3] {
    match c.len() {
        1 => [c[0]; 3],
        3 => [c[0], c[1], c[2]],
        4 => {
            let k = 255 - c[3] as u32;
            let f = |v: u8| ((255 - v as u32) * k / 255) as u8;
            [f(c[0]), f(c[1]), f(c[2])]
        }
        _ => [0; 3],
    }
}

/// Save every image in the PDF into `dir`: JPEGs as they are (no quality loss), JPEG 2000
/// as `.jp2`, everything else as PNG. Tiny images (icons, bullets) are skipped.
pub fn extract_images(
    input: &Path,
    dir: &Path,
    stem: &str,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let doc = super::open(input, None)?;
    let mut seen = HashSet::new();
    let mut ids: Vec<ObjectId> = Vec::new();
    // Page order first, so numbering follows the document.
    for page in super::pages(&doc) {
        for img in doc.get_page_images(page).unwrap_or_default() {
            if seen.insert(img.id) {
                ids.push(img.id);
            }
        }
    }
    let mut outs = Vec::new();
    for id in ids {
        cancel.check()?;
        let Ok(Object::Stream(st)) = doc.get_object(id) else {
            continue;
        };
        let w = st.dict.get(b"Width").and_then(Object::as_i64).unwrap_or(0);
        let h = st.dict.get(b"Height").and_then(Object::as_i64).unwrap_or(0);
        if w < 24
            || h < 24
            || st
                .dict
                .get(b"ImageMask")
                .and_then(Object::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        let filters = st.dict.get(b"Filter").map(name_list).unwrap_or_default();
        let n = outs.len() + 1;
        let plain_jpeg = filters == [b"DCTDecode".to_vec()] && !st.dict.has(b"SMask");
        let (bytes, ext) = if plain_jpeg {
            (st.content.clone(), "jpg")
        } else if filters.last().map(Vec::as_slice) == Some(b"JPXDecode") {
            (st.content.clone(), "jp2")
        } else if let Some(img) = decode_image(&doc, st) {
            let mut png = Vec::new();
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(EngineError::encode)?;
            (png, "png")
        } else {
            continue;
        };
        let out = dir.join(format!("{stem} image {n:03}.{ext}"));
        std::fs::write(&out, bytes)?;
        outs.push(out);
    }
    if outs.is_empty() {
        return Err(EngineError::Invalid(
            "this PDF has no pictures to save".into(),
        ));
    }
    Ok(outs)
}

// ------------------------------------------------------------ attachments

/// A file name from an attachment, made safe: no folders, no reserved characters.
fn safe_file_name(s: &str) -> String {
    let base = s.rsplit(['/', '\\']).next().unwrap_or(s);
    let t: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let t = t
        .trim()
        .trim_start_matches('.')
        .trim_end_matches(['.', ' '])
        .to_string();
    let reserved = ["con", "prn", "aux", "nul", "com1", "com2", "lpt1", "lpt2"];
    let stem = t.split('.').next().unwrap_or("").to_ascii_lowercase();
    if t.is_empty() || reserved.contains(&stem.as_str()) {
        format!("attachment {t}").trim().to_string()
    } else {
        t.chars().take(150).collect()
    }
}

/// Every embedded file: (name, bytes).
fn attachments(doc: &Document) -> Vec<(String, Vec<u8>)> {
    let mut specs: Vec<Dictionary> = Vec::new();
    fn walk(doc: &Document, node: &Object, out: &mut Vec<Dictionary>, depth: u32) {
        if depth > 32 {
            return;
        }
        let Object::Dictionary(d) = deref(doc, node) else {
            return;
        };
        if let Ok(Object::Array(names)) = d.get(b"Names").map(|n| deref(doc, n)) {
            for pair in names.chunks(2) {
                if let Some(Object::Dictionary(fs)) = pair.get(1).map(|f| deref(doc, f)) {
                    out.push(fs.clone());
                }
            }
        }
        if let Ok(Object::Array(kids)) = d.get(b"Kids").map(|k| deref(doc, k)) {
            for k in kids {
                walk(doc, k, out, depth + 1);
            }
        }
    }
    if let Some(ef) = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .map(|n| deref(doc, n))
        .and_then(|n| n.as_dict().ok())
        .and_then(|n| n.get(b"EmbeddedFiles").ok())
    {
        walk(doc, ef, &mut specs, 0);
    }
    for page in super::pages(doc) {
        for a in doc.get_page_annotations(page).unwrap_or_default() {
            if a.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"FileAttachment")
                && let Ok(Object::Dictionary(fs)) = a.get(b"FS").map(|f| deref(doc, f))
            {
                specs.push(fs.clone());
            }
        }
    }
    let mut out = Vec::new();
    for fs in specs {
        let name = fs
            .get(b"UF")
            .or_else(|_| fs.get(b"F"))
            .ok()
            .and_then(|n| decode_text_string(deref(doc, n)).ok())
            .unwrap_or_else(|| "attachment".into());
        let Some(Object::Stream(st)) = fs
            .get(b"EF")
            .ok()
            .map(|e| deref(doc, e))
            .and_then(|e| e.as_dict().ok())
            .and_then(|e| e.get(b"UF").or_else(|_| e.get(b"F")).ok())
            .map(|s| deref(doc, s))
        else {
            continue;
        };
        if let Ok(bytes) = st.decompressed_content() {
            out.push((name, bytes));
        }
    }
    out
}

/// Save the files embedded in the PDF into `dir` (names made safe; never outside `dir`).
pub fn extract_attachments(input: &Path, dir: &Path) -> Result<Vec<PathBuf>, EngineError> {
    let doc = super::open(input, None)?;
    let files = attachments(&doc);
    if files.is_empty() {
        return Err(EngineError::Invalid(
            "this PDF has no attached files".into(),
        ));
    }
    let mut outs = Vec::new();
    let mut used = HashSet::new();
    for (name, bytes) in files {
        let mut n = safe_file_name(&name);
        let mut k = 2;
        while !used.insert(n.to_lowercase()) {
            let (stem, ext) = match n.rsplit_once('.') {
                Some((s, e)) => (s.to_string(), format!(".{e}")),
                None => (n.clone(), String::new()),
            };
            n = format!("{stem} ({k}){ext}");
            k += 1;
        }
        let out = dir.join(&n);
        std::fs::write(&out, bytes)?;
        outs.push(out);
    }
    Ok(outs)
}

/// Embed `files` in the PDF (shown in viewers' attachments panel).
pub fn attach(input: &Path, files: &[PathBuf], out: &Path) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let mut pairs: Vec<Object> = Vec::new();
    // Keep what's attached already.
    let existing = attachments(&doc);
    let add = |doc: &mut Document, name: &str, bytes: Vec<u8>| {
        let mut params = Dictionary::new();
        params.set("Size", Object::Integer(bytes.len() as i64));
        let mut sd = Dictionary::new();
        sd.set("Type", Object::Name(b"EmbeddedFile".to_vec()));
        sd.set("Params", Object::Dictionary(params));
        let mut st = Stream::new(sd, bytes);
        let _ = st.compress();
        let sid = doc.add_object(st);
        let mut ef = Dictionary::new();
        ef.set("F", Object::Reference(sid));
        ef.set("UF", Object::Reference(sid));
        let mut fs = Dictionary::new();
        fs.set("Type", Object::Name(b"Filespec".to_vec()));
        fs.set("F", text_string(name));
        fs.set("UF", text_string(name));
        fs.set("EF", Object::Dictionary(ef));
        let fid = doc.add_object(Object::Dictionary(fs));
        (text_string(name), Object::Reference(fid))
    };
    let mut entries: Vec<(String, Object, Object)> = Vec::new();
    for (name, bytes) in existing {
        let (k, v) = add(&mut doc, &name, bytes);
        entries.push((name, k, v));
    }
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let bytes = std::fs::read(f)?;
        let (k, v) = add(&mut doc, &name, bytes);
        entries.push((name, k, v));
    }
    // Name trees must be sorted by key.
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, k, v) in entries {
        pairs.push(k);
        pairs.push(v);
    }
    let mut tree = Dictionary::new();
    tree.set("Names", Object::Array(pairs));
    let tree_id = doc.add_object(Object::Dictionary(tree));
    let names_ref = doc
        .catalog()
        .ok()
        .and_then(|c| c.get(b"Names").ok())
        .and_then(|n| n.as_reference().ok());
    match names_ref {
        Some(id) => {
            doc.get_dictionary_mut(id)
                .map_err(EngineError::encode)?
                .set("EmbeddedFiles", Object::Reference(tree_id));
        }
        None => {
            let cat = doc.catalog_mut().map_err(EngineError::encode)?;
            let mut names = match cat.get(b"Names") {
                Ok(Object::Dictionary(n)) => n.clone(),
                _ => Dictionary::new(),
            };
            names.set("EmbeddedFiles", Object::Reference(tree_id));
            cat.set("Names", Object::Dictionary(names));
        }
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.set("PageMode", Object::Name(b"UseAttachments".to_vec()));
    }
    doc.prune_objects();
    super::save(&mut doc, out, false)?;
    Ok(files.len())
}

// ------------------------------------------------------------ bookmarks

/// One outline entry: level 0 is a top-level chapter.
#[derive(Debug, Clone, PartialEq)]
pub struct Mark {
    pub level: u8,
    pub title: String,
    pub page: usize,
    /// Top of the target in display points from the page bottom (None = top of page).
    pub top: Option<f32>,
}

/// Replace the PDF's bookmarks with `marks` (in reading order).
pub fn set_bookmarks(input: &Path, out: &Path, marks: &[Mark]) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let pages = super::pages(&doc);
    if marks.is_empty() {
        return Err(EngineError::Invalid("no headings found to bookmark".into()));
    }
    let root = doc.new_object_id();
    // Allocate ids, then link parents, siblings and children by level.
    let ids: Vec<ObjectId> = marks.iter().map(|_| doc.new_object_id()).collect();
    let mut parent: Vec<Option<usize>> = vec![None; marks.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        while let Some(&top) = stack.last() {
            if marks[top].level >= m.level {
                stack.pop();
            } else {
                break;
            }
        }
        parent[i] = stack.last().copied();
        stack.push(i);
    }
    let children =
        |p: Option<usize>| -> Vec<usize> { (0..marks.len()).filter(|&i| parent[i] == p).collect() };
    fn count(i: Option<usize>, parent: &[Option<usize>]) -> i64 {
        (0..parent.len())
            .filter(|&k| parent[k] == i)
            .map(|k| 1 + count(Some(k), parent))
            .sum()
    }
    for (i, m) in marks.iter().enumerate() {
        let page = *pages
            .get(m.page)
            .ok_or_else(|| EngineError::Invalid(format!("no page {}", m.page + 1)))?;
        let g = super::geometry::Geom::of(&doc, page);
        let dest_top = m.top.map(|t| {
            let u = g.rect_to_user([0.0, t, 1.0, t]);
            Object::Real(u[3])
        });
        let mut d = Dictionary::new();
        d.set("Title", text_string(&m.title));
        d.set(
            "Parent",
            Object::Reference(parent[i].map_or(root, |p| ids[p])),
        );
        d.set(
            "Dest",
            Object::Array(vec![
                Object::Reference(page),
                Object::Name(b"XYZ".to_vec()),
                Object::Null,
                dest_top.unwrap_or(Object::Null),
                Object::Null,
            ]),
        );
        let sibs = children(parent[i]);
        let pos = sibs.iter().position(|&s| s == i).unwrap_or(0);
        if pos > 0 {
            d.set("Prev", Object::Reference(ids[sibs[pos - 1]]));
        }
        if let Some(&n) = sibs.get(pos + 1) {
            d.set("Next", Object::Reference(ids[n]));
        }
        let kids = children(Some(i));
        if let (Some(&f), Some(&l)) = (kids.first(), kids.last()) {
            d.set("First", Object::Reference(ids[f]));
            d.set("Last", Object::Reference(ids[l]));
            // Top-level chapters open; deeper levels start collapsed.
            let c = count(Some(i), &parent);
            d.set("Count", Object::Integer(if m.level == 0 { c } else { -c }));
        }
        doc.objects.insert(ids[i], Object::Dictionary(d));
    }
    let top = children(None);
    let mut o = Dictionary::new();
    o.set("Type", Object::Name(b"Outlines".to_vec()));
    if let (Some(&f), Some(&l)) = (top.first(), top.last()) {
        o.set("First", Object::Reference(ids[f]));
        o.set("Last", Object::Reference(ids[l]));
    }
    o.set("Count", Object::Integer(count(None, &parent)));
    doc.objects.insert(root, Object::Dictionary(o));
    let cat = doc.catalog_mut().map_err(EngineError::encode)?;
    cat.set("Outlines", Object::Reference(root));
    cat.set("PageMode", Object::Name(b"UseOutlines".to_vec()));
    doc.prune_objects();
    super::save(&mut doc, out, false)?;
    Ok(marks.len())
}

/// Bookmarks from the headings our layout analysis finds (docs/13): chapter titles,
/// section headings, by size and weight.
pub fn bookmarks_from_headings(
    input: &Path,
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let bytes = std::fs::read(input)?;
    let layout = crate::pdflayout::analyze(&bytes, cancel)?;
    let mut marks = Vec::new();
    for (pi, page) in layout.pages.iter().enumerate() {
        for band in &page.bands {
            for col in &band.cols {
                for b in &col.blocks {
                    if let crate::pdflayout::model::Block::Para(p) = b
                        && let Some(level) = p.heading
                    {
                        let title = p.text().split_whitespace().collect::<Vec<_>>().join(" ");
                        if title.is_empty() || title.chars().count() > 200 {
                            continue;
                        }
                        marks.push(Mark {
                            level: level.saturating_sub(1),
                            title,
                            page: pi,
                            top: Some(page.h - p.top()),
                        });
                    }
                }
            }
        }
    }
    if marks.is_empty() {
        return Err(EngineError::Invalid(
            "no headings found (it may be a scan, or plain text without headings)".into(),
        ));
    }
    set_bookmarks(input, out, &marks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_info_attach_extract_bookmarks() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 3, false);

        let titled = d.path().join("t.pdf");
        set_info(
            &src,
            &titled,
            &Info {
                title: Some("Résumé 2026".into()),
                author: Some("A. Writer".into()),
                ..Info::default()
            },
        )
        .unwrap();
        let i = read_info(&titled).unwrap();
        assert_eq!(i.title.as_deref(), Some("Résumé 2026"));
        assert_eq!(i.author.as_deref(), Some("A. Writer"));

        let note = d.path().join("notes.txt");
        std::fs::write(&note, b"hello").unwrap();
        let att = d.path().join("att.pdf");
        assert_eq!(
            attach(&titled, std::slice::from_ref(&note), &att).unwrap(),
            1
        );
        let outdir = d.path().join("out");
        std::fs::create_dir(&outdir).unwrap();
        let got = extract_attachments(&att, &outdir).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(std::fs::read(&got[0]).unwrap(), b"hello");
        assert!(got[0].ends_with("notes.txt"));

        let clean = d.path().join("clean.pdf");
        let r = sanitize(&att, &clean).unwrap();
        assert!(r.items.contains(&"author and title info"), "{r:?}");
        assert!(r.items.contains(&"attached files"), "{r:?}");
        assert!(read_info(&clean).unwrap().author.is_none());
        assert!(extract_attachments(&clean, &outdir).is_err());
        // The link annotation is still there.
        let doc = lopdf::Document::load(&clean).unwrap();
        let p1 = super::super::pages(&doc)[0];
        assert_eq!(doc.get_page_annotations(p1).unwrap().len(), 1);

        let marked = d.path().join("m.pdf");
        let marks = [
            Mark {
                level: 0,
                title: "Chapter 1".into(),
                page: 0,
                top: None,
            },
            Mark {
                level: 1,
                title: "Section 1.1".into(),
                page: 1,
                top: Some(400.0),
            },
            Mark {
                level: 0,
                title: "Chapter 2 — Ünïcode".into(),
                page: 2,
                top: None,
            },
        ];
        assert_eq!(set_bookmarks(&src, &marked, &marks).unwrap(), 3);
        let doc = lopdf::Document::load(&marked).unwrap();
        let toc = doc.get_toc().unwrap();
        let titles: Vec<(usize, String, usize)> = toc
            .toc
            .iter()
            .map(|t| (t.level, t.title.clone(), t.page))
            .collect();
        assert_eq!(titles[0], (1, "Chapter 1".into(), 1));
        assert_eq!(titles[1], (2, "Section 1.1".into(), 2));
        assert_eq!(titles[2].1, "Chapter 2 — Ünïcode");

        let fixed = d.path().join("fixed.pdf");
        repair(&src, &fixed).unwrap();
        assert_eq!(super::super::assemble::page_count(&fixed).unwrap(), 3);
    }

    #[test]
    fn attachment_names_stay_inside_the_folder() {
        assert_eq!(safe_file_name("../../evil.exe"), "evil.exe");
        assert_eq!(safe_file_name("C:\\Windows\\x.dll"), "x.dll");
        assert_eq!(safe_file_name("con.txt"), "attachment con.txt");
        assert_eq!(safe_file_name("..."), "attachment");
        assert_eq!(safe_file_name("a:b?.pdf"), "a_b_.pdf");
    }

    #[test]
    fn extracts_pictures() {
        let d = tempfile::tempdir().unwrap();
        let png = d.path().join("p.png");
        image::RgbImage::from_fn(120, 80, |x, y| image::Rgb([x as u8 * 2, y as u8 * 3, 90]))
            .save(&png)
            .unwrap();
        let pdf = d.path().join("i.pdf");
        crate::pdf_tools::images_to_pdf(&[png.clone(), png], &pdf, &CancelToken::new()).unwrap();
        let out = d.path().join("imgs");
        std::fs::create_dir(&out).unwrap();
        let got = extract_images(&pdf, &out, "i", &CancelToken::new()).unwrap();
        assert!(!got.is_empty());
        let img = image::open(&got[0]).unwrap();
        assert_eq!((img.width(), img.height()), (120, 80));
    }
}
