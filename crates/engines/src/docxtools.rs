//! Word tools (docs/17 §6): compress, remove personal info and comments, accept tracked
//! changes, save pictures, statistics, find and replace, and mail merge. Each reads a
//! .docx and writes a new one, editing the XML losslessly (`xmldom`) so everything a tool
//! doesn't mean to change stays byte-for-byte what Word wrote.

use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use swirvo_core::{CancelToken, EngineError};

use crate::xmldom::{self, Elem, Keep, XNode};

/// Per-part cap when reading packages.
const PART_LIMIT: u64 = 256 << 20;

/// A package in memory: part name → bytes, in the original order.
pub struct Pkg {
    pub parts: Vec<(String, Vec<u8>)>,
}

impl Pkg {
    pub fn read(bytes: &[u8]) -> Result<Pkg, EngineError> {
        let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(EngineError::decode)?;
        let mut parts = Vec::with_capacity(z.len());
        let mut total = 0u64;
        for i in 0..z.len() {
            let f = z.by_index(i).map_err(EngineError::decode)?;
            if f.is_dir() {
                continue;
            }
            let name = f.name().map_err(EngineError::decode)?.to_string();
            let mut v = Vec::new();
            f.take(PART_LIMIT).read_to_end(&mut v)?;
            total += v.len() as u64;
            if total > 2 << 30 {
                return Err(EngineError::Invalid(
                    "this document is too big to edit".into(),
                ));
            }
            parts.push((name, v));
        }
        if !parts.iter().any(|(n, _)| n == "word/document.xml") {
            return Err(EngineError::Decode("not a Word document".into()));
        }
        Ok(Pkg { parts })
    }

    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.parts
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, b)| b.as_slice())
    }

    pub fn set(&mut self, name: &str, bytes: Vec<u8>) {
        match self.parts.iter_mut().find(|(n, _)| n == name) {
            Some(p) => p.1 = bytes,
            None => self.parts.push((name.to_string(), bytes)),
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let n = self.parts.len();
        self.parts.retain(|(p, _)| p != name);
        n != self.parts.len()
    }

    pub fn xml(&self, name: &str) -> Option<xmldom::Doc> {
        self.get(name).and_then(|b| xmldom::parse(b).ok())
    }

    pub fn set_xml(&mut self, name: &str, d: &xmldom::Doc) -> Result<(), EngineError> {
        self.set(name, d.write()?);
        Ok(())
    }

    /// The text parts Word renders: body, headers, footers, notes.
    pub fn story_parts(&self) -> Vec<String> {
        self.parts
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| {
                n == "word/document.xml"
                    || (n.starts_with("word/header") && n.ends_with(".xml"))
                    || (n.starts_with("word/footer") && n.ends_with(".xml"))
                    || n == "word/footnotes.xml"
                    || n == "word/endnotes.xml"
            })
            .collect()
    }

    /// Write with maximum deflate; `[Content_Types].xml` first, as Office expects.
    pub fn write(&self) -> Result<Vec<u8>, EngineError> {
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(9));
            let mut order: Vec<&(String, Vec<u8>)> = self.parts.iter().collect();
            order.sort_by_key(|(n, _)| (n != "[Content_Types].xml", n != "_rels/.rels"));
            for (name, bytes) in order {
                z.start_file(name.as_str(), o)
                    .map_err(EngineError::encode)?;
                z.write_all(bytes)?;
            }
            z.finish().map_err(EngineError::encode)?;
        }
        Ok(buf)
    }
}

fn rels_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

// ------------------------------------------------------------ compress

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Shrunk {
    pub before: u64,
    pub after: u64,
    pub pictures: usize,
}

/// Displayed size (points) of each media part, from the drawings that show it.
fn displayed(pkg: &Pkg) -> HashMap<String, (f32, f32)> {
    let mut out: HashMap<String, (f32, f32)> = HashMap::new();
    for part in pkg.story_parts() {
        let Some(d) = pkg.xml(&part) else { continue };
        let Some(rels) = pkg.xml(&rels_name(&part)) else {
            continue;
        };
        let targets: HashMap<String, String> = rels
            .root
            .children()
            .filter_map(|r| Some((r.attr("Id")?.to_string(), r.attr("Target")?.to_string())))
            .collect();
        let dir = part.rsplit_once('/').map_or("", |p| p.0).to_string();
        fn walk(e: &Elem, size: Option<(f32, f32)>, f: &mut dyn FnMut(&str, (f32, f32))) {
            let mut size = size;
            if e.local() == "extent"
                && let (Some(cx), Some(cy)) = (e.attr("cx"), e.attr("cy"))
            {
                size = Some((
                    cx.parse::<f32>().unwrap_or(0.0) / 12_700.0,
                    cy.parse::<f32>().unwrap_or(0.0) / 12_700.0,
                ));
            }
            if e.local() == "blip"
                && let (Some(id), Some(s)) = (e.attr("r:embed"), size)
            {
                f(id, s);
            }
            for c in e.children() {
                walk(c, size, f);
            }
        }
        // `extent` sits beside the graphic in an inline/anchor; walk with the nearest.
        let mut found = Vec::new();
        fn drawings(e: &Elem, found: &mut Vec<(String, (f32, f32))>) {
            if matches!(e.local(), "inline" | "anchor") {
                walk(e, None, &mut |id, s| found.push((id.to_string(), s)));
                return;
            }
            for c in e.children() {
                drawings(c, found);
            }
        }
        drawings(&d.root, &mut found);
        for (id, s) in found {
            if let Some(t) = targets.get(&id) {
                let path = crate::opc::join(&dir, t);
                let e = out.entry(path).or_insert((0.0, 0.0));
                e.0 = e.0.max(s.0);
                e.1 = e.1.max(s.1);
            }
        }
    }
    out
}

/// Shrink the pictures to what their displayed size needs at `dpi` (photos become JPEG
/// at `quality`; screenshots and drawings stay lossless) and repack at maximum
/// compression.
pub fn compress(
    input: &Path,
    out: &Path,
    dpi: f32,
    quality: u8,
    cancel: &CancelToken,
) -> Result<Shrunk, EngineError> {
    let bytes = std::fs::read(input)?;
    let mut pkg = Pkg::read(&bytes)?;
    let shown = displayed(&pkg);
    let mut renames: Vec<(String, String)> = Vec::new();
    let mut count = 0;
    let media: Vec<String> = pkg
        .parts
        .iter()
        .map(|(n, _)| n.clone())
        .filter(|n| n.starts_with("word/media/"))
        .collect();
    for name in media {
        cancel.check()?;
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !matches!(
            ext.as_str(),
            "png" | "jpg" | "jpeg" | "bmp" | "tif" | "tiff" | "gif"
        ) {
            continue;
        }
        let Some(data) = pkg.get(&name).map(<[u8]>::to_vec) else {
            continue;
        };
        if data.len() < 24 * 1024 {
            continue;
        }
        let Ok(img) = image::load_from_memory(&data) else {
            continue;
        };
        let (w, h) = (img.width(), img.height());
        let scale = shown.get(&name).map_or(1.0, |&(sw, sh)| {
            let have = (w as f32 / (sw / 72.0).max(0.01)).min(h as f32 / (sh / 72.0).max(0.01));
            if have > dpi * 1.3 { dpi / have } else { 1.0 }
        });
        let img = if scale < 1.0 {
            img.resize_exact(
                ((w as f32 * scale) as u32).max(1),
                ((h as f32 * scale) as u32).max(1),
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img
        };
        let alpha = img.color().has_alpha();
        let photo = !alpha && crate::pdfedit::compress::photographic(&img);
        let (new_bytes, new_ext) = if photo || ext == "jpg" || ext == "jpeg" {
            let mut b = Vec::new();
            let mut cur = Cursor::new(&mut b);
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cur, quality);
            image::DynamicImage::ImageRgb8(img.to_rgb8())
                .write_with_encoder(enc)
                .map_err(EngineError::encode)?;
            (b, "jpeg")
        } else {
            let mut b = Vec::new();
            let enc = image::codecs::png::PngEncoder::new_with_quality(
                &mut b,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            );
            img.write_with_encoder(enc).map_err(EngineError::encode)?;
            (b, "png")
        };
        if new_bytes.len() as f32 > data.len() as f32 * 0.9 {
            continue;
        }
        count += 1;
        let same_kind =
            (new_ext == "jpeg" && matches!(ext.as_str(), "jpg" | "jpeg")) || new_ext == ext;
        if same_kind {
            pkg.set(&name, new_bytes);
        } else {
            let stem = name.rsplit_once('.').map_or(name.as_str(), |p| p.0);
            let new_name = format!("{stem}.{new_ext}");
            pkg.remove(&name);
            pkg.set(&new_name, new_bytes);
            renames.push((name, new_name));
        }
    }
    if !renames.is_empty() {
        // Point relationships at the new names and register the extension.
        let rel_parts: Vec<String> = pkg
            .parts
            .iter()
            .map(|(n, _)| n.clone())
            .filter(|n| n.ends_with(".rels"))
            .collect();
        for rp in rel_parts {
            let Some(mut d) = pkg.xml(&rp) else { continue };
            let mut changed = false;
            for r in d.root.children_mut() {
                if let Some(t) = r.attr("Target").map(str::to_string) {
                    for (old, new) in &renames {
                        let old_file = old.rsplit('/').next().unwrap_or(old);
                        if t.ends_with(old_file) {
                            let new_file = new.rsplit('/').next().unwrap_or(new);
                            r.set_attr("Target", &t.replace(old_file, new_file));
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                pkg.set_xml(&rp, &d)?;
            }
        }
        if let Some(mut ct) = pkg.xml("[Content_Types].xml") {
            let has = ct.root.children().any(|e| {
                e.local() == "Default"
                    && e.attr("Extension")
                        .is_some_and(|x| x.eq_ignore_ascii_case("jpeg"))
            });
            if !has {
                ct.root.kids.insert(
                    0,
                    XNode::Elem(Elem {
                        name: "Default".into(),
                        attrs: vec![
                            ("Extension".into(), "jpeg".into()),
                            ("ContentType".into(), "image/jpeg".into()),
                        ],
                        kids: Vec::new(),
                        empty: true,
                    }),
                );
                pkg.set_xml("[Content_Types].xml", &ct)?;
            }
        }
    }
    let data = pkg.write()?;
    std::fs::write(out, &data)?;
    Ok(Shrunk {
        before: bytes.len() as u64,
        after: data.len() as u64,
        pictures: count,
    })
}

// ------------------------------------------------------------ clean

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub items: Vec<&'static str>,
}

/// Remove author and company, comments, and editing-session ids; with `accept`, also
/// accept every tracked change (insertions kept, deletions removed, formatting changes
/// kept as they are now).
pub fn clean(input: &Path, out: &Path, accept: bool) -> Result<Cleaned, EngineError> {
    let mut pkg = Pkg::read(&std::fs::read(input)?)?;
    let mut done = Cleaned::default();
    let note = |s: &'static str, d: &mut Cleaned| {
        if !d.items.contains(&s) {
            d.items.push(s);
        }
    };
    // Document properties.
    for (part, keys) in [
        ("docProps/core.xml", &["creator", "lastModifiedBy"][..]),
        ("docProps/app.xml", &["Company", "Manager"][..]),
    ] {
        if let Some(mut d) = pkg.xml(part) {
            let mut changed = false;
            for e in d.root.children_mut() {
                if keys.contains(&e.local()) && !e.text().is_empty() {
                    e.kids.clear();
                    changed = true;
                }
            }
            if changed {
                pkg.set_xml(part, &d)?;
                note("author and company", &mut done);
            }
        }
    }
    // Comments and people.
    for p in [
        "word/comments.xml",
        "word/commentsExtended.xml",
        "word/commentsIds.xml",
        "word/commentsExtensible.xml",
        "word/people.xml",
    ] {
        if pkg.remove(p) {
            note("comments", &mut done);
        }
    }
    if let Some(mut rels) = pkg.xml("word/_rels/document.xml.rels") {
        rels.root.filter(&|e| {
            let t = e.attr("Type").unwrap_or("");
            if t.ends_with("/comments")
                || t.ends_with("/commentsExtended")
                || t.ends_with("/commentsIds")
                || t.ends_with("/commentsExtensible")
                || t.ends_with("/people")
            {
                Keep::Drop
            } else {
                Keep::Yes
            }
        });
        pkg.set_xml("word/_rels/document.xml.rels", &rels)?;
    }
    if let Some(mut ct) = pkg.xml("[Content_Types].xml") {
        ct.root.filter(&|e| {
            let p = e.attr("PartName").unwrap_or("");
            if p.starts_with("/word/comments") || p == "/word/people.xml" {
                Keep::Drop
            } else {
                Keep::Yes
            }
        });
        pkg.set_xml("[Content_Types].xml", &ct)?;
    }
    for part in pkg.story_parts() {
        let Some(mut d) = pkg.xml(&part) else {
            continue;
        };
        let before = d.write()?;
        let accept_now = accept;
        d.root.filter(&|e| match e.local() {
            "commentRangeStart" | "commentRangeEnd" | "commentReference" => Keep::Drop,
            "ins" | "moveTo" if accept_now && e.name.starts_with("w:") => Keep::Unwrap,
            "del" | "moveFrom" | "moveFromRangeStart" | "moveFromRangeEnd" | "moveToRangeStart"
            | "moveToRangeEnd" | "rPrChange" | "pPrChange" | "sectPrChange" | "tblPrChange"
            | "trPrChange" | "tcPrChange" | "tblGridChange" | "numberingChange"
                if accept_now && e.name.starts_with("w:") =>
            {
                Keep::Drop
            }
            _ => Keep::Yes,
        });
        // Editing-session ids (rsid*) say when and in which session text was typed.
        d.root
            .walk_mut(&mut |e| e.attrs.retain(|(k, _)| !k.starts_with("w:rsid")));
        let after = d.write()?;
        if after != before {
            pkg.set(&part, after);
            note("edit history", &mut done);
        }
    }
    if let Some(mut s) = pkg.xml("word/settings.xml") {
        s.root.filter(&|e| match e.local() {
            "rsids" => Keep::Drop,
            "trackRevisions" if accept => Keep::Drop,
            _ => Keep::Yes,
        });
        pkg.set_xml("word/settings.xml", &s)?;
    }
    if accept {
        note("tracked changes (accepted)", &mut done);
    }
    std::fs::write(out, pkg.write()?)?;
    Ok(done)
}

// ------------------------------------------------------------ pictures, stats

/// Copy the document's pictures into `dir`, in document order of their part names.
pub fn extract_images(input: &Path, dir: &Path, stem: &str) -> Result<Vec<PathBuf>, EngineError> {
    let pkg = Pkg::read(&std::fs::read(input)?)?;
    let mut outs = Vec::new();
    let mut media: Vec<&(String, Vec<u8>)> = pkg
        .parts
        .iter()
        .filter(|(n, _)| n.starts_with("word/media/"))
        .collect();
    media.sort_by_key(|(n, _)| {
        let digits: String = n.chars().filter(char::is_ascii_digit).collect();
        (digits.parse::<u64>().unwrap_or(u64::MAX), n.clone())
    });
    for (i, (name, bytes)) in media.into_iter().enumerate() {
        let ext = name
            .rsplit('.')
            .next()
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let ext: String = ext
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(5)
            .collect();
        let out = dir.join(format!("{stem} image {:03}.{ext}", i + 1));
        std::fs::write(&out, bytes)?;
        outs.push(out);
    }
    if outs.is_empty() {
        return Err(EngineError::Invalid("this document has no pictures".into()));
    }
    Ok(outs)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stats {
    pub words: usize,
    pub chars: usize,
    pub chars_no_spaces: usize,
    pub paragraphs: usize,
    pub pages: usize,
}

/// Word, character, paragraph and page counts (pages from our own layout).
pub fn stats(input: &Path) -> Result<Stats, EngineError> {
    let bytes = std::fs::read(input)?;
    let pkg = Pkg::read(&bytes)?;
    let d = pkg
        .xml("word/document.xml")
        .ok_or_else(|| EngineError::Decode("the document body can't be read".into()))?;
    let mut s = Stats::default();
    fn walk(e: &Elem, s: &mut Stats) {
        if e.local() == "p" && e.name.starts_with("w:") {
            let t: String = para_text(e);
            if !t.trim().is_empty() {
                s.paragraphs += 1;
                s.words += t.split_whitespace().count();
                s.chars += t.chars().count();
                s.chars_no_spaces += t.chars().filter(|c| !c.is_whitespace()).count();
            }
            return;
        }
        for c in e.children() {
            walk(c, s);
        }
    }
    walk(&d.root, &mut s);
    s.pages = crate::docxlayout::page_count(&bytes).unwrap_or(0);
    Ok(s)
}

/// A paragraph's visible text (`w:t`, tabs and breaks), not deleted text.
fn para_text(p: &Elem) -> String {
    let mut s = String::new();
    fn walk(e: &Elem, s: &mut String) {
        match e.local() {
            "t" => s.push_str(&e.text()),
            "tab" => s.push('\t'),
            "br" | "cr" => s.push('\n'),
            "del" | "instrText" | "delText" => {}
            _ => {
                for c in e.children() {
                    walk(c, s);
                }
            }
        }
    }
    walk(p, &mut s);
    s
}

// ------------------------------------------------------------ find and replace

/// Paths (child indices) to the `w:t` elements of a paragraph, with their text.
fn text_nodes(p: &Elem) -> Vec<(Vec<usize>, String)> {
    let mut out = Vec::new();
    fn walk(e: &Elem, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, String)>) {
        for (i, k) in e.kids.iter().enumerate() {
            if let XNode::Elem(c) = k {
                if matches!(c.local(), "del" | "instrText" | "delText") {
                    continue;
                }
                path.push(i);
                if c.local() == "t" {
                    out.push((path.clone(), c.text()));
                } else {
                    walk(c, path, out);
                }
                path.pop();
            }
        }
    }
    walk(p, &mut Vec::new(), &mut out);
    out
}

fn at_path<'a>(e: &'a mut Elem, path: &[usize]) -> Option<&'a mut Elem> {
    let mut cur = e;
    for &i in path {
        cur = match cur.kids.get_mut(i)? {
            XNode::Elem(c) => c,
            _ => return None,
        };
    }
    Some(cur)
}

fn set_text(t: &mut Elem, s: &str) {
    t.kids = if s.is_empty() {
        Vec::new()
    } else {
        vec![XNode::Text(s.to_string())]
    };
    t.empty = s.is_empty();
    if s.starts_with(' ') || s.ends_with(' ') || s.contains("  ") {
        t.set_attr("xml:space", "preserve");
    }
}

/// The next match at or after a position: (start, end, replacement).
type Finder<'a> = &'a dyn Fn(&str, usize) -> Option<(usize, usize, String)>;

/// Replace in one paragraph; returns the number of replacements. `find` returns the next
/// match (byte range) at or after a position in the paragraph's text.
fn replace_in_para(p: &mut Elem, find: Finder<'_>) -> usize {
    let nodes = text_nodes(p);
    if nodes.is_empty() {
        return 0;
    }
    let full: String = nodes.iter().map(|n| n.1.as_str()).collect();
    // Matches first, then edit from the end so earlier offsets stay valid.
    let mut matches = Vec::new();
    let mut pos = 0;
    while let Some((a, b, rep)) = find(&full, pos) {
        if b <= a {
            break;
        }
        matches.push((a, b, rep));
        pos = b;
    }
    if matches.is_empty() {
        return 0;
    }
    // Each node's span in the full text.
    let mut spans = Vec::with_capacity(nodes.len());
    let mut o = 0;
    for (_, t) in &nodes {
        spans.push((o, o + t.len()));
        o += t.len();
    }
    let mut texts: Vec<String> = nodes.iter().map(|n| n.1.clone()).collect();
    for (a, b, rep) in matches.iter().rev() {
        let first = spans
            .iter()
            .position(|&(s, e)| *a >= s && *a < e.max(s + 1))
            .unwrap_or(0);
        let last = spans
            .iter()
            .position(|&(s, e)| *b > s && *b <= e)
            .unwrap_or(first);
        for k in (first..=last).rev() {
            let (s, _) = spans[k];
            let t = &texts[k];
            let from = a.saturating_sub(s).min(t.len());
            let to = (b - s).min(t.len());
            let mut n = String::new();
            n.push_str(&t[..from]);
            if k == first {
                n.push_str(rep);
            }
            n.push_str(&t[to..]);
            texts[k] = n;
        }
    }
    for ((path, old), new) in nodes.iter().zip(&texts) {
        if old != new
            && let Some(t) = at_path(p, path)
        {
            set_text(t, new);
        }
    }
    matches.len()
}

fn each_para(e: &mut Elem, f: &mut dyn FnMut(&mut Elem)) {
    if e.local() == "p" && e.name.starts_with("w:") {
        f(e);
        // Text boxes inside a paragraph hold paragraphs of their own.
    }
    for c in e.children_mut() {
        each_para(c, f);
    }
}

/// Find and replace across the whole document (body, headers, footers, notes), also
/// where Word split a word across runs. Returns the number of replacements.
pub fn replace(
    input: &Path,
    out: &Path,
    find: &str,
    with: &str,
    match_case: bool,
    whole_word: bool,
) -> Result<usize, EngineError> {
    if find.is_empty() {
        return Err(EngineError::Invalid("type what to find".into()));
    }
    let mut pkg = Pkg::read(&std::fs::read(input)?)?;
    let needle = if match_case {
        find.to_string()
    } else {
        find.to_lowercase()
    };
    let matcher = |hay: &str, from: usize| -> Option<(usize, usize, String)> {
        let h = if match_case {
            hay.to_string()
        } else {
            hay.to_lowercase()
        };
        // Lowercasing can change byte lengths; fall back to exact search then.
        let (h, n) = if h.len() == hay.len() {
            (h, needle.clone())
        } else {
            (hay.to_string(), find.to_string())
        };
        let mut start = from;
        while let Some(i) = h.get(start..).and_then(|s| s.find(&n)) {
            let a = start + i;
            let b = a + n.len();
            let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !whole_word || (!word(hay[..a].chars().last()) && !word(hay[b..].chars().next())) {
                return Some((a, b, with.to_string()));
            }
            start = a + n.len().max(1);
        }
        None
    };
    let mut count = 0;
    for part in pkg.story_parts() {
        let Some(mut d) = pkg.xml(&part) else {
            continue;
        };
        let mut n = 0;
        each_para(&mut d.root, &mut |p| n += replace_in_para(p, &matcher));
        if n > 0 {
            pkg.set_xml(&part, &d)?;
            count += n;
        }
    }
    std::fs::write(out, pkg.write()?)?;
    Ok(count)
}

// ------------------------------------------------------------ mail merge

/// Rows of a data file (CSV, TSV or a spreadsheet's first sheet): header → value.
pub fn data_rows(path: &Path) -> Result<Vec<BTreeMap<String, String>>, EngineError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let table: Vec<Vec<String>> = match ext.as_str() {
        "xlsx" | "xls" | "ods" => crate::sheet::read_sheet(path)?
            .iter()
            .map(|r| r.iter().map(crate::sheet::cell_text).collect())
            .collect(),
        _ => {
            let text = crate::text_from_bytes(&std::fs::read(path)?);
            let delim = if ext == "tsv" {
                b'\t'
            } else {
                crate::data::sniff_delimiter(&text)
            };
            let mut rdr = csv::ReaderBuilder::new()
                .delimiter(delim)
                .has_headers(false)
                .flexible(true)
                .from_reader(text.as_bytes());
            rdr.records()
                .filter_map(Result::ok)
                .map(|r| r.iter().map(str::to_string).collect())
                .collect()
        }
    };
    let mut it = table.into_iter();
    let header: Vec<String> = it
        .next()
        .ok_or_else(|| EngineError::Invalid("the data file is empty".into()))?
        .into_iter()
        .map(|h| h.trim().to_string())
        .collect();
    Ok(it
        .filter(|r| r.iter().any(|c| !c.trim().is_empty()))
        .map(|r| {
            header
                .iter()
                .enumerate()
                .filter(|(_, h)| !h.is_empty())
                .map(|(i, h)| (h.clone(), r.get(i).cloned().unwrap_or_default()))
                .collect()
        })
        .collect())
}

/// Remove MERGEFIELD field codes (keeping their results), so placeholders like «Name»
/// are plain text that the merge replaces.
fn strip_merge_fields(root: &mut Elem) {
    // Simple fields: unwrap.
    root.filter(&|e| {
        if e.local() == "fldSimple"
            && e.attr("w:instr")
                .is_some_and(|i| i.to_ascii_uppercase().contains("MERGEFIELD"))
        {
            Keep::Unwrap
        } else {
            Keep::Yes
        }
    });
    // Complex fields: drop the begin/instr/separate/end runs of MERGEFIELDs.
    each_para(root, &mut |p| {
        let mut drop = vec![false; p.kids.len()];
        let mut state: Option<(Vec<usize>, String, bool)> = None;
        for (i, k) in p.kids.iter().enumerate() {
            let XNode::Elem(r) = k else { continue };
            if r.local() != "r" {
                continue;
            }
            let fc = r
                .children()
                .find(|c| c.local() == "fldChar")
                .and_then(|c| c.attr("w:fldCharType"))
                .map(str::to_string);
            let instr: String = r
                .children()
                .filter(|c| c.local() == "instrText")
                .map(Elem::text)
                .collect();
            match fc.as_deref() {
                Some("begin") => state = Some((vec![i], String::new(), false)),
                Some("separate") => {
                    if let Some(s) = &mut state {
                        s.0.push(i);
                        s.2 = true;
                    }
                }
                Some("end") => {
                    if let Some((mut runs, code, _)) = state.take() {
                        runs.push(i);
                        if code.to_ascii_uppercase().contains("MERGEFIELD") {
                            for r in runs {
                                drop[r] = true;
                            }
                        }
                    }
                }
                _ => {
                    if let Some(s) = &mut state
                        && !s.2
                        && !instr.is_empty()
                    {
                        s.0.push(i);
                        s.1.push_str(&instr);
                    }
                }
            }
        }
        let mut i = 0;
        p.kids.retain(|_| {
            let keep = !drop[i];
            i += 1;
            keep
        });
    });
}

/// Fill a template for one row: `{{Name}}` and `«Name»` placeholders (and Word merge
/// fields) get the row's values; unknown names become empty.
pub fn fill(template: &[u8], row: &BTreeMap<String, String>) -> Result<Vec<u8>, EngineError> {
    let mut pkg = Pkg::read(template)?;
    let lower: HashMap<String, &String> = row.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
    let matcher = |hay: &str, from: usize| -> Option<(usize, usize, String)> {
        let rest = hay.get(from..)?;
        let a = rest
            .find("{{")
            .map(|i| (i, "{{", "}}"))
            .into_iter()
            .chain(rest.find('«').map(|i| (i, "«", "»")))
            .min_by_key(|x| x.0)?;
        let start = from + a.0;
        let inner_from = start + a.1.len();
        let end_rel = hay[inner_from..].find(a.2)?;
        let name = hay[inner_from..inner_from + end_rel].trim().to_lowercase();
        if name.is_empty() || name.len() > 80 {
            return None;
        }
        let end = inner_from + end_rel + a.2.len();
        Some((
            start,
            end,
            lower.get(&name).map(|v| v.to_string()).unwrap_or_default(),
        ))
    };
    for part in pkg.story_parts() {
        let Some(mut d) = pkg.xml(&part) else {
            continue;
        };
        strip_merge_fields(&mut d.root);
        each_para(&mut d.root, &mut |p| {
            replace_in_para(p, &matcher);
        });
        pkg.set_xml(&part, &d)?;
    }
    if let Some(mut s) = pkg.xml("word/settings.xml") {
        // The result is a finished letter, not a merge main document.
        s.root.filter(&|e| {
            if e.local() == "mailMerge" {
                Keep::Drop
            } else {
                Keep::Yes
            }
        });
        pkg.set_xml("word/settings.xml", &s)?;
    }
    pkg.write()
}

/// A cell value usable in a file name: no reserved or control characters, spaces
/// collapsed, at most 60 characters.
pub(crate) fn safe_label(v: &str) -> String {
    v.chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .chars()
        .take(60)
        .collect()
}

/// One filled document per row into `dir`, named from `name_field` (or numbered). With
/// `pdf`, a PDF per row as well.
pub fn mail_merge(
    template: &Path,
    rows: &[BTreeMap<String, String>],
    dir: &Path,
    stem: &str,
    name_field: Option<&str>,
    pdf: bool,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let tpl = std::fs::read(template)?;
    if rows.is_empty() {
        return Err(EngineError::Invalid("the data file has no rows".into()));
    }
    let mut outs = Vec::new();
    let mut used = std::collections::HashSet::new();
    let digits = rows.len().to_string().len().max(3);
    for (i, row) in rows.iter().enumerate() {
        cancel.check()?;
        let bytes = fill(&tpl, row)?;
        let label = name_field
            .and_then(|f| {
                row.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(f))
                    .map(|(_, v)| v.clone())
            })
            .map(|v| safe_label(&v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("{:0digits$}", i + 1));
        let mut name = format!("{stem} {label}");
        let mut k = 2;
        while !used.insert(name.to_lowercase()) {
            name = format!("{stem} {label} ({k})");
            k += 1;
        }
        let docx = dir.join(format!("{name}.docx"));
        std::fs::write(&docx, &bytes)?;
        outs.push(docx);
        if pdf {
            let p = dir.join(format!("{name}.pdf"));
            crate::docxlayout::convert(&bytes, &name, &p, cancel)?;
            outs.push(p);
        }
    }
    Ok(outs)
}

#[cfg(test)]
mod tests;
