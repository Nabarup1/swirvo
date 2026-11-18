//! True redaction: what's blacked out is gone from the file, not just covered.
//!
//! A page with anything to redact is flattened: it's rendered to a picture, the boxes are
//! painted black into the pixels, and the page's content (text, fonts, vector art,
//! pictures, links, comments) is replaced by that one picture. Pages without matches keep
//! their content untouched. With OCR, flattened pages get a fresh invisible text layer
//! read from the redacted picture, so they stay searchable without the removed words.
//!
//! Matches are found in the PDF's text and, on pages with pictures or no text (scans),
//! by OCR. The file's metadata, bookmarks, comments and form values are scrubbed too.
//! The result is read back and checked; if a match survives, nothing is written.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use swirvo_core::{CancelToken, EngineError};

use super::geometry::{Box4, Geom, cm};
use super::stamp;
use crate::ocr;
use crate::pdf_tools::PageRenderer;
use crate::pdflayout::extract::{self, RawGlyph};

/// Flattened pages are rendered at this resolution.
const DPI: f32 = 200.0;

/// What to look for.
#[derive(Debug, Clone, Default)]
pub struct Find {
    /// Words or phrases (a space in a phrase matches any space or line break).
    pub terms: Vec<String>,
    pub emails: bool,
    pub phones: bool,
    pub match_case: bool,
}

impl Find {
    fn is_empty(&self) -> bool {
        self.terms.iter().all(|t| t.trim().is_empty()) && !self.emails && !self.phones
    }

    /// Character ranges of every match in `s`.
    fn matches(&self, s: &[char]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for t in &self.terms {
            let t: Vec<char> = t.trim().chars().collect();
            if !t.is_empty() {
                out.extend(find_term(s, &t, self.match_case));
            }
        }
        if self.emails {
            out.extend(find_emails(s));
        }
        if self.phones {
            out.extend(find_phones(s));
        }
        out
    }

    fn hits(&self, text: &str) -> bool {
        !self.matches(&text.chars().collect::<Vec<_>>()).is_empty()
    }
}

/// A box to black out whatever is in it: 0-based page, display points (y up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub page: usize,
    pub rect: Box4,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Redacted {
    /// Pages flattened.
    pub pages: usize,
    /// Matches blacked out.
    pub hits: usize,
    /// 1-based pages where a match was in text whose position couldn't be worked out
    /// (rotated, or not drawn at all). Those pages were flattened anyway, which removes
    /// the text, but nothing was painted: the reader should look at them.
    pub unplaced: Vec<usize>,
    pub searchable: bool,
}

/// Redact `find` and `areas` from `input` into `out`. With `searchable` (and an OCR
/// engine), flattened pages get an invisible text layer back.
pub fn redact(
    input: &Path,
    out: &Path,
    find: &Find,
    areas: &[Area],
    searchable: bool,
    cancel: &CancelToken,
) -> Result<Redacted, EngineError> {
    if find.is_empty() && areas.is_empty() {
        return Err(EngineError::Invalid("say what to redact".into()));
    }
    let bytes = std::fs::read(input)?;
    let mut doc = super::open_bytes(&bytes, None)?;
    let ids = super::pages(&doc);
    let ex = extract::extract(&bytes, cancel)?;
    let renderer = PageRenderer::open(input)?;
    let can_ocr = ocr::available();
    let mut rep = Redacted::default();

    // 1. Where: boxes per page (display points, y up).
    let mut boxes: BTreeMap<usize, Vec<Box4>> = BTreeMap::new();
    for a in areas {
        if a.page >= ids.len() {
            return Err(EngineError::Invalid(format!("no page {}", a.page + 1)));
        }
        boxes.entry(a.page).or_default().push(a.rect);
    }
    // Pages already rendered for OCR that will be flattened.
    let mut rendered: BTreeMap<usize, image::RgbaImage> = BTreeMap::new();
    renderer.session(|s| -> Result<(), EngineError> {
        for (i, &id) in ids.iter().enumerate() {
            cancel.check()?;
            let (pw, ph) = Geom::of(&doc, id).display_size();
            if let Some(p) = ex.pages.get(i) {
                let found = glyph_boxes(&p.glyphs, p.h, find);
                rep.hits += found.len();
                if !found.is_empty() {
                    boxes.entry(i).or_default().extend(found);
                }
            }
            let scan_like = ex
                .pages
                .get(i)
                .is_none_or(|p| p.glyphs.is_empty() || !p.images.is_empty());
            if can_ocr && scan_like && !find.is_empty() {
                let img = s.render_dpi(i, DPI)?;
                let lines = ocr::recognize_lines(&img)?;
                let found = ocr_boxes(&lines, img.dimensions(), (pw, ph), find);
                rep.hits += found.len();
                if !found.is_empty() {
                    boxes.entry(i).or_default().extend(found);
                    // Kept only for pages that will be flattened (a scan can be long).
                    rendered.insert(i, img);
                }
            }
        }
        Ok(())
    })?;
    // Text the extractor couldn't place (rotated, clipped away, never drawn) but that is
    // in the file: those pages are flattened too, so the text is gone.
    for (i, _) in ids.iter().enumerate() {
        if boxes.contains_key(&i) || find.is_empty() {
            continue;
        }
        let raw = doc.extract_text(&[i as u32 + 1]).unwrap_or_default();
        if find.hits(&raw) {
            boxes.entry(i).or_default();
            rep.unplaced.push(i + 1);
        }
    }
    if boxes.is_empty() {
        return Err(EngineError::Invalid(
            "nothing matched, so nothing was redacted".into(),
        ));
    }

    // 2. Flatten each page with boxes.
    let mut layers: BTreeMap<usize, (Vec<ocr::Line>, (u32, u32))> = BTreeMap::new();
    renderer.session(|s| -> Result<(), EngineError> {
        for (&i, bs) in &boxes {
            cancel.check()?;
            let geom = Geom::of(&doc, ids[i]);
            let (pw, ph) = geom.display_size();
            let mut img = match rendered.remove(&i) {
                Some(img) => img,
                None => s.render_dpi(i, DPI)?,
            };
            black_out(&mut img, bs, pw, ph);
            if searchable && can_ocr {
                layers.insert(i, (ocr::recognize_lines(&img)?, img.dimensions()));
            }
            flatten(&mut doc, ids[i], &geom, img)?;
        }
        Ok(())
    })?;
    rep.pages = boxes.len();

    // 3. Everything else that can carry the words.
    if !find.is_empty() {
        scrub(&mut doc, find, &boxes.keys().map(|&i| ids[i]).collect());
    }
    if !layers.is_empty() {
        let sel: Vec<usize> = layers.keys().copied().collect();
        stamp::stamp_doc(&mut doc, Some(&sel), cancel, |p| {
            layers
                .get(&p.index)
                .map(|(l, (iw, ih))| super::searchable::layer(l, *iw, *ih, p.w, p.h))
                .unwrap_or_default()
        })?;
        rep.searchable = true;
    }
    doc.prune_objects();
    super::save(&mut doc, out, true)?;

    // 4. Check the result as a reader would see it.
    if !find.is_empty() {
        verify(out, find, cancel)?;
    }
    Ok(rep)
}

/// Fail if any match is still in the written file.
fn verify(out: &Path, find: &Find, cancel: &CancelToken) -> Result<(), EngineError> {
    let bytes = std::fs::read(out)?;
    let ex = extract::extract(&bytes, cancel)?;
    let doc = super::open_bytes(&bytes, None)?;
    for (i, p) in ex.pages.iter().enumerate() {
        let left = glyph_boxes(&p.glyphs, p.h, find).len();
        let raw = doc.extract_text(&[i as u32 + 1]).unwrap_or_default();
        if left > 0 || find.hits(&raw) {
            return Err(EngineError::Encode(format!(
                "a match was still readable on page {} after redacting, so nothing was saved",
                i + 1
            )));
        }
    }
    Ok(())
}

/// Page text in drawing order with spaces and line breaks inferred from positions, and the
/// glyph each character came from.
fn page_text(g: &[RawGlyph]) -> (Vec<char>, Vec<Option<usize>>) {
    let mut s = Vec::with_capacity(g.len() + g.len() / 4);
    let mut from = Vec::with_capacity(s.capacity());
    for (k, c) in g.iter().enumerate() {
        if let Some(p) = k.checked_sub(1).map(|j| &g[j]) {
            let size = p.size.max(c.size).max(1.0);
            if (c.base - p.base).abs() > 0.4 * size || c.x0 + 0.5 * size < p.x0 {
                s.push('\n');
                from.push(None);
            } else if c.x0 - p.x1 > 0.2 * size && c.ch != ' ' && p.ch != ' ' {
                s.push(' ');
                from.push(None);
            }
        }
        s.push(c.ch);
        from.push(Some(k));
    }
    (s, from)
}

/// Boxes (display points, y up) around every match in a page's glyphs. `h` is the page
/// height in the extractor's y-down space.
fn glyph_boxes(g: &[RawGlyph], h: f32, find: &Find) -> Vec<Box4> {
    let (s, from) = page_text(g);
    let mut out = Vec::new();
    for (a, b) in find.matches(&s) {
        // One box per line the match spans.
        let mut line: Option<Box4> = None;
        let mut last_base = f32::NAN;
        for k in from[a..b].iter().flatten() {
            let c = &g[*k];
            let pad = 0.06 * c.size;
            let r = [
                c.x0 - pad,
                h - (c.base + 0.28 * c.size),
                c.x1 + pad,
                h - (c.base - 0.92 * c.size),
            ];
            match &mut line {
                Some(l) if (c.base - last_base).abs() <= 0.4 * c.size => {
                    l[0] = l[0].min(r[0]);
                    l[1] = l[1].min(r[1]);
                    l[2] = l[2].max(r[2]);
                    l[3] = l[3].max(r[3]);
                }
                _ => {
                    if let Some(l) = line.take() {
                        out.push(l);
                    }
                    line = Some(r);
                }
            }
            last_base = c.base;
        }
        out.extend(line);
    }
    out
}

/// Boxes around matches in OCR'd lines: pixels (y down) → display points (y up).
fn ocr_boxes(
    lines: &[ocr::Line],
    (iw, ih): (u32, u32),
    (pw, ph): (f32, f32),
    find: &Find,
) -> Vec<Box4> {
    let kx = pw / iw.max(1) as f32;
    let ky = ph / ih.max(1) as f32;
    let mut out = Vec::new();
    for l in lines {
        let mut s = Vec::new();
        let mut from = Vec::new();
        for (j, w) in l.words.iter().enumerate() {
            if j > 0 {
                s.push(' ');
                from.push(None);
            }
            for c in w.text.chars() {
                s.push(c);
                from.push(Some(j));
            }
        }
        for (a, b) in find.matches(&s) {
            let words: BTreeSet<usize> = from[a..b].iter().flatten().copied().collect();
            let mut r: Option<Box4> = None;
            for j in words {
                let w = &l.words[j];
                let pad = 0.15 * w.h;
                let b = [
                    (w.x - pad) * kx,
                    ph - (w.y + w.h + pad) * ky,
                    (w.x + w.w + pad) * kx,
                    ph - (w.y - pad) * ky,
                ];
                r = Some(match r {
                    Some(r) => [
                        r[0].min(b[0]),
                        r[1].min(b[1]),
                        r[2].max(b[2]),
                        r[3].max(b[3]),
                    ],
                    None => b,
                });
            }
            out.extend(r);
        }
    }
    out
}

/// Paint the boxes solid black into the rendered page.
fn black_out(img: &mut image::RgbaImage, boxes: &[Box4], pw: f32, ph: f32) {
    let (iw, ih) = img.dimensions();
    let kx = iw as f32 / pw.max(1.0);
    let ky = ih as f32 / ph.max(1.0);
    for b in boxes {
        let x0 = ((b[0] * kx).floor() as i64 - 1).clamp(0, iw as i64) as u32;
        let x1 = ((b[2] * kx).ceil() as i64 + 1).clamp(0, iw as i64) as u32;
        let y0 = (((ph - b[3]) * ky).floor() as i64 - 1).clamp(0, ih as i64) as u32;
        let y1 = (((ph - b[1]) * ky).ceil() as i64 + 1).clamp(0, ih as i64) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
    }
}

/// Replace the page's content by the picture of it.
fn flatten(
    doc: &mut Document,
    page: ObjectId,
    geom: &Geom,
    img: image::RgbaImage,
) -> Result<(), EngineError> {
    let (w, h) = img.dimensions();
    // Black-and-white pages (most documents) are stored as gray: a third of the size.
    let gray = img.pixels().all(|p| p[0] == p[1] && p[1] == p[2]);
    let dynimg = if gray {
        image::DynamicImage::ImageLuma8(image::DynamicImage::ImageRgba8(img).to_luma8())
    } else {
        image::DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(img).to_rgb8())
    };
    let mut dict = Dictionary::new();
    dict.set("Type", Object::Name(b"XObject".to_vec()));
    dict.set("Subtype", Object::Name(b"Image".to_vec()));
    dict.set("Width", Object::Integer(w as i64));
    dict.set("Height", Object::Integer(h as i64));
    dict.set(
        "ColorSpace",
        Object::Name(if gray {
            b"DeviceGray".to_vec()
        } else {
            b"DeviceRGB".to_vec()
        }),
    );
    dict.set("BitsPerComponent", Object::Integer(8));
    let image = if super::compress::photographic(&dynimg) {
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 85)
            .encode_image(&dynimg)
            .map_err(EngineError::encode)?;
        dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        Stream::new(dict, jpg)
    } else {
        let mut st = Stream::new(dict, dynimg.into_bytes());
        let _ = st.compress();
        st
    };
    let im = doc.add_object(image);
    let (pw, ph) = geom.display_size();
    let content = format!("q {} {pw} 0 0 {ph} 0 0 cm /Rd0 Do Q", cm(geom.to_user()));
    let contents = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));
    let mut xo = Dictionary::new();
    xo.set("Rd0", Object::Reference(im));
    let mut res = Dictionary::new();
    res.set("XObject", Object::Dictionary(xo));
    let p = doc
        .get_object_mut(page)
        .and_then(Object::as_dict_mut)
        .map_err(EngineError::decode)?;
    p.set("Contents", Object::Reference(contents));
    p.set("Resources", Object::Dictionary(res));
    for k in [
        &b"Annots"[..],
        b"Thumb",
        b"PieceInfo",
        b"Metadata",
        b"B",
        b"StructParents",
    ] {
        p.remove(k);
    }
    Ok(())
}

fn string_hits(o: &Object, find: &Find) -> bool {
    match o {
        Object::String(b, _) => find.hits(&decode_text(b)),
        _ => false,
    }
}

/// A PDF text string (UTF-16BE with a BOM, or PDFDocEncoding ≈ Latin-1).
fn decode_text(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let (pairs, _) = rest.as_chunks::<2>();
        String::from_utf16_lossy(
            &pairs
                .iter()
                .map(|p| u16::from_be_bytes(*p))
                .collect::<Vec<_>>(),
        )
    } else {
        b.iter().map(|&c| c as char).collect()
    }
}

/// Remove matches from the document's metadata, bookmarks, comments and form values.
fn scrub(doc: &mut Document, find: &Find, flattened: &BTreeSet<ObjectId>) {
    // Document info: drop any entry that matches. XMP is a copy of it: drop it.
    if let Ok(Object::Reference(info)) = doc.trailer.get(b"Info").cloned()
        && let Ok(Object::Dictionary(d)) = doc.get_object_mut(info)
    {
        let bad: Vec<Vec<u8>> = d
            .iter()
            .filter(|(_, v)| string_hits(v, find))
            .map(|(k, _)| k.clone())
            .collect();
        for k in bad {
            d.remove(&k);
        }
    }
    if let Ok(cat) = doc.catalog_mut() {
        cat.remove(b"Metadata");
    }
    // Every dictionary: bookmark titles, comments' text, form values.
    let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
    for id in ids {
        let Some(Object::Dictionary(d)) = doc.objects.get_mut(&id) else {
            continue;
        };
        for key in [
            &b"Title"[..],
            b"Contents",
            b"RC",
            b"Subj",
            b"T",
            b"TU",
            b"V",
            b"DV",
        ] {
            if d.get(key).is_ok_and(|v| string_hits(v, find)) {
                d.set(key, Object::string_literal("[redacted]"));
                d.remove(b"AP");
            }
        }
    }
    // Comments on pages that weren't flattened, whose text matched: gone entirely.
    for page in super::pages(doc) {
        if flattened.contains(&page) {
            continue;
        }
        let annots = super::inherited(doc, page, b"Annots").map(|o| super::deref(doc, o).clone());
        let Some(Object::Array(list)) = annots else {
            continue;
        };
        let keep: Vec<Object> = list
            .into_iter()
            .filter(|a| {
                let d = match a {
                    Object::Reference(r) => doc.get_dictionary(*r).ok(),
                    Object::Dictionary(d) => Some(d),
                    _ => None,
                };
                !d.is_some_and(|d| {
                    d.get(b"Contents").is_ok_and(
                        |c| matches!(c, Object::String(s, _) if decode_text(s) == "[redacted]"),
                    )
                })
            })
            .collect();
        if let Ok(p) = doc.get_object_mut(page).and_then(Object::as_dict_mut) {
            p.set("Annots", Object::Array(keep));
        }
    }
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn find_term(s: &[char], t: &[char], case: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let (mut a, mut b) = (i, 0);
        while b < t.len() && a < s.len() {
            let (x, y) = (s[a], t[b]);
            if y.is_whitespace() {
                if !x.is_whitespace() {
                    break;
                }
                while a < s.len() && s[a].is_whitespace() {
                    a += 1;
                }
                while b < t.len() && t[b].is_whitespace() {
                    b += 1;
                }
                continue;
            }
            if x == y || (!case && fold(x) == fold(y)) {
                a += 1;
                b += 1;
            } else {
                break;
            }
        }
        if b == t.len() {
            out.push((i, a));
            i = a.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

fn find_emails(s: &[char]) -> Vec<(usize, usize)> {
    let local = |c: char| c.is_alphanumeric() || "._%+-'".contains(c);
    let domain = |c: char| c.is_alphanumeric() || c == '.' || c == '-';
    let mut out = Vec::new();
    for (i, &c) in s.iter().enumerate() {
        if c != '@' {
            continue;
        }
        let mut a = i;
        while a > 0 && local(s[a - 1]) {
            a -= 1;
        }
        let mut b = i + 1;
        while b < s.len() && domain(s[b]) {
            b += 1;
        }
        while b > i + 1 && matches!(s[b - 1], '.' | '-') {
            b -= 1;
        }
        let dom: String = s[i + 1..b].iter().collect();
        let tld_ok = dom
            .rsplit('.')
            .next()
            .is_some_and(|t| t.len() >= 2 && t.chars().all(char::is_alphabetic));
        if a < i && dom.contains('.') && tld_ok {
            out.push((a, b));
        }
    }
    out
}

/// Phone numbers: 9 to 15 digits with the usual separators, or 7+ after a `+` or `(`.
fn find_phones(s: &[char]) -> Vec<(usize, usize)> {
    let sep = |c: char| matches!(c, ' ' | '-' | '.' | '(' | ')' | '/' | '\u{a0}');
    let mut out = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        let starts = c.is_ascii_digit()
            || ((c == '+' || c == '(') && s.get(i + 1).is_some_and(|d| d.is_ascii_digit()));
        if !starts || (i > 0 && s[i - 1].is_alphanumeric()) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut digits = usize::from(c.is_ascii_digit());
        let mut last_digit = i;
        let mut run = 0;
        while j < s.len() {
            let d = s[j];
            if d.is_ascii_digit() {
                digits += 1;
                last_digit = j;
                run = 0;
            } else if sep(d) && run < 2 {
                run += 1;
            } else {
                break;
            }
            j += 1;
        }
        let end = last_digit + 1;
        let lead = c == '+' || c == '(';
        let ok = (9..=15).contains(&digits) || (lead && (7..=15).contains(&digits));
        if ok && !s.get(end).is_some_and(|c| c.is_alphanumeric()) {
            out.push((i, end));
            i = end;
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn finds_terms_emails_and_phones() {
        let f = Find {
            terms: vec!["project zebra".into()],
            emails: true,
            phones: true,
            match_case: false,
        };
        let s = chars(
            "Call +44 20 7946 0958 or (555) 123-4567, mail ada.l@example.co.uk\nabout Project\nZebra on 2024-01-15.",
        );
        let m = f.matches(&s);
        let got: Vec<String> = m.iter().map(|&(a, b)| s[a..b].iter().collect()).collect();
        assert!(got.contains(&"Project\nZebra".to_string()), "{got:?}");
        assert!(got.contains(&"ada.l@example.co.uk".to_string()), "{got:?}");
        assert!(got.contains(&"+44 20 7946 0958".to_string()), "{got:?}");
        assert!(got.contains(&"(555) 123-4567".to_string()), "{got:?}");
        assert!(
            !got.iter().any(|g| g.contains("2024")),
            "dates aren't phones: {got:?}"
        );
        assert!(
            Find {
                terms: vec!["ZEBRA".into()],
                match_case: true,
                ..Find::default()
            }
            .matches(&s)
            .is_empty()
        );
    }

    #[test]
    fn redacted_text_is_gone_from_the_file() {
        if !crate::docxlayout::fonts::Fonts::available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("in.pdf");
        super::super::testing::sample(&src, 2, false);
        // Real text on both pages; the secret only on page 2.
        let text = |p: stamp::PageInfo| {
            let line = if p.index == 1 {
                "Agent Zebra is at ada@example.com today"
            } else {
                "Nothing to hide here"
            };
            vec![stamp::Item::Text {
                text: line.into(),
                size: 14.0,
                color: [0, 0, 0],
                opacity: 1.0,
                x: 72.0,
                y: 700.0,
                align: stamp::HAlign::Left,
                angle: 0.0,
                bold: false,
            }]
        };
        let with = d.path().join("text.pdf");
        stamp::stamp(&src, &with, None, &CancelToken::new(), text).unwrap();
        let find = Find {
            terms: vec!["zebra".into()],
            emails: true,
            ..Find::default()
        };
        let out = d.path().join("out.pdf");
        let r = redact(&with, &out, &find, &[], false, &CancelToken::new()).unwrap();
        assert_eq!((r.pages, r.hits), (1, 2), "{r:?}");
        assert!(r.unplaced.is_empty());
        let bytes = std::fs::read(&out).unwrap();
        let ex = extract::extract(&bytes, &CancelToken::new()).unwrap();
        let page = |i: usize| ex.pages[i].glyphs.iter().map(|g| g.ch).collect::<String>();
        assert!(page(0).contains("Nothing"), "untouched page keeps its text");
        assert!(
            page(1).is_empty(),
            "flattened page has no text left: {}",
            page(1)
        );
        // Black where the word was, on the picture of page 2.
        let img = PageRenderer::open(&out)
            .unwrap()
            .session(|s| s.render_dpi(1, 72.0))
            .unwrap();
        let w = stamp::text_width("Agent ", 14.0, false);
        let px = img.get_pixel((72.0 + w + 5.0) as u32, (842.0 - 704.0) as u32);
        assert!(px[0] < 30, "{px:?}");
        // Nothing matched elsewhere: an error, no output.
        let none = Find {
            terms: vec!["unicorn".into()],
            ..Find::default()
        };
        assert!(
            redact(
                &with,
                &d.path().join("x.pdf"),
                &none,
                &[],
                false,
                &CancelToken::new()
            )
            .is_err()
        );
    }
}
