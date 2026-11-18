//! Make a scanned PDF searchable: recognize the text on each page with the OS OCR engine
//! and lay it over the page as invisible text, word for word where the reader sees it.
//! The page itself is untouched, so it looks exactly as before; text can now be found,
//! selected and copied.

use std::collections::HashMap;
use std::path::Path;

use lopdf::{Document, Object, ObjectId};
use swirvo_core::{CancelToken, EngineError};

use super::stamp::{self, Item};
use crate::ocr;
use crate::pdf_tools::PageRenderer;

/// Pages are recognized at this resolution: enough for 8 pt text, fast enough for long
/// scans.
const DPI: f32 = 300.0;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Searchable {
    /// Pages that got a text layer.
    pub pages: usize,
    /// Pages left alone because they already had text.
    pub skipped: usize,
    pub words: usize,
}

/// OCR the pages of `input` that have no text (with `all`, every page) into `out`.
pub fn make_searchable(
    input: &Path,
    out: &Path,
    all: bool,
    cancel: &CancelToken,
) -> Result<Searchable, EngineError> {
    if !ocr::available() {
        return Err(EngineError::Invalid(
            "this PC has no text recognition (OCR) engine".into(),
        ));
    }
    let mut doc = super::open(input, None)?;
    let ids = super::pages(&doc);
    let todo: Vec<usize> = (0..ids.len())
        .filter(|&i| all || !page_has_text(&doc, ids[i]))
        .collect();
    let mut rep = Searchable {
        skipped: ids.len() - todo.len(),
        ..Searchable::default()
    };
    if todo.is_empty() {
        return Err(EngineError::Invalid(
            "every page already has text you can search".into(),
        ));
    }
    let renderer = PageRenderer::open(input)?;
    // Per page: recognized lines in pixels, and the rendered size.
    let mut found: HashMap<usize, (Vec<ocr::Line>, (u32, u32))> = HashMap::new();
    renderer.session(|s| -> Result<(), EngineError> {
        for &i in &todo {
            cancel.check()?;
            let img = s.render_dpi(i, DPI)?;
            let lines = ocr::recognize_lines(&img)?;
            rep.words += lines.iter().map(|l| l.words.len()).sum::<usize>();
            found.insert(i, (lines, img.dimensions()));
        }
        Ok(())
    })?;
    if rep.words == 0 {
        return Err(EngineError::Invalid(
            "no text was recognized on the pages".into(),
        ));
    }
    rep.pages = stamp::stamp_doc(&mut doc, Some(&todo), cancel, |p| {
        found
            .get(&p.index)
            .map(|(lines, (iw, ih))| layer(lines, *iw, *ih, p.w, p.h))
            .unwrap_or_default()
    })?;
    super::save(&mut doc, out, false)?;
    Ok(rep)
}

/// Hidden text items for one page: OCR boxes (pixels, y down) → display points (y up).
pub(crate) fn layer(lines: &[ocr::Line], iw: u32, ih: u32, pw: f32, ph: f32) -> Vec<Item> {
    let kx = pw / iw.max(1) as f32;
    let ky = ph / ih.max(1) as f32;
    let mut out = Vec::new();
    for l in lines {
        let top = l.words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
        let bottom = l.words.iter().map(|w| w.y + w.h).fold(0.0, f32::max);
        let h = (bottom - top).max(1.0);
        // The box runs from ascender to descender; the baseline sits about a fifth up.
        let size = (h * ky * 0.95).max(1.0);
        let base = ph - (bottom - h * 0.2) * ky;
        for (j, w) in l.words.iter().enumerate() {
            if w.text.trim().is_empty() {
                continue;
            }
            // A trailing space up to the next word keeps words apart when copied.
            let (text, width) = match l.words.get(j + 1) {
                Some(next) if next.x > w.x + w.w => (format!("{} ", w.text), (next.x - w.x) * kx),
                _ => (w.text.clone(), w.w * kx),
            };
            out.push(Item::Hidden {
                text,
                size,
                x: w.x * kx,
                y: base,
                w: width.max(1.0),
            });
        }
    }
    out
}

/// Does the page (or a form it draws) show any text? Scans without an OCR layer don't.
pub(crate) fn page_has_text(doc: &Document, page: ObjectId) -> bool {
    let Ok(content) = doc.get_page_content(page) else {
        return false;
    };
    let res = super::inherited(doc, page, b"Resources").map(|r| super::deref(doc, r));
    shows_text(doc, &content, res, 0)
}

fn shows_text(doc: &Document, content: &[u8], res: Option<&Object>, depth: u8) -> bool {
    let Ok(ops) = lopdf::content::Content::decode(content) else {
        return false;
    };
    let mut forms = Vec::new();
    for op in &ops.operations {
        match op.operator.as_str() {
            "Tj" | "TJ" | "'" | "\"" => return true,
            "Do" => {
                if let Some(Object::Name(n)) = op.operands.first() {
                    forms.push(n.clone());
                }
            }
            _ => {}
        }
    }
    if depth >= 4 {
        return false;
    }
    let Some(Object::Dictionary(res)) = res else {
        return false;
    };
    let Ok(xo) = res.get(b"XObject").map(|o| super::deref(doc, o)) else {
        return false;
    };
    let Object::Dictionary(xo) = xo else {
        return false;
    };
    forms.iter().any(|n| {
        let Ok(Object::Reference(id)) = xo.get(n) else {
            return false;
        };
        let Ok(Object::Stream(st)) = doc.get_object(*id) else {
            return false;
        };
        if st.dict.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) != Some(&b"Form"[..]) {
            return false;
        }
        // A form without its own resources uses the page's.
        let inner = st
            .dict
            .get(b"Resources")
            .ok()
            .map(|r| super::deref(doc, r).clone())
            .unwrap_or_else(|| Object::Dictionary(res.clone()));
        let body = st
            .decompressed_content()
            .unwrap_or_else(|_| st.content.clone());
        shows_text(doc, &body, Some(&inner), depth + 1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_maps_pixels_to_points() {
        let lines = vec![ocr::Line {
            words: vec![
                ocr::Word {
                    text: "Hello".into(),
                    x: 100.0,
                    y: 200.0,
                    w: 200.0,
                    h: 50.0,
                },
                ocr::Word {
                    text: "world".into(),
                    x: 330.0,
                    y: 200.0,
                    w: 210.0,
                    h: 50.0,
                },
            ],
        }];
        // 1000 × 2000 px rendering of a 500 × 1000 pt page: half a point per pixel.
        let items = layer(&lines, 1000, 2000, 500.0, 1000.0);
        assert_eq!(items.len(), 2);
        let Item::Hidden {
            text,
            x,
            y,
            w,
            size,
        } = &items[0]
        else {
            panic!("hidden text expected");
        };
        assert_eq!(text, "Hello ");
        assert_eq!(*x, 50.0);
        assert_eq!(*w, 115.0, "stretched up to the next word");
        assert!((*size - 23.75).abs() < 0.01);
        // Bottom 250 px → 875 pt from the bottom, baseline 5 pt above.
        assert!((*y - 880.0).abs() < 0.01, "{y}");
    }

    #[test]
    fn hidden_text_is_found_but_not_seen() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("in.pdf");
        super::super::testing::sample(&src, 1, false);
        let doc = super::super::open(&src, None).unwrap();
        let id = super::super::pages(&doc)[0];
        assert!(!page_has_text(&doc, id), "the sample is drawn, like a scan");
        let out = d.path().join("out.pdf");
        let mut doc2 = super::super::open(&src, None).unwrap();
        let before = crate::pdf_tools::PageRenderer::open(&src)
            .unwrap()
            .session(|s| s.render_dpi(0, 40.0))
            .unwrap();
        stamp::stamp_doc(&mut doc2, None, &CancelToken::new(), |_| {
            vec![Item::Hidden {
                text: "Quarterly zebra".into(),
                size: 30.0,
                x: 60.0,
                y: 400.0,
                w: 300.0,
            }]
        })
        .unwrap();
        super::super::save(&mut doc2, &out, false).unwrap();
        let done = super::super::open(&out, None).unwrap();
        assert!(
            page_has_text(&done, super::super::pages(&done)[0]),
            "found inside the stamp's form"
        );
        let after = crate::pdf_tools::PageRenderer::open(&out)
            .unwrap()
            .session(|s| s.render_dpi(0, 40.0))
            .unwrap();
        assert_eq!(before.as_raw(), after.as_raw(), "nothing visible changed");
        // Real text, in the invisible mode, stretched to the word box.
        let g = crate::pdflayout::debug_page(&std::fs::read(&out).unwrap(), 0).unwrap();
        let hidden: String = g
            .lines()
            .filter(|l| l.ends_with(" invisible"))
            .filter_map(|l| l.split('\'').nth(1))
            .collect();
        assert_eq!(hidden, "Quarterly zebra", "{g}");
        let x1 = g
            .lines()
            .filter(|l| l.ends_with(" invisible"))
            .filter_map(|l| l.split("..").nth(1)?.split(' ').next()?.parse::<f32>().ok())
            .fold(0.0, f32::max);
        assert!((x1 - 360.0).abs() < 2.0, "ends at x + w: {x1}");
    }
}
