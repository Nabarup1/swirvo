//! PDF layout reconstruction (docs/13): rebuild each page (fonts, columns, tables, pictures,
//! boxes, headers and footers) instead of pouring extracted text into a template.
//!
//! Stages, each a pure function of the previous one's output:
//! `extract` (hayro device) → `tables` / figures / decorations → `text` (rows, segments) →
//! `order` (XY-cut) → `paras` → [`model::Layout`] → `docx` writer, or `to_doc` for the
//! document model the other writers share.

mod build;
mod docx;
pub(crate) mod extract;
mod fonts;
pub mod model;
mod order;
mod paras;
mod raster;
mod tables;
mod text;
mod to_doc;
mod xlsx;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, StepRequest};

pub use to_doc::to_doc;

/// Analyse a PDF. Panics inside the PDF stack become decode errors.
pub fn analyze(bytes: &[u8], cancel: &CancelToken) -> Result<model::Layout, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build::analyze(bytes, cancel)
    }))
    .map_err(|_| EngineError::Decode("couldn't read the layout of this PDF".into()))?
}

/// Write a layout as DOCX.
pub fn write_docx(
    l: &model::Layout,
    title: &str,
    out: &std::path::Path,
) -> Result<(), EngineError> {
    docx::write(l, title, out)
}

pub struct PdfLayoutEngine;

impl Engine for PdfLayoutEngine {
    fn id(&self) -> &'static str {
        "pdf-layout"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("pdf-layout", "pdf", "docx")
                .loss(12)
                .time(80, 400)
                .note("rebuilds the pages: fonts, columns, tables, pictures, boxes, headers"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        crate::pdfedit::secure::permits(req.input, crate::pdfedit::secure::Need::Copy)?;
        let bytes = std::fs::read(req.input)?;
        let title = req
            .input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let l = analyze(&bytes, req.cancel)?;
        req.cancel.check()?;
        if l.has_text {
            return docx::write(&l, &title, req.output);
        }
        // No text layer: the document path recognizes it with OCR.
        let doc = crate::doc::read_path(req.input, "pdf")?;
        req.cancel.check()?;
        crate::doc::write_path(&doc, "docx", req.output)
    }
}

/// PDF → Excel: the tables (and the text around them) as cells.
pub struct PdfTablesEngine;

impl Engine for PdfTablesEngine {
    fn id(&self) -> &'static str {
        "pdf-tables"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("pdf-tables", "pdf", "xlsx")
                .loss(20)
                .time(80, 400)
                .note("tables become cells, numbers stay numbers; a sheet per page"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        crate::pdfedit::secure::permits(req.input, crate::pdfedit::secure::Need::Copy)?;
        let bytes = std::fs::read(req.input)?;
        let mut l = analyze(&bytes, req.cancel)?;
        req.cancel.check()?;
        if !l.has_text {
            // A scan: read it first, then lay out the recognized text.
            let dir = tempfile::tempdir()?;
            let ocr = dir.path().join("read.pdf");
            crate::pdfedit::searchable::make_searchable(req.input, &ocr, false, req.cancel)?;
            l = analyze(&std::fs::read(&ocr)?, req.cancel)?;
        }
        xlsx::write(&l, req.output)
    }
}

/// Debugging aid for `examples/pdf_layout`: raw glyphs and vector marks of one page.
#[doc(hidden)]
pub fn debug_page(bytes: &[u8], page: usize) -> Result<String, EngineError> {
    use std::fmt::Write as _;
    let ex = extract::extract(bytes, &CancelToken::new())?;
    let Some(p) = ex.pages.get(page) else {
        return Ok(String::new());
    };
    let mut s = String::new();
    for g in &p.glyphs {
        let _ = writeln!(
            s,
            "G {:?} x {:.1}..{:.1} base {:.1} size {:.1} font {} {:?}{}{}",
            g.ch,
            g.x0,
            g.x1,
            g.base,
            g.size,
            g.font,
            g.color,
            if g.invisible { " invisible" } else { "" },
            if g.stroked { " stroked" } else { "" }
        );
    }
    for m in &p.marks {
        let _ = writeln!(s, "M {m:?}");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::model::{Block, Piece};
    use super::*;
    use std::io::Read as _;

    /// Markdown → PDF with our own typesetter, then back through the layout engine.
    fn sample_pdf(dir: &std::path::Path) -> Option<Vec<u8>> {
        let md = dir.join("in.md");
        std::fs::write(
            &md,
            "# Quarterly report\n\nRevenue grew **12 %** this *quarter*.\n\n## Highlights\n\n\
             - New office\n- Two hires\n\n| Region | Sales |\n|---|---|\n| North | 10 |\n\
             | South & East | 7 |\n\nClosing words for the report.\n",
        )
        .ok()?;
        let doc = crate::doc::read_path(&md, "md").ok()?;
        let pdf = dir.join("in.pdf");
        match crate::doc::write_path(&doc, "pdf", &pdf) {
            Err(EngineError::Encode(m)) if m.contains("no system font") => return None,
            r => r.ok()?,
        }
        std::fs::read(&pdf).ok()
    }

    #[test]
    fn pdf_to_docx_keeps_structure() {
        let dir = tempfile::tempdir().unwrap();
        let Some(bytes) = sample_pdf(dir.path()) else {
            eprintln!("SKIPPED: no system fonts");
            return; // headless box without fonts
        };
        let l = analyze(&bytes, &CancelToken::new()).unwrap();
        assert!(l.has_text);
        assert_eq!(l.pages.len(), 1);
        let blocks: Vec<&Block> = l.pages[0]
            .bands
            .iter()
            .flat_map(|b| b.cols.iter().flat_map(|c| c.blocks.iter()))
            .collect();
        let para = |t: &str| {
            blocks.iter().find_map(|b| match b {
                Block::Para(p) if p.text().contains(t) => Some(p),
                _ => None,
            })
        };
        let title = para("Quarterly report").unwrap();
        assert_eq!(title.heading, Some(1));
        assert!(para("New office").unwrap().bullet.is_some());
        let bold = para("Revenue grew")
            .unwrap()
            .lines
            .iter()
            .flatten()
            .any(|x| matches!(x, Piece::Run(r) if r.bold && r.text.contains("12")));
        assert!(bold, "bold survives");
        let table = blocks
            .iter()
            .find_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .expect("the ruled table is a table");
        assert_eq!((table.rows.len(), table.cols.len()), (3, 2));
        assert!(
            table.rows[2].cells[0].paras[0]
                .text()
                .contains("South & East"),
            "cell text in place"
        );
        // Reading order: the closing paragraph comes after the table.
        let pos = |f: &dyn Fn(&Block) -> bool| blocks.iter().position(|b| f(b)).unwrap();
        assert!(
            pos(&|b| matches!(b, Block::Table(_)))
                < pos(&|b| matches!(b, Block::Para(p) if p.text().contains("Closing")))
        );

        // The package opens and its XML is well formed.
        let out = dir.path().join("out.docx");
        write_docx(&l, "t", &out).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        for name in [
            "word/document.xml",
            "word/styles.xml",
            "word/numbering.xml",
            "word/fontTable.xml",
            "word/settings.xml",
            "[Content_Types].xml",
            "word/_rels/document.xml.rels",
        ] {
            let mut s = String::new();
            z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
            let mut r = quick_xml::Reader::from_str(&s);
            loop {
                match r.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(e) => panic!("{name}: {e}"),
                }
            }
            if name == "word/document.xml" {
                assert!(s.contains("<w:tbl>") && s.contains("w:numId") && s.contains("<w:pgSz"));
                assert!(s.contains("Quarterly report"));
            }
        }
    }

    #[test]
    fn engine_writes_docx() {
        let dir = tempfile::tempdir().unwrap();
        let Some(bytes) = sample_pdf(dir.path()) else {
            return;
        };
        let (src, out) = (dir.path().join("a.pdf"), dir.path().join("a.docx"));
        std::fs::write(&src, bytes).unwrap();
        PdfLayoutEngine
            .run(&StepRequest {
                input: &src,
                output: &out,
                from: "pdf",
                to: "docx",
                preset: swirvo_core::Preset::Balanced,
                pdf: Default::default(),
                cancel: &CancelToken::new(),
            })
            .unwrap();
        let doc = crate::doc::read_path(&out, "docx").unwrap();
        assert!(
            !doc.blocks.is_empty(),
            "our own DOCX reader can read it back"
        );
    }
}
