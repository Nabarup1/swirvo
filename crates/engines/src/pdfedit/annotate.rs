//! Comments added by hand (docs/17 §2): highlights, sticky notes, ink, boxes and text
//! boxes, written as standard PDF annotations so every viewer can show, edit or delete
//! them. Each gets its own appearance stream, so it looks the same everywhere, including
//! viewers that don't draw annotations themselves.
//!
//! Positions are in display coordinates (points, origin bottom-left of the page as shown,
//! y up), like the other tools. On a rotated page the appearance is turned with the
//! inverse of the page's rotation (through the form's `/Matrix`), so text and strokes
//! stay upright for the reader.

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, text_string};

use super::forms::{escape, win_ansi};
use super::geometry::{Box4, Geom, box_object, round};
use super::stamp::text_width;
use swirvo_core::EngineError;

/// One comment on one page.
#[derive(Debug, Clone, PartialEq)]
pub enum Comment {
    /// A see-through marker over `rect` (multiplied, so the text under it stays black).
    Highlight { rect: Box4, color: [u8; 3] },
    /// A sticky note whose icon's top-left corner is at `at`.
    Note {
        at: [f32; 2],
        text: String,
        color: [u8; 3],
    },
    /// Freehand strokes.
    Ink {
        strokes: Vec<Vec<[f32; 2]>>,
        color: [u8; 3],
        width: f32,
    },
    /// A rectangle outline.
    Box {
        rect: Box4,
        color: [u8; 3],
        width: f32,
    },
    /// Text typed on the page, wrapped to `rect`'s width.
    Text {
        rect: Box4,
        text: String,
        size: f32,
        color: [u8; 3],
    },
}

/// Annotation flag: print the annotation with the page.
const PRINT: i64 = 4;
/// Notes keep their size and stay upright, as every viewer draws them.
const NO_ZOOM_ROTATE: i64 = 8 | 16;
const NOTE: f32 = 20.0;

fn name(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

fn rgb(c: [u8; 3]) -> [f32; 3] {
    c.map(|v| v as f32 / 255.0)
}

fn color_array(c: [u8; 3]) -> Object {
    Object::Array(rgb(c).iter().map(|&v| Object::Real(round(v))).collect())
}

fn n(v: f32) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

fn normalise(r: Box4) -> Box4 {
    [
        r[0].min(r[2]),
        r[1].min(r[3]),
        r[0].max(r[2]),
        r[1].max(r[3]),
    ]
}

/// A Form XObject drawn in display orientation over a `w`×`h` box. `turn` is the rotation
/// part of display → user space, so the viewer maps the turned box onto `/Rect`.
fn form(w: f32, h: f32, turn: [f32; 6], content: Vec<u8>, res: Dictionary) -> Stream {
    let mut d = Dictionary::new();
    d.set("Type", name("XObject"));
    d.set("Subtype", name("Form"));
    d.set("BBox", box_object([0.0, 0.0, w, h]));
    if turn != [1.0, 0.0, 0.0, 1.0, 0.0, 0.0] {
        d.set(
            "Matrix",
            Object::Array(turn.iter().map(|&v| Object::Real(v)).collect()),
        );
    }
    d.set("Resources", Object::Dictionary(res));
    Stream::new(d, content)
}

fn helvetica() -> Dictionary {
    let mut font = Dictionary::new();
    font.set("Type", name("Font"));
    font.set("Subtype", name("Type1"));
    font.set("BaseFont", name("Helvetica"));
    font.set("Encoding", name("WinAnsiEncoding"));
    let mut fonts = Dictionary::new();
    fonts.set("Helv", Object::Dictionary(font));
    let mut res = Dictionary::new();
    res.set("Font", Object::Dictionary(fonts));
    res
}

/// Greedy word wrap with Helvetica widths.
pub(crate) fn wrap(text: &str, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if !line.is_empty() && text_width(&candidate, size, false) > width {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out
}

/// True when [`Comment::Text`] can be an annotation: its text is in Helvetica's
/// character set. Other text is stamped onto the page instead (see `organise`).
pub fn typable(text: &str) -> bool {
    win_ansi(text).is_some()
}

/// Add `c` to page `page` (0-based).
pub fn add(doc: &mut Document, page: usize, c: &Comment) -> Result<(), EngineError> {
    let ids = super::pages(doc);
    let &pid = ids
        .get(page)
        .ok_or_else(|| EngineError::Invalid(format!("page {} doesn't exist", page + 1)))?;
    let g = Geom::of(doc, pid);
    let m = g.to_user();
    let turn = [m[0], m[1], m[2], m[3], 0.0, 0.0];
    let to_user = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let mut a = Dictionary::new();
    a.set("Type", name("Annot"));
    a.set("F", Object::Integer(PRINT));
    a.set("P", Object::Reference(pid));
    // Display box of the appearance, and its content in box-local coordinates.
    let (bbox, content, res, flags): (Box4, Vec<u8>, Dictionary, i64) = match c {
        Comment::Highlight { rect, color } => {
            let r = normalise(*rect);
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            a.set("Subtype", name("Highlight"));
            a.set("C", color_array(*color));
            let q: Vec<Object> = [(r[0], r[3]), (r[2], r[3]), (r[0], r[1]), (r[2], r[1])]
                .iter()
                .flat_map(|&(x, y)| {
                    let (ux, uy) = to_user(x, y);
                    [Object::Real(round(ux)), Object::Real(round(uy))]
                })
                .collect();
            a.set("QuadPoints", Object::Array(q));
            let [cr, cg, cb] = rgb(*color);
            let mut gs = Dictionary::new();
            gs.set("Type", name("ExtGState"));
            gs.set("BM", name("Multiply"));
            let mut gss = Dictionary::new();
            gss.set("GS0", Object::Dictionary(gs));
            let mut res = Dictionary::new();
            res.set("ExtGState", Object::Dictionary(gss));
            let s = format!(
                "q /GS0 gs {} {} {} rg 0 0 {} {} re f Q",
                n(cr),
                n(cg),
                n(cb),
                n(w),
                n(h)
            );
            (r, s.into_bytes(), res, PRINT)
        }
        Comment::Box { rect, color, width } => {
            let r = normalise(*rect);
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let lw = width.max(0.25);
            a.set("Subtype", name("Square"));
            a.set("C", color_array(*color));
            let mut bs = Dictionary::new();
            bs.set("W", Object::Real(lw));
            a.set("BS", Object::Dictionary(bs));
            let [cr, cg, cb] = rgb(*color);
            let s = format!(
                "q {} w {} {} {} RG {} {} {} {} re S Q",
                n(lw),
                n(cr),
                n(cg),
                n(cb),
                n(lw / 2.0),
                n(lw / 2.0),
                n((w - lw).max(0.0)),
                n((h - lw).max(0.0))
            );
            (r, s.into_bytes(), Dictionary::new(), PRINT)
        }
        Comment::Ink {
            strokes,
            color,
            width,
        } => {
            let lw = width.max(0.25);
            let pts = strokes.iter().flatten();
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for p in pts {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            if x0 > x1 {
                return Err(EngineError::Invalid("an ink comment needs a stroke".into()));
            }
            let pad = lw;
            let r = [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
            a.set("Subtype", name("Ink"));
            a.set("C", color_array(*color));
            let mut bs = Dictionary::new();
            bs.set("W", Object::Real(lw));
            a.set("BS", Object::Dictionary(bs));
            let list: Vec<Object> = strokes
                .iter()
                .map(|s| {
                    Object::Array(
                        s.iter()
                            .flat_map(|p| {
                                let (ux, uy) = to_user(p[0], p[1]);
                                [Object::Real(round(ux)), Object::Real(round(uy))]
                            })
                            .collect(),
                    )
                })
                .collect();
            a.set("InkList", Object::Array(list));
            let [cr, cg, cb] = rgb(*color);
            let mut s = format!("q {} w 1 J 1 j {} {} {} RG ", n(lw), n(cr), n(cg), n(cb));
            for stroke in strokes {
                for (k, p) in stroke.iter().enumerate() {
                    let op = if k == 0 { "m" } else { "l" };
                    s.push_str(&format!("{} {} {op} ", n(p[0] - r[0]), n(p[1] - r[1])));
                }
                if stroke.len() == 1 {
                    // A dot: a zero-length line with round caps.
                    let p = stroke[0];
                    s.push_str(&format!("{} {} l ", n(p[0] - r[0]), n(p[1] - r[1])));
                }
                s.push_str("S ");
            }
            s.push('Q');
            (r, s.into_bytes(), Dictionary::new(), PRINT)
        }
        Comment::Note { at, text, color } => {
            a.set("Subtype", name("Text"));
            a.set("Name", name("Comment"));
            a.set("C", color_array(*color));
            a.set("Contents", text_string(text));
            let [cr, cg, cb] = rgb(*color);
            // A speech bubble: filled square with a tail, three lines of "text".
            let s = format!(
                "q {} {} {} rg 0.2 G 0.6 w 1 4 m 1 19 l 19 19 l 19 4 l 9 4 l 5 0.5 l 6 4 l h B \
                 0.25 G 4 15 m 16 15 l 4 11.5 m 16 11.5 l 4 8 m 12 8 l S Q",
                n(cr),
                n(cg),
                n(cb)
            );
            // With NoRotate a viewer keeps the upper-left corner of /Rect where it is and
            // draws the icon upright from there, so that corner goes where the reader put it
            // and the appearance isn't turned.
            let (ux, uy) = to_user(at[0], at[1]);
            let ap = doc.add_object(form(
                NOTE,
                NOTE,
                [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                s.into_bytes(),
                Dictionary::new(),
            ));
            a.set("Rect", box_object([ux, uy - NOTE, ux + NOTE, uy]));
            a.set("F", Object::Integer(PRINT | NO_ZOOM_ROTATE));
            let mut apd = Dictionary::new();
            apd.set("N", Object::Reference(ap));
            a.set("AP", Object::Dictionary(apd));
            return push(doc, pid, a);
        }
        Comment::Text {
            rect,
            text,
            size,
            color,
        } => {
            let r = normalise(*rect);
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let size = size.clamp(4.0, 144.0);
            let [cr, cg, cb] = rgb(*color);
            a.set("Subtype", name("FreeText"));
            a.set("Contents", text_string(text));
            let da = format!("/Helv {} Tf {} {} {} rg", n(size), n(cr), n(cg), n(cb));
            a.set("DA", Object::string_literal(da.clone()));
            let mut s = format!("/Tx BMC q 0 0 {} {} re W n BT {da} ", n(w), n(h)).into_bytes();
            let lead = size * 1.15;
            for (k, line) in wrap(text, size, w - 4.0).iter().enumerate() {
                let Some(bytes) = win_ansi(line) else {
                    return Err(EngineError::Invalid(
                        "that text needs a font with more characters".into(),
                    ));
                };
                let y = h - 2.0 - size * 0.9 - k as f32 * lead;
                s.extend(format!("1 0 0 1 2 {} Tm ", n(y)).into_bytes());
                s.extend(escape(&bytes));
                s.extend(b" Tj ");
            }
            s.extend(b"ET Q EMC");
            (r, s, helvetica(), PRINT)
        }
    };
    let (w, h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    a.set("Rect", box_object(g.rect_to_user(bbox)));
    a.set("F", Object::Integer(flags));
    let ap = doc.add_object(form(w, h, turn, content, res));
    let mut apd = Dictionary::new();
    apd.set("N", Object::Reference(ap));
    a.set("AP", Object::Dictionary(apd));
    push(doc, pid, a)
}

/// Append annotation `a` to the page's `/Annots`, which may be inline or indirect.
fn push(doc: &mut Document, page: ObjectId, a: Dictionary) -> Result<(), EngineError> {
    let id = doc.add_object(Object::Dictionary(a));
    let existing = doc
        .get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Annots").ok())
        .cloned();
    match existing {
        Some(Object::Reference(arr)) => {
            if let Ok(Object::Array(v)) = doc.get_object_mut(arr) {
                v.push(Object::Reference(id));
                return Ok(());
            }
            set_annots(doc, page, vec![Object::Reference(id)])
        }
        Some(Object::Array(mut v)) => {
            v.push(Object::Reference(id));
            set_annots(doc, page, v)
        }
        _ => set_annots(doc, page, vec![Object::Reference(id)]),
    }
}

fn set_annots(doc: &mut Document, page: ObjectId, v: Vec<Object>) -> Result<(), EngineError> {
    doc.get_dictionary_mut(page)
        .map_err(EngineError::encode)?
        .set("Annots", Object::Array(v));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_tools::PageRenderer;

    fn pixel(img: &image::RgbaImage, fx: f32, fy: f32) -> [u8; 3] {
        let p = img.get_pixel(
            ((img.width() as f32 * fx) as u32).min(img.width() - 1),
            ((img.height() as f32 * fy) as u32).min(img.height() - 1),
        );
        [p[0], p[1], p[2]]
    }

    /// Every kind lands where it was put, upright, on plain and rotated pages, and is a
    /// real annotation a viewer can list.
    #[test]
    fn comments_show_where_they_were_put() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&src, 2, false);
        let rot = d.path().join("r.pdf");
        crate::pdf_tools::rotate_pages(&src, &rot, 90, Some(&[1])).unwrap();
        let mut doc = crate::pdfedit::open(&rot, None).unwrap();
        for page in 0..2 {
            let g = Geom::of(&doc, super::super::pages(&doc)[page]);
            let (w, h) = g.display_size();
            // Top-left quarter: a red box; bottom-right: a yellow highlight.
            add(
                &mut doc,
                page,
                &Comment::Box {
                    rect: [20.0, h - 120.0, 120.0, h - 20.0],
                    color: [220, 20, 20],
                    width: 6.0,
                },
            )
            .unwrap();
            add(
                &mut doc,
                page,
                &Comment::Highlight {
                    rect: [w - 160.0, 30.0, w - 30.0, 80.0],
                    color: [255, 230, 0],
                },
            )
            .unwrap();
            add(
                &mut doc,
                page,
                &Comment::Ink {
                    strokes: vec![vec![[w / 2.0 - 40.0, h / 2.0], [w / 2.0 + 40.0, h / 2.0]]],
                    color: [20, 20, 220],
                    width: 8.0,
                },
            )
            .unwrap();
            add(
                &mut doc,
                page,
                &Comment::Note {
                    at: [w - 60.0, h - 30.0],
                    text: "Check this".into(),
                    color: [255, 210, 0],
                },
            )
            .unwrap();
            add(
                &mut doc,
                page,
                &Comment::Text {
                    rect: [40.0, 200.0, 300.0, 260.0],
                    text: "Approved — see note".into(),
                    size: 14.0,
                    color: [0, 0, 0],
                },
            )
            .unwrap();
        }
        let out = d.path().join("c.pdf");
        crate::pdfedit::save(&mut doc, &out, false).unwrap();

        let back = lopdf::Document::load(&out).unwrap();
        for pid in super::super::pages(&back) {
            let annots = back.get_page_annotations(pid).unwrap();
            let kinds: Vec<Vec<u8>> = annots
                .iter()
                .map(|a| a.get(b"Subtype").unwrap().as_name().unwrap().to_vec())
                .filter(|k| k != b"Link")
                .collect();
            assert_eq!(
                kinds,
                [
                    b"Square".to_vec(),
                    b"Highlight".to_vec(),
                    b"Ink".to_vec(),
                    b"Text".to_vec(),
                    b"FreeText".to_vec()
                ]
            );
        }
        let r = PageRenderer::open(&out).unwrap();
        r.session(|s| {
            for page in 0..2 {
                let img = s.render_fit(page, 400, 400).unwrap();
                let (iw, ih) = (img.width() as f32, img.height() as f32);
                let (w, h) = if page == 0 {
                    (595.0, 842.0)
                } else {
                    (842.0, 595.0)
                };
                assert!(
                    (iw / ih - w / h).abs() < 0.02,
                    "page {page} is shown at its display size"
                );
                // Box edge: left side at x = 20..26 pt, y from the top 20..120.
                let red = pixel(&img, 23.0 / w, 70.0 / h);
                assert!(red[0] > 150 && red[1] < 90, "box on page {page}: {red:?}");
                let yellow = pixel(&img, (w - 95.0) / w, (h - 55.0) / h);
                assert!(
                    yellow[0] > 200 && yellow[1] > 180 && yellow[2] < 90,
                    "highlight on page {page}: {yellow:?}"
                );
                let blue = pixel(&img, 0.5, 0.5);
                assert!(
                    blue[2] > 150 && blue[0] < 90,
                    "ink on page {page}: {blue:?}"
                );
            }
        });
    }

    #[test]
    fn text_wraps_to_its_box() {
        let lines = wrap("one two three four five six seven", 12.0, 80.0);
        assert!(lines.len() > 2, "{lines:?}");
        assert!(
            lines
                .iter()
                .all(|l| text_width(l, 12.0, false) <= 80.0 || !l.contains(' '))
        );
        assert!(typable("Café — 25 €"));
        assert!(!typable("日本"));
    }
}
