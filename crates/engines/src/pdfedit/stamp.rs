//! Stamps: text, images and boxes drawn on top of existing pages (page numbers, headers and
//! footers, Bates numbers, watermarks, signatures, redaction boxes).
//!
//! The stamp for each page is drawn as its own small PDF page by krilla (real fonts with
//! subsetting and full Unicode, images, opacity), then grafted onto the original page as a
//! Form XObject. The original page keeps its content, links, form fields and everything
//! else; it only gains one `Do` at the end of its content.

use std::collections::HashMap;
use std::path::Path;

use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::Fill;
use krilla::text::TextDirection;
use lopdf::{Dictionary, Document, Object, Stream};
use swirvo_core::{CancelToken, EngineError};

use super::geometry::{Box4, Geom};
use super::{fresh_name, own_resources, resources_mut, selection, sub_dict, wrap_and_append};
use crate::docxlayout::fonts::Fonts;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    Left,
    #[default]
    Center,
    Right,
}

/// Something drawn on a page, in display coordinates (points, origin at the bottom-left
/// of the page as shown, y up).
#[derive(Debug, Clone)]
pub enum Item {
    /// Text whose baseline starts at (`x`, `y`) after alignment, turned `angle` degrees
    /// counter-clockwise about that point.
    Text {
        text: String,
        size: f32,
        color: [u8; 3],
        opacity: f32,
        x: f32,
        y: f32,
        align: HAlign,
        angle: f32,
        bold: bool,
    },
    /// A PNG or JPEG fitted into `rect` (aspect ratio kept, centred).
    Image {
        bytes: std::sync::Arc<Vec<u8>>,
        rect: Box4,
        opacity: f32,
    },
    Rect {
        rect: Box4,
        color: [u8; 3],
        opacity: f32,
    },
    /// Invisible text (a searchable PDF's OCR layer): baseline-left at (`x`, `y`),
    /// stretched to `w` points wide. Selectable and searchable, never seen or printed.
    Hidden {
        text: String,
        size: f32,
        x: f32,
        y: f32,
        w: f32,
    },
}

/// What the stamp function knows about each page.
#[derive(Debug, Clone, Copy)]
pub struct PageInfo {
    /// 0-based page index and the document's page count.
    pub index: usize,
    pub count: usize,
    /// Display width and height in points.
    pub w: f32,
    pub h: f32,
}

/// Stamp the 0-based `pages` (None = all) with the items `f` returns for each. Pages for
/// which `f` returns nothing are left untouched. Returns how many pages were stamped.
pub fn stamp(
    input: &Path,
    out: &Path,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
    f: impl Fn(PageInfo) -> Vec<Item>,
) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let n = stamp_doc(&mut doc, pages, cancel, f)?;
    super::save(&mut doc, out, false)?;
    Ok(n)
}

/// [`stamp`] on an open document.
pub fn stamp_doc(
    doc: &mut Document,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
    f: impl Fn(PageInfo) -> Vec<Item>,
) -> Result<usize, EngineError> {
    let ids = super::pages(doc);
    let count = ids.len();
    let sel = selection(pages, count)?;
    let mut work = Vec::new();
    for i in sel {
        let g = Geom::of(doc, ids[i]);
        let (w, h) = g.display_size();
        let items = f(PageInfo {
            index: i,
            count,
            w,
            h,
        });
        if !items.is_empty() {
            work.push((ids[i], g, w, h, items));
        }
    }
    if work.is_empty() {
        return Ok(0);
    }
    cancel.check()?;
    let overlay = draw_overlay(
        &work
            .iter()
            .map(|w| (w.2, w.3, &w.4[..]))
            .collect::<Vec<_>>(),
    )?;
    cancel.check()?;
    graft(
        doc,
        overlay,
        &work.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>(),
    )?;
    Ok(work.len())
}

/// One krilla page per stamped page.
fn draw_overlay(pages: &[(f32, f32, &[Item])]) -> Result<Vec<u8>, EngineError> {
    if !Fonts::available()
        && pages.iter().any(|p| {
            p.2.iter()
                .any(|i| matches!(i, Item::Text { .. } | Item::Hidden { .. }))
        })
    {
        return Err(EngineError::Encode("no fonts are installed".into()));
    }
    let mut fonts = Fonts::new(HashMap::new());
    let mut doc = krilla::Document::new();
    for &(w, h, items) in pages {
        let settings = PageSettings::from_wh(w, h)
            .ok_or_else(|| EngineError::Encode("bad page size".into()))?;
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        for it in items {
            draw_item(&mut s, &mut fonts, h, it)?;
        }
        s.finish();
        page.finish();
    }
    let pdf = doc
        .finish()
        .map_err(|e| EngineError::Encode(format!("stamp: {e:?}")))?;
    if pages
        .iter()
        .any(|p| p.2.iter().any(|i| matches!(i, Item::Hidden { .. })))
    {
        invisible_text(&pdf)
    } else {
        Ok(pdf)
    }
}

/// Switch stroked text (`1 Tr`) to invisible text (`3 Tr`) in every content stream of the
/// overlay.
fn invisible_text(pdf: &[u8]) -> Result<Vec<u8>, EngineError> {
    let mut d = Document::load_mem(pdf).map_err(|e| EngineError::Encode(format!("stamp: {e}")))?;
    let mut contents: Vec<lopdf::ObjectId> = d
        .get_pages()
        .values()
        .flat_map(|&p| d.get_page_contents(p))
        .collect();
    contents.extend(d.objects.iter().filter_map(|(id, o)| match o {
        Object::Stream(st)
            if st.dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok())
                == Some(&b"Form"[..]) =>
        {
            Some(*id)
        }
        _ => None,
    }));
    for id in contents {
        let Ok(Object::Stream(st)) = d.get_object_mut(id) else {
            continue;
        };
        let raw = st
            .decompressed_content()
            .unwrap_or_else(|_| st.content.clone());
        let mut c = lopdf::content::Content::decode(&raw).map_err(EngineError::encode)?;
        let mut changed = false;
        for op in &mut c.operations {
            if op.operator == "Tr" && matches!(op.operands.first(), Some(Object::Integer(1))) {
                op.operands = vec![Object::Integer(3)];
                changed = true;
            }
        }
        if changed {
            st.set_plain_content(c.encode().map_err(EngineError::encode)?);
            let _ = st.compress();
        }
    }
    let mut out = Vec::new();
    d.save_to(&mut out).map_err(EngineError::encode)?;
    Ok(out)
}

fn opacity(o: f32) -> NormalizedF32 {
    NormalizedF32::new(o.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE)
}

fn fill(c: [u8; 3]) -> Fill {
    Fill {
        paint: krilla::color::rgb::Color::new(c[0], c[1], c[2]).into(),
        ..Default::default()
    }
}

fn draw_item(
    s: &mut krilla::surface::Surface<'_>,
    fonts: &mut Fonts,
    page_h: f32,
    it: &Item,
) -> Result<(), EngineError> {
    match it {
        Item::Rect {
            rect,
            color,
            opacity: o,
        } => {
            let Some(r) = Rect::from_ltrb(rect[0], page_h - rect[3], rect[2], page_h - rect[1])
            else {
                return Ok(());
            };
            let mut pb = PathBuilder::new();
            pb.push_rect(r);
            let Some(path) = pb.finish() else {
                return Ok(());
            };
            s.push_opacity(opacity(*o));
            s.set_fill(Some(fill(*color)));
            s.set_stroke(None);
            s.draw_path(&path);
            s.pop();
        }
        Item::Image {
            bytes,
            rect,
            opacity: o,
        } => {
            let data: krilla::Data = bytes.as_ref().clone().into();
            let img = match image::guess_format(bytes) {
                Ok(image::ImageFormat::Jpeg) => krilla::image::Image::from_jpeg(data, true),
                Ok(image::ImageFormat::Png) => krilla::image::Image::from_png(data, true),
                _ => {
                    let d = image::load_from_memory(bytes).map_err(EngineError::decode)?;
                    let mut png = Vec::new();
                    d.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                        .map_err(EngineError::encode)?;
                    krilla::image::Image::from_png(png.into(), true)
                }
            }
            .map_err(|e| EngineError::Decode(format!("image: {e}")))?;
            let (iw, ih) = img.size();
            let (bw, bh) = (rect[2] - rect[0], rect[3] - rect[1]);
            let k = (bw / iw.max(1) as f32).min(bh / ih.max(1) as f32);
            let (w, h) = (iw as f32 * k, ih as f32 * k);
            let x = rect[0] + (bw - w) / 2.0;
            let top = page_h - rect[3] + (bh - h) / 2.0;
            if let Some(size) = Size::from_wh(w, h) {
                s.push_opacity(opacity(*o));
                s.push_transform(&Transform::from_translate(x, top));
                s.draw_image(img, size);
                s.pop();
                s.pop();
            }
        }
        Item::Hidden {
            text,
            size,
            x,
            y,
            w,
        } => {
            let runs = runs(fonts, text, false);
            let natural = runs.iter().map(|r| r.2).sum::<f32>() * size;
            if natural <= 0.0 || *size <= 0.0 {
                return Ok(());
            }
            let sx = (w / natural).clamp(0.05, 20.0);
            // Drawn as stroked text, which nothing else here uses; `invisible_text` then
            // turns stroke mode into the invisible mode (3 Tr) krilla doesn't offer.
            s.set_fill(None);
            s.set_stroke(Some(krilla::paint::Stroke::default()));
            s.push_transform(&Transform::from_row(sx, 0.0, 0.0, 1.0, *x, page_h - *y));
            let mut pen = 0.0;
            for (face, t, adv) in runs {
                let font = fonts.get(face).pdf.clone();
                s.draw_text(
                    Point::from_xy(pen, 0.0),
                    font,
                    *size,
                    &t,
                    false,
                    TextDirection::Auto,
                );
                pen += adv * size;
            }
            s.pop();
        }
        Item::Text {
            text,
            size,
            color,
            opacity: o,
            x,
            y,
            align,
            angle,
            bold,
        } => {
            let runs = runs(fonts, text, *bold);
            let width: f32 = runs.iter().map(|r| r.2).sum::<f32>() * size;
            let start = match align {
                HAlign::Left => 0.0,
                HAlign::Center => -width / 2.0,
                HAlign::Right => -width,
            };
            s.push_opacity(opacity(*o));
            s.set_fill(Some(fill(*color)));
            s.set_stroke(None);
            // krilla's page space is y-down; rotate about the anchor, counter-clockwise.
            s.push_transform(&Transform::from_translate(*x, page_h - *y));
            s.push_transform(&Transform::from_rotate(-angle));
            let mut pen = start;
            for (face, t, adv) in runs {
                let font = fonts.get(face).pdf.clone();
                s.draw_text(
                    Point::from_xy(pen, 0.0),
                    font,
                    *size,
                    &t,
                    false,
                    TextDirection::Auto,
                );
                pen += adv * size;
            }
            s.pop();
            s.pop();
            s.pop();
        }
    }
    Ok(())
}

/// Split `text` into runs by the face that covers each character; (face, text, advance in
/// ems).
fn runs(fonts: &mut Fonts, text: &str, bold: bool) -> Vec<(usize, String, f32)> {
    let Some(base) = fonts.face("Arial", bold, false) else {
        return Vec::new();
    };
    let mut out: Vec<(usize, String)> = Vec::new();
    for c in text.chars() {
        let f = fonts.cover(base, c, bold, false);
        match out.last_mut() {
            Some((lf, s)) if *lf == f => s.push(c),
            _ => out.push((f, c.to_string())),
        }
    }
    out.into_iter()
        .map(|(f, t)| {
            let adv = fonts.shape(f, &t, false, true).iter().map(|g| g.adv).sum();
            (f, t, adv)
        })
        .collect()
}

/// Width of `text` in points at `size` (for fitting watermarks and footers).
pub fn text_width(text: &str, size: f32, bold: bool) -> f32 {
    let mut fonts = Fonts::new(HashMap::new());
    runs(&mut fonts, text, bold)
        .iter()
        .map(|r| r.2)
        .sum::<f32>()
        * size
}

/// Copy the overlay's objects into `doc` and draw overlay page k on `targets[k]`.
fn graft(
    doc: &mut Document,
    overlay: Vec<u8>,
    targets: &[(lopdf::ObjectId, Geom)],
) -> Result<(), EngineError> {
    let mut ov = Document::load_mem(&overlay).map_err(EngineError::encode)?;
    ov.renumber_objects_with(doc.max_id + 1);
    let ov_pages = super::pages(&ov);
    if ov_pages.len() != targets.len() {
        return Err(EngineError::Encode("stamp pages don't line up".into()));
    }
    let mut forms = Vec::with_capacity(targets.len());
    for (&pid, (_, g)) in ov_pages.iter().zip(targets) {
        let content = ov.get_page_content(pid).map_err(EngineError::encode)?;
        let res = super::inherited(&ov, pid, b"Resources")
            .cloned()
            .unwrap_or(Object::Dictionary(Dictionary::new()));
        let (w, h) = g.display_size();
        let m = g.to_user();
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set(
            "BBox",
            Object::Array(vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]),
        );
        dict.set(
            "Matrix",
            Object::Array(m.iter().map(|&v| Object::Real(v)).collect()),
        );
        dict.set("Resources", res);
        let mut st = Stream::new(dict, content);
        let _ = st.compress();
        forms.push(st);
    }
    // The overlay's own catalog and page tree come along; they're unreferenced and pruned.
    doc.max_id = doc.max_id.max(ov.max_id);
    doc.objects.extend(std::mem::take(&mut ov.objects));
    for ((page, _), form) in targets.iter().zip(forms) {
        let fid = doc.add_object(form);
        own_resources(doc, *page)?;
        let objects = doc.objects.clone();
        let res = resources_mut(doc, *page)?;
        let xo = sub_dict(&objects, res, b"XObject");
        let name = fresh_name(xo, "ApStamp");
        xo.set(name.clone(), Object::Reference(fid));
        let op = format!("q /{} Do Q\n", String::from_utf8_lossy(&name));
        wrap_and_append(doc, *page, op.into_bytes())?;
    }
    doc.prune_objects();
    Ok(())
}

// ------------------------------------------------------------ ready-made stamps

/// Where a line of text goes on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Spot {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    #[default]
    BottomCenter,
    BottomRight,
}

impl Spot {
    pub fn parse(s: &str) -> Option<Spot> {
        Some(
            match s.to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
                "topleft" => Spot::TopLeft,
                "top" | "topcenter" => Spot::TopCenter,
                "topright" => Spot::TopRight,
                "bottomleft" => Spot::BottomLeft,
                "bottom" | "bottomcenter" => Spot::BottomCenter,
                "bottomright" => Spot::BottomRight,
                _ => return None,
            },
        )
    }

    /// Baseline point and alignment for text of `size` with `margin` from the edges.
    fn place(self, p: PageInfo, size: f32, margin: f32) -> (f32, f32, HAlign) {
        let top = p.h - margin - size * 0.75;
        let bottom = margin;
        match self {
            Spot::TopLeft => (margin, top, HAlign::Left),
            Spot::TopCenter => (p.w / 2.0, top, HAlign::Center),
            Spot::TopRight => (p.w - margin, top, HAlign::Right),
            Spot::BottomLeft => (margin, bottom, HAlign::Left),
            Spot::BottomCenter => (p.w / 2.0, bottom, HAlign::Center),
            Spot::BottomRight => (p.w - margin, bottom, HAlign::Right),
        }
    }
}

/// Text on a spot, with tokens: `{n}` page number, `{total}` page count, `{file}`, `{date}`.
#[derive(Debug, Clone)]
pub struct Line {
    pub template: String,
    pub spot: Spot,
    pub size: f32,
    pub color: [u8; 3],
    /// The number the first stamped page gets ({n}).
    pub start: usize,
    /// Zero-pad {n} to this many digits (Bates numbering).
    pub digits: usize,
    /// {total} when it isn't this run's page count (a preview stamping one page alone).
    pub total: Option<usize>,
    pub file: String,
    pub date: String,
}

impl Default for Line {
    fn default() -> Self {
        Line {
            template: "{n} / {total}".into(),
            spot: Spot::BottomCenter,
            size: 10.0,
            color: [60, 60, 60],
            start: 1,
            digits: 0,
            total: None,
            file: String::new(),
            date: String::new(),
        }
    }
}

impl Line {
    fn render(&self, number: usize, total: usize) -> String {
        let n = if self.digits > 0 {
            format!("{number:0width$}", width = self.digits)
        } else {
            number.to_string()
        };
        self.template
            .replace("{n}", &n)
            .replace("{total}", &total.to_string())
            .replace("{file}", &self.file)
            .replace("{date}", &self.date)
    }
}

/// Add text lines (page numbers, headers, footers, Bates numbers) to the selected pages.
/// Numbering counts the stamped pages only, starting at each line's `start`.
pub fn add_lines(
    input: &Path,
    out: &Path,
    lines: &[Line],
    pages: Option<&[usize]>,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let n = super::pages(&doc).len();
    let sel = selection(pages, n)?;
    let order: HashMap<usize, usize> = sel.iter().enumerate().map(|(k, &i)| (i, k)).collect();
    let total = sel.len();
    let stamped = stamp_doc(&mut doc, Some(&sel), cancel, |p| {
        let k = order.get(&p.index).copied().unwrap_or(0);
        lines
            .iter()
            .filter(|l| !l.template.trim().is_empty())
            .map(|l| {
                let margin = 24.0_f32.min(p.h / 10.0);
                let (x, y, align) = l.spot.place(p, l.size, margin);
                Item::Text {
                    text: l.render(l.start + k, l.total.unwrap_or(l.start - 1 + total)),
                    size: l.size,
                    color: l.color,
                    opacity: 1.0,
                    x,
                    y,
                    align,
                    angle: 0.0,
                    bold: false,
                }
            })
            .collect()
    })?;
    super::save(&mut doc, out, false)?;
    Ok(stamped)
}

/// Where a watermark goes on each page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Its centre, as fractions of the page from the top-left corner.
    pub center: [f32; 2],
    /// Its width as a fraction of the page width (None: text fills most of the
    /// diagonal, a picture half the page).
    pub width: Option<f32>,
    /// Degrees counter-clockwise (None: along the page's diagonal). Text only.
    pub angle: Option<f32>,
}

impl Default for Placement {
    fn default() -> Self {
        Placement {
            center: [0.5, 0.5],
            width: None,
            angle: None,
        }
    }
}

/// A text watermark on every selected page at `opacity` (0.1–0.3 reads as a
/// watermark): by default along the diagonal, sized to fill most of it.
#[allow(clippy::too_many_arguments)]
pub fn watermark_text(
    input: &Path,
    out: &Path,
    text: &str,
    opacity: f32,
    color: [u8; 3],
    place: &Placement,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    if text.trim().is_empty() {
        return Err(EngineError::Invalid("type the watermark text".into()));
    }
    let unit = text_width(text, 1.0, true).max(0.1);
    let place = *place;
    stamp(input, out, pages, cancel, |p| {
        let diag = (p.w * p.w + p.h * p.h).sqrt();
        let angle = place
            .angle
            .unwrap_or_else(|| (p.h / p.w).atan().to_degrees());
        let size = match place.width {
            Some(f) => (f.clamp(0.02, 1.5) * p.w / unit).clamp(4.0, 2000.0),
            None => (diag * 0.7 / unit).min(p.h.min(p.w) * 0.35),
        };
        // Centre the text box (baseline drops by ~a third of the size).
        let r = angle.to_radians();
        let (dx, dy) = (r.sin() * size * 0.35, -r.cos() * size * 0.35);
        let [fx, fy] = place.center.map(|c| c.clamp(0.0, 1.0));
        vec![Item::Text {
            text: text.to_string(),
            size,
            color,
            opacity,
            x: p.w * fx + dx,
            y: p.h * (1.0 - fy) + dy,
            align: HAlign::Center,
            angle,
            bold: true,
        }]
    })
}

/// A picture watermark (a logo) on each selected page, centred at `place.center`, as
/// wide as `place.width` of the page (default half), aspect ratio kept.
pub fn watermark_image(
    input: &Path,
    out: &Path,
    image: Vec<u8>,
    place: &Placement,
    opacity: f32,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let bytes = std::sync::Arc::new(image);
    let place = *place;
    stamp(input, out, pages, cancel, |p| {
        let f = place.width.unwrap_or(0.5).clamp(0.05, 1.0);
        let (w, h) = (p.w * f, p.h * f);
        let [fx, fy] = place.center.map(|c| c.clamp(0.0, 1.0));
        let (cx, cy) = (p.w * fx, p.h * (1.0 - fy));
        vec![Item::Image {
            bytes: bytes.clone(),
            rect: [cx - w / 2.0, cy - h / 2.0, cx + w / 2.0, cy + h / 2.0],
            opacity,
        }]
    })
}

/// Place an image (a signature, a stamp) in display-space `rect` on one page.
pub fn place_image(
    input: &Path,
    out: &Path,
    page: usize,
    rect: Box4,
    image: Vec<u8>,
    cancel: &CancelToken,
) -> Result<(), EngineError> {
    let bytes = std::sync::Arc::new(image);
    stamp(input, out, Some(&[page]), cancel, |_| {
        vec![Item::Image {
            bytes: bytes.clone(),
            rect,
            opacity: 1.0,
        }]
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_tools::PageRenderer;

    fn red_at(img: &image::RgbaImage, fx: f32, fy: f32) -> bool {
        let p = img.get_pixel(
            ((img.width() as f32 * fx) as u32).min(img.width() - 1),
            ((img.height() as f32 * fy) as u32).min(img.height() - 1),
        );
        p[0] > 180 && p[1] < 90 && p[2] < 90
    }

    #[test]
    fn stamps_land_where_the_reader_sees_them_on_rotated_pages() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 4, false);
        let rotated = d.path().join("r.pdf");
        crate::pdf_tools::rotate_pages(&src, &rotated, 90, Some(&[1])).unwrap();
        crate::pdf_tools::rotate_pages(&rotated, &src, 180, Some(&[2])).unwrap();
        crate::pdf_tools::rotate_pages(&src, &rotated, 270, Some(&[3])).unwrap();
        let out = d.path().join("s.pdf");
        // A red square in the top-right corner of every page, as displayed.
        let n = stamp(&rotated, &out, None, &CancelToken::new(), |p| {
            vec![Item::Rect {
                rect: [p.w - 60.0, p.h - 60.0, p.w - 10.0, p.h - 10.0],
                color: [220, 20, 20],
                opacity: 1.0,
            }]
        })
        .unwrap();
        assert_eq!(n, 4);
        let r = PageRenderer::open(&out).unwrap();
        r.session(|s| {
            for i in 0..4 {
                let img = s.render_dpi(i, 36.0).unwrap();
                assert!(red_at(&img, 0.95, 0.04), "page {i}: stamp not top-right");
                assert!(
                    !red_at(&img, 0.05, 0.96),
                    "page {i}: stamp in the wrong corner"
                );
            }
        });
        // The link on page 1 survives.
        let doc = lopdf::Document::load(&out).unwrap();
        let p1 = super::super::pages(&doc)[0];
        assert_eq!(doc.get_page_annotations(p1).unwrap().len(), 1);
    }

    #[test]
    fn page_numbers_and_watermark_are_real_text() {
        if !Fonts::available() {
            eprintln!("SKIPPED: no fonts");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 3, false);
        let numbered = d.path().join("n.pdf");
        let line = Line {
            template: "Page {n} of {total}".into(),
            ..Line::default()
        };
        assert_eq!(
            add_lines(&src, &numbered, &[line], None, &CancelToken::new()).unwrap(),
            3
        );
        let wm = d.path().join("w.pdf");
        watermark_text(
            &numbered,
            &wm,
            "DRAFT",
            0.2,
            [200, 0, 0],
            &Placement::default(),
            None,
            &CancelToken::new(),
        )
        .unwrap();
        let l = crate::pdflayout::analyze(&std::fs::read(&numbered).unwrap(), &CancelToken::new())
            .unwrap();
        let text = format!("{:?}", l.footer);
        // Our own analysis even recognises them as page-number fields.
        assert!(
            text.contains("PageNumber") && text.contains(" of "),
            "{text}"
        );
        // The watermark is pale red across the middle of the page.
        let r = PageRenderer::open(&wm).unwrap();
        let img = r.session(|s| s.render_dpi(1, 36.0)).unwrap();
        if let Ok(p) = std::env::var("STAMP_OUT") {
            img.save(p).unwrap();
        }
        let tinted = img
            .pixels()
            .filter(|p| p[0] > 200 && p[1] < 235 && p[1] > 120 && p[2] < 235)
            .count();
        assert!(tinted > 500, "only {tinted} watermark pixels");
    }

    /// A watermark put flat in the top-left quarter stays there.
    #[test]
    fn watermarks_go_where_they_are_placed() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 1, false);
        let wm = d.path().join("w.pdf");
        let place = Placement {
            center: [0.25, 0.2],
            width: Some(0.3),
            angle: Some(0.0),
        };
        watermark_text(
            &src,
            &wm,
            "TOP",
            0.9,
            [220, 0, 0],
            &place,
            None,
            &CancelToken::new(),
        )
        .unwrap();
        let r = PageRenderer::open(&wm).unwrap();
        let img = r.session(|s| s.render_dpi(0, 36.0)).unwrap();
        let (w, h) = (img.width() as f32, img.height() as f32);
        let (mut inside, mut outside) = (0, 0);
        for (x, y, p) in img.enumerate_pixels() {
            if p[0] > 180 && p[1] < 120 && p[2] < 120 {
                let (fx, fy) = (x as f32 / w, y as f32 / h);
                if (0.05..0.45).contains(&fx) && (0.1..0.3).contains(&fy) {
                    inside += 1;
                } else {
                    outside += 1;
                }
            }
        }
        assert!(
            inside > 50 && outside == 0,
            "inside {inside}, outside {outside}"
        );
    }
}
