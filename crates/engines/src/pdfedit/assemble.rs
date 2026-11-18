//! Pages: build a new PDF from pages of one or more PDFs (reorder, duplicate, insert,
//! blank pages), split in several ways, put several pages on one sheet (N-up, booklet),
//! and resize or crop pages in place.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use krilla::geom::{Size, Transform};
use krilla::page::PageSettings;
use krilla::pdf::{Pdf, PdfDocument};
use lopdf::Object;
use swirvo_core::{CancelToken, EngineError};

use super::geometry::{Box4, Geom, box_object, cm};
use super::{selection, wrap_with};

/// One page of the new document.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageSrc {
    /// Page `page` (0-based) of input file `file`.
    Page { file: usize, page: usize },
    /// An empty page of this size in points.
    Blank { w: f32, h: f32 },
}

pub(crate) fn load(path: &Path) -> Result<Arc<Pdf>, EngineError> {
    let bytes = std::fs::read(path)?;
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes);
    Pdf::new(data).map(Arc::new).map_err(|e| {
        EngineError::Decode(format!(
            "couldn't open this PDF ({e:?}); it may be damaged or password-protected"
        ))
    })
}

pub(crate) fn write(doc: krilla::Document, out: &Path) -> Result<(), EngineError> {
    let bytes = doc
        .finish()
        .map_err(|e| EngineError::Encode(format!("PDF: {e:?}")))?;
    std::fs::write(out, bytes)?;
    Ok(())
}

/// Build `out` from `plan`. Runs of consecutive pages from the same file are embedded
/// together, so shared fonts and images are written once. Returns the page count.
pub fn assemble(
    inputs: &[PathBuf],
    plan: &[PageSrc],
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    if plan.is_empty() {
        return Err(EngineError::Invalid("no pages to save".into()));
    }
    let pdfs: Vec<Arc<Pdf>> = inputs.iter().map(|p| load(p)).collect::<Result<_, _>>()?;
    let docs: Vec<PdfDocument> = pdfs.iter().map(|p| PdfDocument::new(p.clone())).collect();
    for src in plan {
        if let PageSrc::Page { file, page } = *src {
            let n = pdfs
                .get(file)
                .ok_or_else(|| EngineError::Invalid("no such file".into()))?
                .pages()
                .len();
            selection(Some(&[page]), n)?;
        }
    }
    let mut doc = krilla::Document::new();
    let mut i = 0;
    while i < plan.len() {
        cancel.check()?;
        match plan[i] {
            PageSrc::Blank { w, h } => {
                let settings = PageSettings::from_wh(w, h)
                    .ok_or_else(|| EngineError::Invalid("bad page size".into()))?;
                doc.start_page_with(settings).finish();
                i += 1;
            }
            PageSrc::Page { file, .. } => {
                let mut run = Vec::new();
                while let Some(PageSrc::Page { file: f, page }) = plan.get(i).copied() {
                    if f != file {
                        break;
                    }
                    run.push(page);
                    i += 1;
                }
                doc.embed_pdf_pages(&docs[file], &run);
            }
        }
    }
    write(doc, out)?;
    Ok(plan.len())
}

/// The plan for one file's pages in `order` (0-based; duplicates allowed).
pub fn reorder_plan(order: &[usize]) -> Vec<PageSrc> {
    order
        .iter()
        .map(|&page| PageSrc::Page { file: 0, page })
        .collect()
}

/// Insert all pages of `other` (file 1) after page `after` of file 0 (None = at the start).
pub fn insert_plan(n_main: usize, n_other: usize, after: Option<usize>) -> Vec<PageSrc> {
    let cut = after.map_or(0, |a| (a + 1).min(n_main));
    let main = |page| PageSrc::Page { file: 0, page };
    (0..cut)
        .map(main)
        .chain((0..n_other).map(|page| PageSrc::Page { file: 1, page }))
        .chain((cut..n_main).map(main))
        .collect()
}

pub fn page_count(path: &Path) -> Result<usize, EngineError> {
    Ok(load(path)?.pages().len())
}

/// Page sizes in points (as displayed).
pub fn page_sizes(path: &Path) -> Result<Vec<(f32, f32)>, EngineError> {
    Ok(load(path)?
        .pages()
        .iter()
        .map(|p| p.render_dimensions())
        .collect())
}

/// Split into files of `every` pages: `stem p01-05.pdf`, `stem p06-10.pdf`, …
pub fn split_every(
    input: &Path,
    dir: &Path,
    stem: &str,
    every: usize,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let n = page_count(input)?;
    let every = every.max(1);
    let ranges: Vec<(usize, usize)> = (0..n)
        .step_by(every)
        .map(|a| (a, (a + every).min(n)))
        .collect();
    write_ranges(input, dir, stem, n, &ranges, cancel)
}

/// Split at each top-level bookmark (chapter), naming files after the bookmarks.
pub fn split_by_bookmarks(
    input: &Path,
    dir: &Path,
    stem: &str,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let doc = super::open(input, None)?;
    let n = super::pages(&doc).len();
    let toc = doc
        .get_toc()
        .map_err(|_| EngineError::Invalid("this PDF has no bookmarks to split at".into()))?;
    let top = toc.toc.iter().map(|t| t.level).min().unwrap_or(1);
    let mut starts: Vec<(usize, String)> = toc
        .toc
        .iter()
        .filter(|t| t.level == top && t.page >= 1 && t.page <= n)
        .map(|t| (t.page - 1, t.title.clone()))
        .collect();
    starts.sort_by_key(|s| s.0);
    starts.dedup_by_key(|s| s.0);
    if starts.is_empty() {
        return Err(EngineError::Invalid(
            "this PDF has no bookmarks to split at".into(),
        ));
    }
    if starts[0].0 > 0 {
        starts.insert(0, (0, "Front matter".into()));
    }
    let mut outs = Vec::new();
    let pdf = load(input)?;
    let pdoc = PdfDocument::new(pdf);
    for (k, (a, title)) in starts.iter().enumerate() {
        cancel.check()?;
        let b = starts.get(k + 1).map_or(n, |s| s.0);
        let mut d = krilla::Document::new();
        d.embed_pdf_pages(&pdoc, &(*a..b).collect::<Vec<_>>());
        let name = format!("{stem} {:02} {}.pdf", k + 1, safe_name(title));
        let out = dir.join(name);
        write(d, &out)?;
        outs.push(out);
    }
    Ok(outs)
}

/// Split into parts no bigger than `max_bytes` each (for email attachment limits). A
/// single page bigger than the limit becomes a part of its own.
pub fn split_by_size(
    input: &Path,
    dir: &Path,
    stem: &str,
    max_bytes: u64,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let pdf = load(input)?;
    let n = pdf.pages().len();
    let pdoc = PdfDocument::new(pdf);
    let size_of = |a: usize, b: usize| -> Result<u64, EngineError> {
        let mut d = krilla::Document::new();
        d.embed_pdf_pages(&pdoc, &(a..b).collect::<Vec<_>>());
        Ok(d.finish()
            .map_err(|e| EngineError::Encode(format!("PDF: {e:?}")))?
            .len() as u64)
    };
    // Grow each part with a doubling then binary search on the real written size, so a
    // 500-page file needs a few dozen trial writes, not hundreds.
    let mut ranges = Vec::new();
    let mut a = 0;
    while a < n {
        cancel.check()?;
        let mut good = a + 1;
        let mut step = 1;
        let mut bad = None;
        while good < n {
            let probe = (good + step).min(n);
            if size_of(a, probe)? <= max_bytes {
                good = probe;
                step *= 2;
            } else {
                bad = Some(probe);
                break;
            }
        }
        if let Some(mut hi) = bad {
            let mut lo = good;
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if size_of(a, mid)? <= max_bytes {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            good = lo;
        }
        ranges.push((a, good));
        a = good;
    }
    write_ranges(input, dir, stem, n, &ranges, cancel)
}

fn write_ranges(
    input: &Path,
    dir: &Path,
    stem: &str,
    n: usize,
    ranges: &[(usize, usize)],
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let pdoc = PdfDocument::new(load(input)?);
    let digits = n.to_string().len().max(2);
    let mut outs = Vec::new();
    for &(a, b) in ranges {
        cancel.check()?;
        let mut d = krilla::Document::new();
        d.embed_pdf_pages(&pdoc, &(a..b).collect::<Vec<_>>());
        let name = if b - a == 1 {
            format!("{stem} p{:0digits$}.pdf", a + 1)
        } else {
            format!("{stem} p{:0digits$}-{:0digits$}.pdf", a + 1, b)
        };
        let out = dir.join(name);
        write(d, &out)?;
        outs.push(out);
    }
    Ok(outs)
}

/// Split into one file per group of pages (each group in the order given): `stem 1
/// (p1-15, 16).pdf`, `stem 2 (p25-40).pdf`… Groups may share pages.
pub fn split_groups(
    input: &Path,
    dir: &Path,
    stem: &str,
    groups: &[Vec<usize>],
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let pdf = load(input)?;
    let n = pdf.pages().len();
    if groups.is_empty() || groups.iter().any(Vec::is_empty) {
        return Err(EngineError::Invalid(
            "each PDF needs at least one page".into(),
        ));
    }
    if let Some(&p) = groups.iter().flatten().find(|&&p| p >= n) {
        return Err(EngineError::Invalid(format!(
            "page {} doesn't exist (there are {n})",
            p + 1
        )));
    }
    let pdoc = PdfDocument::new(pdf);
    let digits = groups.len().to_string().len();
    let mut outs = Vec::with_capacity(groups.len());
    for (k, g) in groups.iter().enumerate() {
        cancel.check()?;
        let mut d = krilla::Document::new();
        d.embed_pdf_pages(&pdoc, g);
        let label = page_label(g);
        let label = if label.len() <= 40 {
            format!("p{label}")
        } else {
            format!("{} pages", g.len())
        };
        let out = dir.join(format!("{stem} {:0digits$} ({label}).pdf", k + 1));
        write(d, &out)?;
        outs.push(out);
    }
    Ok(outs)
}

/// "1-3, 7" for 0-based pages (runs of consecutive pages joined), safe in file names.
fn page_label(pages: &[usize]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < pages.len() {
        let a = pages[i];
        let mut b = a;
        while i + 1 < pages.len() && pages[i + 1] == b + 1 {
            i += 1;
            b = pages[i];
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        if a == b {
            out.push_str(&(a + 1).to_string());
        } else {
            out.push_str(&format!("{}-{}", a + 1, b + 1));
        }
        i += 1;
    }
    out
}

/// A file-name-safe version of a bookmark title.
fn safe_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(60).collect();
    let t = t.trim_end_matches(['.', ' ']).to_string();
    if t.is_empty() { "Part".into() } else { t }
}

// ------------------------------------------------------------ sheets

/// Paper sizes in points, portrait.
pub const A4: (f32, f32) = (595.28, 841.89);
pub const LETTER: (f32, f32) = (612.0, 792.0);

pub fn paper(name: &str) -> Option<(f32, f32)> {
    Some(match name.to_ascii_lowercase().as_str() {
        "a4" => A4,
        "a3" => (841.89, 1190.55),
        "a5" => (419.53, 595.28),
        "letter" => LETTER,
        "legal" => (612.0, 1008.0),
        _ => return None,
    })
}

/// Draw `page` of `pdoc` fitted and centred in the cell (`x`, `y`, `w`, `h`), y-down.
pub(crate) fn draw_in(
    s: &mut krilla::surface::Surface<'_>,
    pdoc: &PdfDocument,
    size: (f32, f32),
    page: usize,
    cell: (f32, f32, f32, f32),
) {
    let (x, y, w, h) = cell;
    let k = (w / size.0).min(h / size.1);
    let (pw, ph) = (size.0 * k, size.1 * k);
    s.push_transform(&Transform::from_translate(
        x + (w - pw) / 2.0,
        y + (h - ph) / 2.0,
    ));
    if let Some(sz) = Size::from_wh(pw, ph) {
        s.draw_pdf_page(pdoc, sz, page);
    }
    s.pop();
}

/// Put `per_sheet` (2, 4, 6, 8 or 9) pages on each `sheet`, in reading order. 2 and 8 per
/// sheet print on landscape paper.
pub fn n_up(
    input: &Path,
    out: &Path,
    per_sheet: usize,
    sheet: (f32, f32),
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let (cols, rows, landscape) = match per_sheet {
        2 => (2, 1, true),
        4 => (2, 2, false),
        6 => (2, 3, false),
        8 => (4, 2, true),
        9 => (3, 3, false),
        _ => {
            return Err(EngineError::Invalid(
                "pages per sheet: 2, 4, 6, 8 or 9".into(),
            ));
        }
    };
    let (sw, sh) = if landscape {
        (sheet.0.max(sheet.1), sheet.0.min(sheet.1))
    } else {
        (sheet.0.min(sheet.1), sheet.0.max(sheet.1))
    };
    let pdf = load(input)?;
    let sizes: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
    let pdoc = PdfDocument::new(pdf);
    let margin = 18.0;
    let gap = 10.0;
    let cw = (sw - 2.0 * margin - gap * (cols - 1) as f32) / cols as f32;
    let ch = (sh - 2.0 * margin - gap * (rows - 1) as f32) / rows as f32;
    let mut doc = krilla::Document::new();
    let mut sheets = 0;
    for chunk in (0..sizes.len()).collect::<Vec<_>>().chunks(per_sheet) {
        cancel.check()?;
        let settings = PageSettings::from_wh(sw, sh)
            .ok_or_else(|| EngineError::Invalid("bad sheet size".into()))?;
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        for (k, &p) in chunk.iter().enumerate() {
            let (c, r) = (k % cols, k / cols);
            let cell = (
                margin + c as f32 * (cw + gap),
                margin + r as f32 * (ch + gap),
                cw,
                ch,
            );
            draw_in(&mut s, &pdoc, sizes[p], p, cell);
        }
        s.finish();
        page.finish();
        sheets += 1;
    }
    write(doc, out)?;
    Ok(sheets)
}

/// The page order for a saddle-stitched booklet of `n` pages (padded to a multiple of 4
/// with blanks, shown as None): for each sheet, front left, front right, back left, back
/// right.
pub fn booklet_order(n: usize) -> Vec<Option<usize>> {
    let total = n.div_ceil(4) * 4;
    let at = |i: usize| (i < n).then_some(i);
    let mut v = Vec::with_capacity(total);
    for s in 0..total / 4 {
        v.push(at(total - 1 - 2 * s));
        v.push(at(2 * s));
        v.push(at(2 * s + 1));
        v.push(at(total - 2 - 2 * s));
    }
    v
}

/// A booklet: print double-sided (flip on the short edge), fold the stack in half.
pub fn booklet(
    input: &Path,
    out: &Path,
    sheet: (f32, f32),
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let pdf = load(input)?;
    let sizes: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
    let pdoc = PdfDocument::new(pdf);
    let (sw, sh) = (sheet.0.max(sheet.1), sheet.0.min(sheet.1));
    let mut doc = krilla::Document::new();
    let order = booklet_order(sizes.len());
    for side in order.chunks(2) {
        cancel.check()?;
        let settings = PageSettings::from_wh(sw, sh)
            .ok_or_else(|| EngineError::Invalid("bad sheet size".into()))?;
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        for (k, p) in side.iter().enumerate() {
            if let Some(p) = *p {
                draw_in(
                    &mut s,
                    &pdoc,
                    sizes[p],
                    p,
                    (k as f32 * sw / 2.0, 0.0, sw / 2.0, sh),
                );
            }
        }
        s.finish();
        page.finish();
    }
    write(doc, out)?;
    Ok(order.len() / 2)
}

// ------------------------------------------------------------ in place

/// Scale every selected page to fit `paper` (portrait; landscape pages get landscape
/// paper), centred. Content, links and form fields scale with it.
pub fn resize(
    input: &Path,
    out: &Path,
    paper: (f32, f32),
    pages: Option<&[usize]>,
) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let ids = super::pages(&doc);
    let sel = selection(pages, ids.len())?;
    for &i in &sel {
        let id = ids[i];
        let g = Geom::of(&doc, id);
        let (dw, dh) = g.display_size();
        let (pw, ph) = if dw > dh {
            (paper.0.max(paper.1), paper.0.min(paper.1))
        } else {
            (paper.0.min(paper.1), paper.0.max(paper.1))
        };
        // Target in user space (swap for pages displayed sideways).
        let (tw, th) = if g.rotate % 180 == 0 {
            (pw, ph)
        } else {
            (ph, pw)
        };
        let [x0, y0, x1, y1] = g.crop;
        let (cw, ch) = (x1 - x0, y1 - y0);
        let k = (tw / cw).min(th / ch);
        let dx = (tw - k * cw) / 2.0 - k * x0;
        let dy = (th - k * ch) / 2.0 - k * y0;
        let m = [k, 0.0, 0.0, k, dx, dy];
        let map = |x: f32, y: f32| (k * x + dx, k * y + dy);
        // Annotations move with the content.
        let annots: Vec<lopdf::ObjectId> = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"Annots").ok())
            .map(|a| match super::deref(&doc, a) {
                Object::Array(v) => v.iter().filter_map(|o| o.as_reference().ok()).collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default();
        for a in annots {
            if let Ok(d) = doc.get_dictionary_mut(a) {
                if let Some(r) = d.get(b"Rect").ok().and_then(rect4) {
                    let (ax, ay) = map(r[0], r[1]);
                    let (bx, by) = map(r[2], r[3]);
                    d.set("Rect", box_object([ax, ay, bx, by]));
                }
                if let Ok(Object::Array(q)) = d.get(b"QuadPoints") {
                    let v: Vec<Object> = q
                        .chunks(2)
                        .flat_map(|xy| {
                            let x = num(&xy[0]);
                            let y = xy.get(1).map_or(0.0, num);
                            let (a, b) = map(x, y);
                            [Object::Real(a), Object::Real(b)]
                        })
                        .collect();
                    d.set("QuadPoints", Object::Array(v));
                }
            }
        }
        let d = doc.get_dictionary_mut(id).map_err(EngineError::encode)?;
        d.set("MediaBox", box_object([0.0, 0.0, tw, th]));
        for k in [&b"CropBox"[..], b"BleedBox", b"TrimBox", b"ArtBox"] {
            d.remove(k);
        }
        wrap_with(
            &mut doc,
            id,
            format!("q {}\n", cm(m)).into_bytes(),
            b"\nQ\n".to_vec(),
        )?;
    }
    super::save(&mut doc, out, false)?;
    Ok(sel.len())
}

fn num(o: &Object) -> f32 {
    match o {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        _ => 0.0,
    }
}

fn rect4(o: &Object) -> Option<Box4> {
    let a = o.as_array().ok()?;
    (a.len() == 4).then(|| [num(&a[0]), num(&a[1]), num(&a[2]), num(&a[3])])
}

/// Crop the selected pages by `margins` (left, bottom, right, top) in points, measured on
/// the page as displayed. Lossless: the cut-off part is hidden, not deleted.
pub fn crop(
    input: &Path,
    out: &Path,
    margins: [f32; 4],
    pages: Option<&[usize]>,
) -> Result<usize, EngineError> {
    let n = page_count(input)?;
    let sel = selection(pages, n)?;
    let plan: Vec<(usize, [f32; 4])> = sel.into_iter().map(|i| (i, margins)).collect();
    crop_each(input, out, &plan)
}

/// Crop pages by their own margins ([left, bottom, right, top] in points from the
/// displayed page's edges). A page listed twice takes its last entry; all-zero margins
/// leave a page alone.
pub fn crop_each(
    input: &Path,
    out: &Path,
    plan: &[(usize, [f32; 4])],
) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let ids = super::pages(&doc);
    let mut last: std::collections::BTreeMap<usize, [f32; 4]> = Default::default();
    for &(i, m) in plan {
        if i >= ids.len() {
            return Err(EngineError::Invalid(format!(
                "page {} doesn't exist (there are {})",
                i + 1,
                ids.len()
            )));
        }
        last.insert(i, m.map(|m| m.max(0.0)));
    }
    let mut done = 0;
    for (i, [l, b, r, t]) in last {
        if l + b + r + t <= 0.0 {
            continue;
        }
        let g = Geom::of(&doc, ids[i]);
        let (w, h) = g.display_size();
        if l + r > w - 10.0 || b + t > h - 10.0 {
            return Err(EngineError::Invalid(format!(
                "those margins are bigger than page {}",
                i + 1
            )));
        }
        set_crop(&mut doc, ids[i], &g, [l, b, w - r, h - t])?;
        done += 1;
    }
    if done == 0 {
        return Err(EngineError::Invalid(
            "nothing to crop: all margins are 0".into(),
        ));
    }
    super::save(&mut doc, out, false)?;
    Ok(done)
}

/// A copy holding only page `page` (0-based), with its content, annotations and
/// resources as they are (not redrawn): for previews of tools that edit pages in place.
pub fn keep_page(input: &Path, page: usize, out: &Path) -> Result<(), EngineError> {
    let mut doc = super::open(input, None)?;
    let n = super::pages(&doc).len();
    if page >= n {
        return Err(EngineError::Invalid(format!(
            "page {} doesn't exist (there are {n})",
            page + 1
        )));
    }
    let drop: Vec<u32> = (1..=n as u32).filter(|&p| p as usize != page + 1).collect();
    doc.delete_pages(&drop);
    doc.prune_objects();
    super::save(&mut doc, out, false)
}

pub(crate) fn set_crop(
    doc: &mut lopdf::Document,
    page: lopdf::ObjectId,
    g: &Geom,
    display: Box4,
) -> Result<(), EngineError> {
    let u = g.rect_to_user(display);
    let d = doc.get_dictionary_mut(page).map_err(EngineError::encode)?;
    d.set("CropBox", box_object(u));
    // Printers and viewers that honour TrimBox/ArtBox should see the same area.
    for k in [&b"TrimBox"[..], b"ArtBox", b"BleedBox"] {
        d.remove(k);
    }
    Ok(())
}

/// Trim the white margins around each selected page's content, keeping `pad` points.
/// The content box is found by rendering the page at low resolution.
pub fn auto_crop(
    input: &Path,
    out: &Path,
    pad: f32,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let ids = super::pages(&doc);
    let sel = selection(pages, ids.len())?;
    let r = crate::pdf_tools::PageRenderer::open(input)?;
    const DPI: f32 = 48.0;
    let boxes: Vec<Option<Box4>> = r.session(|s| {
        sel.iter()
            .map(|&i| -> Result<Option<Box4>, EngineError> {
                cancel.check()?;
                let img = s.render_dpi(i, DPI)?;
                Ok(ink_box(&img).map(|[x0, y0, x1, y1]| {
                    let k = 72.0 / DPI;
                    let h = img.height() as f32;
                    // Pixels (y down) → display points (y up), one pixel of slack.
                    [
                        (x0 as f32 - 1.0) * k,
                        (h - y1 as f32 - 2.0) * k,
                        (x1 as f32 + 2.0) * k,
                        (h - y0 as f32 + 1.0) * k,
                    ]
                }))
            })
            .collect::<Result<Vec<_>, _>>()
    })?;
    let mut changed = 0;
    for (&i, b) in sel.iter().zip(boxes) {
        let Some(b) = b else { continue };
        let g = Geom::of(&doc, ids[i]);
        let (w, h) = g.display_size();
        let r = [
            (b[0] - pad).max(0.0),
            (b[1] - pad).max(0.0),
            (b[2] + pad).min(w),
            (b[3] + pad).min(h),
        ];
        if r[0] > 1.0 || r[1] > 1.0 || r[2] < w - 1.0 || r[3] < h - 1.0 {
            set_crop(&mut doc, ids[i], &g, r)?;
            changed += 1;
        }
    }
    if changed == 0 {
        return Err(EngineError::Invalid(
            "there are no white margins to trim".into(),
        ));
    }
    super::save(&mut doc, out, false)?;
    Ok(changed)
}

/// Bounding box of non-white pixels: [x0, y0, x1, y1] in pixels, y down.
fn ink_box(img: &image::RgbaImage) -> Option<[u32; 4]> {
    let (w, h) = img.dimensions();
    let ink = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        p[0] < 245 || p[1] < 245 || p[2] < 245
    };
    let rows: Vec<u32> = (0..h).filter(|&y| (0..w).any(|x| ink(x, y))).collect();
    let (&y0, &y1) = (rows.first()?, rows.last()?);
    let x0 = (0..w).find(|&x| (y0..=y1).any(|y| ink(x, y)))?;
    let x1 = (0..w).rev().find(|&x| (y0..=y1).any(|y| ink(x, y)))?;
    Some([x0, y0, x1 + 1, y1 + 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdfedit::testing::sample;

    #[test]
    fn booklet_pages_pair_up_for_folding() {
        // 8 pages: sheet 1 front 8|1, back 2|7; sheet 2 front 6|3, back 4|5.
        let v: Vec<Option<usize>> = booklet_order(8);
        let one = |i: usize| Some(i - 1);
        assert_eq!(
            v,
            [
                one(8),
                one(1),
                one(2),
                one(7),
                one(6),
                one(3),
                one(4),
                one(5)
            ]
        );
        // 5 pages pad to 8 with blanks.
        assert_eq!(booklet_order(5)[0], None);
    }

    #[test]
    fn assemble_split_nup_resize_crop() {
        let d = tempfile::tempdir().unwrap();
        let c = CancelToken::new();
        let a = d.path().join("a.pdf");
        let b = d.path().join("b.pdf");
        sample(&a, 5, false);
        sample(&b, 2, true);

        let out = d.path().join("plan.pdf");
        let mut plan = insert_plan(5, 2, Some(1));
        plan.push(PageSrc::Blank { w: 200.0, h: 300.0 });
        assert_eq!(
            assemble(&[a.clone(), b.clone()], &plan, &out, &c).unwrap(),
            8
        );
        let sizes = page_sizes(&out).unwrap();
        assert_eq!(sizes.len(), 8);
        assert!(
            sizes[2].0 > sizes[2].1,
            "page 3 comes from the landscape file"
        );
        assert_eq!(sizes[7], (200.0, 300.0));

        let re = d.path().join("re.pdf");
        assert_eq!(
            assemble(
                std::slice::from_ref(&a),
                &reorder_plan(&[4, 3, 3, 0]),
                &re,
                &c
            )
            .unwrap(),
            4
        );

        let parts = d.path().join("parts");
        std::fs::create_dir(&parts).unwrap();
        let outs = split_every(&a, &parts, "a", 2, &c).unwrap();
        let names: Vec<String> = outs
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a p01-02.pdf", "a p03-04.pdf", "a p05.pdf"]);

        let sized = d.path().join("sized");
        std::fs::create_dir(&sized).unwrap();
        let whole = std::fs::metadata(&re).unwrap().len();
        let limit = whole * 7 / 10;
        let outs = split_by_size(&a, &sized, "a", limit, &c).unwrap();
        assert!(outs.len() >= 2 && outs.len() <= 5, "{outs:?}");
        let total: usize = outs.iter().map(|p| page_count(p).unwrap()).sum();
        assert_eq!(total, 5);
        for p in &outs {
            let len = std::fs::metadata(p).unwrap().len();
            assert!(
                len <= limit || page_count(p).unwrap() == 1,
                "{p:?} {len} > {limit}"
            );
        }

        let up = d.path().join("up.pdf");
        assert_eq!(n_up(&a, &up, 4, A4, &c).unwrap(), 2);
        let bk = d.path().join("bk.pdf");
        assert_eq!(booklet(&a, &bk, A4, &c).unwrap(), 4);
        let s = page_sizes(&bk).unwrap();
        assert!(s[0].0 > s[0].1);

        let letter = d.path().join("letter.pdf");
        resize(&b, &letter, LETTER, None).unwrap();
        let s = page_sizes(&letter).unwrap();
        assert!(
            (s[0].0 - 792.0).abs() < 0.5 && (s[0].1 - 612.0).abs() < 0.5,
            "{s:?}"
        );

        let cropped = d.path().join("crop.pdf");
        crop(&a, &cropped, [50.0, 60.0, 70.0, 80.0], Some(&[0])).unwrap();
        let s = page_sizes(&cropped).unwrap();
        assert!(
            (s[0].0 - 475.0).abs() < 0.5 && (s[0].1 - 702.0).abs() < 0.5,
            "{s:?}"
        );
        assert_eq!(s[1], (595.0, 842.0));

        let trimmed = d.path().join("trim.pdf");
        auto_crop(&a, &trimmed, 0.0, Some(&[0]), &c).unwrap();
        let s = page_sizes(&trimmed).unwrap();
        // The block on page 1 is 40 × 40 pt.
        assert!(s[0].0 < 60.0 && s[0].1 < 60.0 && s[0].0 > 38.0, "{s:?}");
    }
}
