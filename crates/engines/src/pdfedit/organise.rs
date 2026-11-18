//! Organise by hand (docs/17 §2, the page picker's Organise mode): a new PDF from a page
//! plan (reordered, duplicated, inserted from other PDFs, blank pages), each page turned
//! and cropped as the reader chose, with redaction boxes, signatures and comments.
//!
//! Order matters and is fixed:
//! 1. **Pages.** The plan is assembled (re-embedded with krilla). When the plan is the
//!    first file's pages in their own order, the file is edited in place instead, so its
//!    links, forms, bookmarks and metadata survive.
//! 2. **Turn and crop**, in place. Crops are drawn on the page as the reader sees it after
//!    turning, so they are applied after it.
//! 3. **Redaction** ([`super::redact`]), which rebuilds the marked pages from pixels.
//! 4. **Signatures** (pictures stamped into the page content) and **comments**
//!    (annotations), after redaction so they aren't flattened with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swirvo_core::{CancelToken, EngineError};

use super::annotate::{self, Comment};
use super::assemble::{self, PageSrc};
use super::geometry::{Box4, Geom};
use super::redact::{self, Area, Find};
use super::stamp::{self, HAlign, Item};

/// One page of the result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sheet {
    pub src: PageSrc,
    /// Extra clockwise turn: 0, 90, 180 or 270.
    pub turn: i64,
    /// The part to keep, in display points of the turned page (y up).
    pub crop: Option<Box4>,
}

impl Sheet {
    pub fn page(file: usize, page: usize) -> Sheet {
        Sheet {
            src: PageSrc::Page { file, page },
            turn: 0,
            crop: None,
        }
    }
}

/// Something put on page `page` (0-based, of the result), in display points of the turned
/// page before cropping: what the reader sees in the editor, where a crop only dims the
/// outside.
#[derive(Debug, Clone, PartialEq)]
pub enum Mark {
    /// Black out everything in `rect`; the page is rebuilt from pixels.
    Redact {
        page: usize,
        rect: Box4,
    },
    /// A PNG or JPEG (a signature, a stamp) fitted into `rect`.
    Picture {
        page: usize,
        rect: Box4,
        image: Arc<Vec<u8>>,
    },
    Comment {
        page: usize,
        comment: Comment,
    },
}

impl Mark {
    fn page(&self) -> usize {
        match self {
            Mark::Redact { page, .. } | Mark::Picture { page, .. } | Mark::Comment { page, .. } => {
                *page
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Organised {
    pub pages: usize,
    /// True when the pages were re-embedded (links and forms aren't carried over then).
    pub rebuilt: bool,
    pub redacted: usize,
    pub pictures: usize,
    pub comments: usize,
    /// Text comments whose characters Helvetica lacks, drawn into the page instead.
    pub stamped_text: usize,
}

/// True when `sheets` are file 0's `n` pages in their own order, untouched by the plan.
fn is_identity(sheets: &[Sheet], n: usize) -> bool {
    sheets.len() == n
        && sheets
            .iter()
            .enumerate()
            .all(|(i, s)| s.src == PageSrc::Page { file: 0, page: i })
}

/// Build `out` from `inputs` (file 0 is the document being organised).
pub fn organise(
    inputs: &[PathBuf],
    sheets: &[Sheet],
    marks: &[Mark],
    out: &Path,
    cancel: &CancelToken,
) -> Result<Organised, EngineError> {
    if sheets.is_empty() {
        return Err(EngineError::Invalid("keep at least one page".into()));
    }
    if let Some(m) = marks.iter().find(|m| m.page() >= sheets.len()) {
        return Err(EngineError::Invalid(format!(
            "page {} doesn't exist",
            m.page() + 1
        )));
    }
    for s in sheets {
        if s.turn.rem_euclid(90) != 0 {
            return Err(EngineError::Invalid("pages turn in quarter turns".into()));
        }
    }
    let first = inputs
        .first()
        .ok_or_else(|| EngineError::Invalid("no PDF to organise".into()))?;
    let tmp = tempfile::tempdir()?;
    let mut rep = Organised {
        pages: sheets.len(),
        ..Organised::default()
    };

    // 1. Pages.
    let step1 = tmp.path().join("1.pdf");
    let n0 = assemble::page_count(first)?;
    if is_identity(sheets, n0) {
        std::fs::copy(first, &step1)?;
    } else {
        let plan: Vec<PageSrc> = sheets.iter().map(|s| s.src).collect();
        assemble::assemble(inputs, &plan, &step1, cancel)?;
        rep.rebuilt = true;
    }
    cancel.check()?;

    // 2. Turn and crop.
    let mut doc = super::open(&step1, None)?;
    let ids = super::pages(&doc);
    for (s, &pid) in sheets.iter().zip(&ids) {
        if s.turn.rem_euclid(360) != 0 {
            let g = Geom::of(&doc, pid);
            let d = doc.get_dictionary_mut(pid).map_err(EngineError::encode)?;
            d.set(
                "Rotate",
                lopdf::Object::Integer((g.rotate + s.turn).rem_euclid(360)),
            );
        }
        if let Some(c) = s.crop {
            let g = Geom::of(&doc, pid);
            let (w, h) = g.display_size();
            let c = [
                c[0].clamp(0.0, w),
                c[1].clamp(0.0, h),
                c[2].clamp(0.0, w),
                c[3].clamp(0.0, h),
            ];
            let c = [
                c[0].min(c[2]),
                c[1].min(c[3]),
                c[0].max(c[2]),
                c[1].max(c[3]),
            ];
            if c[2] - c[0] < 10.0 || c[3] - c[1] < 10.0 {
                return Err(EngineError::Invalid("that crop is too small".into()));
            }
            assemble::set_crop(&mut doc, pid, &g, c)?;
        }
    }
    // Marks are placed on the uncropped page; after cropping, display coordinates start
    // at the crop's corner.
    let offsets: Vec<(f32, f32)> = sheets
        .iter()
        .map(|s| {
            s.crop
                .map_or((0.0, 0.0), |c| (c[0].min(c[2]), c[1].min(c[3])))
        })
        .collect();
    let shift = |page: usize, r: Box4| -> Box4 {
        let (dx, dy) = offsets[page];
        [r[0] - dx, r[1] - dy, r[2] - dx, r[3] - dy]
    };
    let step2 = tmp.path().join("2.pdf");
    super::save(&mut doc, &step2, false)?;
    cancel.check()?;

    // 3. Redaction.
    let areas: Vec<Area> = marks
        .iter()
        .filter_map(|m| match m {
            Mark::Redact { page, rect } => Some(Area {
                page: *page,
                rect: shift(*page, *rect),
            }),
            _ => None,
        })
        .collect();
    let step3 = if areas.is_empty() {
        step2
    } else {
        let p = tmp.path().join("3.pdf");
        let r = redact::redact(&step2, &p, &Find::default(), &areas, false, cancel)?;
        rep.redacted = r.pages;
        p
    };

    // 4. Pictures and comments.
    let mut doc = super::open(&step3, None)?;
    let mut items: Vec<Vec<Item>> = vec![Vec::new(); sheets.len()];
    for m in marks {
        match m {
            Mark::Picture { page, rect, image } => {
                items[*page].push(Item::Image {
                    bytes: image.clone(),
                    rect: shift(*page, *rect),
                    opacity: 1.0,
                });
                rep.pictures += 1;
            }
            Mark::Comment {
                page,
                comment:
                    Comment::Text {
                        rect,
                        text,
                        size,
                        color,
                    },
            } if !annotate::typable(text) => {
                // Helvetica can't show it: draw it into the page in a system font.
                let r = shift(*page, *rect);
                let lead = size * 1.15;
                for (k, line) in annotate::wrap(text, *size, r[2] - r[0] - 4.0)
                    .into_iter()
                    .enumerate()
                {
                    items[*page].push(Item::Text {
                        text: line,
                        size: *size,
                        color: *color,
                        opacity: 1.0,
                        x: r[0] + 2.0,
                        y: r[3] - 2.0 - size * 0.9 - k as f32 * lead,
                        align: HAlign::Left,
                        angle: 0.0,
                        bold: false,
                    });
                }
                rep.stamped_text += 1;
            }
            _ => {}
        }
    }
    if items.iter().any(|v| !v.is_empty()) {
        let pages: Vec<usize> = (0..items.len()).filter(|&i| !items[i].is_empty()).collect();
        stamp::stamp_doc(&mut doc, Some(&pages), cancel, |p| items[p.index].clone())?;
    }
    for m in marks {
        if let Mark::Comment { page, comment } = m {
            if let Comment::Text { text, .. } = comment
                && !annotate::typable(text)
            {
                continue;
            }
            annotate::add(&mut doc, *page, &moved(comment, offsets[*page]))?;
            rep.comments += 1;
        }
    }
    super::save(&mut doc, out, false)?;
    Ok(rep)
}

/// `c` with its coordinates moved by `-(dx, dy)`.
fn moved(c: &Comment, (dx, dy): (f32, f32)) -> Comment {
    let r = |b: &Box4| [b[0] - dx, b[1] - dy, b[2] - dx, b[3] - dy];
    let p = |q: &[f32; 2]| [q[0] - dx, q[1] - dy];
    match c {
        Comment::Highlight { rect, color } => Comment::Highlight {
            rect: r(rect),
            color: *color,
        },
        Comment::Box { rect, color, width } => Comment::Box {
            rect: r(rect),
            color: *color,
            width: *width,
        },
        Comment::Text {
            rect,
            text,
            size,
            color,
        } => Comment::Text {
            rect: r(rect),
            text: text.clone(),
            size: *size,
            color: *color,
        },
        Comment::Note { at, text, color } => Comment::Note {
            at: p(at),
            text: text.clone(),
            color: *color,
        },
        Comment::Ink {
            strokes,
            color,
            width,
        } => Comment::Ink {
            strokes: strokes.iter().map(|s| s.iter().map(p).collect()).collect(),
            color: *color,
            width: *width,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf_tools::PageRenderer;

    fn png(color: [u8; 4]) -> Arc<Vec<u8>> {
        let img = image::RgbaImage::from_pixel(40, 20, image::Rgba(color));
        let mut v = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
            .unwrap();
        Arc::new(v)
    }

    #[test]
    fn keeps_the_file_when_only_marking_and_rebuilds_for_a_new_order() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.pdf");
        let b = d.path().join("b.pdf");
        crate::pdfedit::testing::sample(&a, 3, false);
        crate::pdfedit::testing::sample(&b, 2, true);
        let c = CancelToken::new();

        // Same pages, one turned and cropped, with a note: edited in place.
        let mut sheets: Vec<Sheet> = (0..3).map(|i| Sheet::page(0, i)).collect();
        sheets[1].turn = 90;
        sheets[2].crop = Some([50.0, 50.0, 545.0, 792.0]);
        let out = d.path().join("o1.pdf");
        let r = organise(
            std::slice::from_ref(&a),
            &sheets,
            &[Mark::Comment {
                page: 2,
                comment: Comment::Note {
                    at: [100.0, 700.0],
                    text: "hi".into(),
                    color: [255, 210, 0],
                },
            }],
            &out,
            &c,
        )
        .unwrap();
        assert!(!r.rebuilt);
        assert_eq!(r.comments, 1);
        let doc = lopdf::Document::load(&out).unwrap();
        let ids = crate::pdfedit::pages(&doc);
        assert_eq!(Geom::of(&doc, ids[1]).display_size(), (842.0, 595.0));
        assert_eq!(Geom::of(&doc, ids[2]).display_size(), (495.0, 742.0));
        // The note keeps its place on the reader's page: 100 pt from the left of the
        // uncropped page is 50 pt from the crop's edge.
        let annot = &doc.get_page_annotations(ids[2]).unwrap()[0];
        let rect = annot.get(b"Rect").unwrap().as_array().unwrap();
        assert_eq!(rect[0].as_float().unwrap(), 100.0);

        // New order, a blank page, a page from another file, a signature and a box.
        let sheets = vec![
            Sheet::page(0, 2),
            Sheet {
                src: PageSrc::Blank { w: 595.0, h: 842.0 },
                turn: 0,
                crop: None,
            },
            Sheet::page(1, 0),
            Sheet::page(0, 0),
        ];
        let out = d.path().join("o2.pdf");
        let r = organise(
            &[a.clone(), b.clone()],
            &sheets,
            &[
                Mark::Picture {
                    page: 1,
                    rect: [100.0, 100.0, 300.0, 200.0],
                    image: png([200, 0, 0, 255]),
                },
                Mark::Redact {
                    page: 3,
                    rect: [0.0, 0.0, 595.0, 421.0],
                },
            ],
            &out,
            &c,
        )
        .unwrap();
        assert!(r.rebuilt);
        assert_eq!((r.pages, r.pictures, r.redacted), (4, 1, 1));
        let renderer = PageRenderer::open(&out).unwrap();
        assert_eq!(renderer.count(), 4);
        assert_eq!(renderer.size(2), Some((842.0, 595.0)));
        renderer.session(|s| {
            let img = s.render_fit(1, 595, 842).unwrap();
            let px = img.get_pixel(200, 842 - 150);
            assert!(px[0] > 150 && px[1] < 80, "signature drawn: {px:?}");
            let img = s.render_fit(3, 595, 842).unwrap();
            let px = img.get_pixel(300, 800);
            assert!(px[0] < 40 && px[1] < 40, "bottom half blacked out: {px:?}");
        });
    }

    #[test]
    fn text_outside_helvetica_is_drawn_into_the_page() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&a, 1, false);
        let out = d.path().join("o.pdf");
        let r = organise(
            &[a],
            &[Sheet::page(0, 0)],
            &[Mark::Comment {
                page: 0,
                comment: Comment::Text {
                    rect: [50.0, 600.0, 400.0, 650.0],
                    text: "Ελληνικά".into(),
                    size: 14.0,
                    color: [0, 0, 0],
                },
            }],
            &out,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!((r.comments, r.stamped_text), (0, 1));
    }

    #[test]
    fn refuses_impossible_plans() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&a, 1, false);
        let out = d.path().join("o.pdf");
        let c = CancelToken::new();
        assert!(organise(std::slice::from_ref(&a), &[], &[], &out, &c).is_err());
        let bad_mark = Mark::Redact {
            page: 4,
            rect: [0.0, 0.0, 10.0, 10.0],
        };
        assert!(
            organise(
                std::slice::from_ref(&a),
                &[Sheet::page(0, 0)],
                &[bad_mark],
                &out,
                &c
            )
            .is_err()
        );
        let mut s = Sheet::page(0, 0);
        s.crop = Some([0.0, 0.0, 3.0, 3.0]);
        assert!(organise(&[a], &[s], &[], &out, &c).is_err());
        assert!(!out.exists());
    }
}
