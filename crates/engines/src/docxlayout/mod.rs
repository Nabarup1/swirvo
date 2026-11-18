//! DOCX → PDF with the document's own layout (docs/16): page size and margins, sections
//! and columns, styles and themes, fonts, lists, tabs, tables, pictures, text boxes,
//! headers and footers with page numbers, footnotes, links and bookmarks.
//!
//! Stages: `read` (package → resolved [`model::Document`]) → `layout` (pages of
//! positioned operations, with `text` breaking lines and `fonts` measuring and shaping)
//! → `render` (krilla PDF). No other program is involved.

pub(crate) mod fonts;
pub(crate) mod layout;
pub(crate) mod model;
pub(crate) mod numbering;
mod odt;
pub(crate) mod props;
mod read;
pub(crate) mod render;
pub(crate) mod rtf;
mod styles;
pub(crate) mod tags;
pub(crate) mod text;
pub(crate) mod theme;
pub(crate) mod xml;

use std::path::Path;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, PdfOptions, StepRequest};

/// Convert DOCX bytes to a PDF at `out`; returns the page count. Panics inside the
/// pipeline become errors, so one odd document can't take the caller down.
pub fn convert(
    bytes: &[u8],
    title: &str,
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    convert_with(read::read, bytes, title, out, PdfOptions::default(), cancel)
}

/// A Word, ODT (`from` = "odt") or RTF ("rtf") document to a PDF at `out`, archival and/or
/// tagged as `opts` asks.
pub fn convert_as(
    bytes: &[u8],
    from: &str,
    title: &str,
    out: &Path,
    opts: PdfOptions,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let (reader, kind): (Reader, _) = match from {
        "odt" => (odt::read, "odt"),
        "rtf" => (rtf::read, "rtf"),
        _ => (read::read, "docx"),
    };
    let name = format!("{title}.{kind}");
    let source = render::Source {
        name: &name,
        kind,
        bytes,
    };
    convert_from(reader, bytes, title, out, opts, Some(source), cancel)
}

/// ODT bytes to a PDF at `out`, through the same layout.
pub fn convert_odt(
    bytes: &[u8],
    title: &str,
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    convert_with(odt::read, bytes, title, out, PdfOptions::default(), cancel)
}

/// RTF bytes to a PDF at `out`, through the same layout.
pub fn convert_rtf(
    bytes: &[u8],
    title: &str,
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    convert_with(rtf::read, bytes, title, out, PdfOptions::default(), cancel)
}

/// A document reader: DOCX, ODT or RTF bytes to the layout model.
type Reader = fn(&[u8]) -> Result<model::Document, EngineError>;

fn convert_with(
    reader: Reader,
    bytes: &[u8],
    title: &str,
    out: &Path,
    opts: PdfOptions,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    convert_from(reader, bytes, title, out, opts, None, cancel)
}

fn convert_from(
    reader: Reader,
    bytes: &[u8],
    title: &str,
    out: &Path,
    opts: PdfOptions,
    source: Option<render::Source<'_>>,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = reader(bytes)?;
        cancel.check()?;
        if !fonts::Fonts::available() {
            return Err(EngineError::Encode(
                "no fonts are installed, so the PDF can't be typeset".into(),
            ));
        }
        let fonts = fonts::Fonts::new(doc.generics.clone());
        let tagged = opts.tagged || opts.accessible;
        let (pages, mut fonts, tags) = layout::layout_with(&doc, fonts, tagged);
        cancel.check()?;
        render::render_from(
            &pages,
            &mut fonts,
            &doc.meta,
            title,
            out,
            opts,
            tags.as_ref(),
            source,
        )?;
        Ok(pages.len())
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this document".into()))?
}

/// The first page of a DOCX (`"docx"`), ODT (`"odt"`) or RTF (`"rtf"`) document as a
/// one-page PDF in memory (thumbnails).
pub fn first_page(bytes: &[u8], kind: &str, cancel: &CancelToken) -> Result<Vec<u8>, EngineError> {
    let reader: fn(&[u8]) -> Result<model::Document, EngineError> = match kind {
        "odt" => odt::read,
        "rtf" => rtf::read,
        _ => read::read,
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = reader(bytes)?;
        cancel.check()?;
        if !fonts::Fonts::available() {
            return Err(EngineError::Encode("no fonts".into()));
        }
        let (pages, mut fonts) = layout::layout(&doc, fonts::Fonts::new(doc.generics.clone()));
        cancel.check()?;
        render::render_bytes(&pages[..pages.len().min(1)], &mut fonts, &doc.meta, "")
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this document".into()))?
}

/// The PDF/A level a PDF declares in its XMP metadata ("2A", "2B", "3U"…), if any. Only
/// what the file says; files we write are checked against the standard while writing.
pub fn pdfa_level(pdf: &[u8]) -> Option<String> {
    let (part, conformance) = xmp_id(pdf, "pdfaid")?;
    Some(format!(
        "{part}{}",
        conformance.unwrap_or_default().to_ascii_uppercase()
    ))
}

/// The PDF/UA part a PDF declares in its XMP metadata ("1"), if any.
pub fn pdfua_part(pdf: &[u8]) -> Option<String> {
    xmp_id(pdf, "pdfuaid").map(|(part, _)| part)
}

/// `ns:part` and `ns:conformance` from the catalog's XMP, as attributes or elements.
fn xmp_id(pdf: &[u8], ns: &str) -> Option<(String, Option<String>)> {
    let doc = lopdf::Document::load_mem(pdf).ok()?;
    let id = doc
        .catalog()
        .ok()?
        .get(b"Metadata")
        .ok()?
        .as_reference()
        .ok()?;
    let stream = doc.get_object(id).ok()?.as_stream().ok()?;
    let content = stream
        .decompressed_content()
        .unwrap_or_else(|_| stream.content.clone());
    let x = String::from_utf8_lossy(&content);
    let grab = |tag: &str| {
        let attr = format!("{ns}:{tag}=\"");
        let el = format!("<{ns}:{tag}>");
        x.find(&attr)
            .map(|i| &x[i + attr.len()..])
            .and_then(|r| r.split('"').next())
            .or_else(|| {
                x.find(&el)
                    .map(|i| &x[i + el.len()..])
                    .and_then(|r| r.split('<').next())
            })
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    Some((grab("part")?, grab("conformance")))
}

/// Pages the document lays out to (for statistics), without writing a PDF.
pub fn page_count(bytes: &[u8]) -> Result<usize, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let doc = read::read(bytes)?;
        if !fonts::Fonts::available() {
            return Err(EngineError::Encode("no fonts".into()));
        }
        let (pages, _) = layout::layout(&doc, fonts::Fonts::new(doc.generics.clone()));
        Ok(pages.len())
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this document".into()))?
}

pub struct DocxLayoutEngine;

impl Engine for DocxLayoutEngine {
    fn id(&self) -> &'static str {
        "docx-layout"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("docx-layout", "docx", "pdf")
                .loss(1)
                .time(60, 40)
                .note("lays out the pages: fonts, tables, pictures, headers, footnotes"),
            Capability::new("docx-layout", "odt", "pdf")
                .loss(2)
                .time(60, 40)
                .note("lays out the pages: styles, lists, tables, pictures, headers, notes"),
            Capability::new("docx-layout", "rtf", "pdf")
                .loss(2)
                .time(40, 40)
                .note("lays out the pages: formatting, tables, pictures, headers, notes"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let bytes = std::fs::read(req.input)?;
        let title = req
            .input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        convert_as(&bytes, req.from, &title, req.output, req.pdf, req.cancel).map(|_| ())
    }
}

#[cfg(test)]
mod tests_pdfa;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write as _;

    /// A small but realistic Word document: theme fonts, styles, a numbered list, a table
    /// with a header row, a page break, a footer with PAGE / NUMPAGES, a footnote, a link.
    pub(crate) fn sample_docx() -> Vec<u8> {
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
        let document = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document {w}><w:body>
<w:p><w:pPr><w:pStyle w:val="Title"/></w:pPr><w:r><w:t>Quarterly report</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Revenue grew </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>12 %</w:t></w:r><w:r><w:t xml:space="preserve"> this quarter, see </w:t></w:r><w:hyperlink r:id="rIdLink"><w:r><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr><w:t>our site</w:t></w:r></w:hyperlink><w:r><w:t>.</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Highlights</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>New office</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Two hires</w:t></w:r></w:p>
<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="5000" w:type="pct"/><w:tblLook w:firstRow="1"/></w:tblPr>
<w:tblGrid><w:gridCol w:w="4680"/><w:gridCol w:w="4680"/></w:tblGrid>
<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:shd w:val="clear" w:fill="D9E2F3"/></w:tcPr><w:p><w:r><w:t>Region</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>Sales</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:p><w:r><w:t>North</w:t></w:r></w:p></w:tc><w:tc><w:p><w:pPr><w:jc w:val="right"/></w:pPr><w:r><w:t>10</w:t></w:r></w:p></w:tc></w:tr>
</w:tbl>
<w:p><w:r><w:br w:type="page"/></w:r><w:r><w:t>Second page.</w:t></w:r></w:p>
<w:sectPr><w:footerReference w:type="default" r:id="rIdFooter"/><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr>
</w:body></w:document>"#
        );
        let styles = format!(
            r#"<w:styles {w}>
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"/><w:sz w:val="22"/></w:rPr></w:rPrDefault>
<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="259" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Title"><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:rPr><w:rFonts w:asciiTheme="majorHAnsi" w:hAnsiTheme="majorHAnsi"/><w:sz w:val="56"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:basedOn w:val="Normal"/><w:pPr><w:keepNext/><w:spacing w:before="240"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:color w:val="2F5496"/><w:sz w:val="32"/></w:rPr></w:style>
<w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:tblPr><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:basedOn w:val="TableNormal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:color="auto"/><w:left w:val="single" w:sz="4" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:color="auto"/><w:right w:val="single" w:sz="4" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:color="auto"/></w:tblBorders></w:tblPr>
<w:tblStylePr w:type="firstRow"><w:rPr><w:b/></w:rPr></w:tblStylePr></w:style>
</w:styles>"#
        );
        let numbering = format!(
            r#"<w:numbering {w}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
        );
        let footer = format!(
            r#"<w:ftr {w}><w:p><w:pPr><w:jc w:val="center"/></w:pPr><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> of </w:t></w:r><w:fldSimple w:instr=" NUMPAGES "><w:r><w:t>1</w:t></w:r></w:fldSimple></w:p></w:ftr>"#
        );
        let footnotes = format!(
            r#"<w:footnotes {w}><w:footnote w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:footnote><w:footnote w:id="1"><w:p><w:r><w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:footnoteRef/></w:r><w:r><w:t xml:space="preserve"> Unaudited figures.</w:t></w:r></w:p></w:footnote></w:footnotes>"#
        );
        let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:accent1><a:srgbClr val="4472C4"/></a:accent1></a:clrScheme><a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
        let rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rIdNum" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
<Relationship Id="rIdTheme" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="theme/theme1.xml"/>
<Relationship Id="rIdFooter" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>
<Relationship Id="rIdNotes" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes" Target="footnotes.xml"/>
<Relationship Id="rIdLink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/>
</Relationships>"#;
        let root_rels = r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("_rels/.rels", root_rels.to_string()),
                ("word/document.xml", document),
                ("word/_rels/document.xml.rels", rels.to_string()),
                ("word/styles.xml", styles),
                ("word/numbering.xml", numbering),
                ("word/footer1.xml", footer),
                ("word/footnotes.xml", footnotes),
                ("word/theme/theme1.xml", theme.to_string()),
            ] {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn reads_the_model() {
        if let Some(p) = std::env::var_os("DOCX_SAMPLE_OUT") {
            std::fs::write(p, sample_docx()).unwrap();
        }
        let doc = read::read(&sample_docx()).unwrap();
        assert_eq!(doc.sections.len(), 1);
        let s = &doc.sections[0];
        assert!((s.sect.w - 595.3).abs() < 0.5, "A4 width {}", s.sect.w);
        let paras: Vec<&model::Para> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                model::Block::Para(p) => Some(&**p),
                _ => None,
            })
            .collect();
        // Theme fonts resolve; the title uses the major font at 28 pt.
        let title = match &paras[0].items[0] {
            model::Item::Text { rs, .. } => rs.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(title.fonts[0], "Calibri Light");
        assert_eq!(title.sz, 28.0);
        // Lists are numbered in order, with the level's hanging indent.
        let labels: Vec<&str> = paras
            .iter()
            .filter_map(|p| p.label.as_ref().map(|l| l.text.as_str()))
            .collect();
        assert_eq!(labels, ["1.", "2."]);
        let item = paras.iter().find(|p| p.label.is_some()).unwrap();
        assert_eq!((item.p.left, item.p.first), (36.0, -18.0));
        // Heading 1 is an outline level and keeps with the next paragraph.
        let h = paras.iter().find(|p| p.outline == Some(0)).unwrap();
        assert!(h.p.keep_next);
        // The table: grid, header row, style borders, first-row bold from the style.
        let t = s
            .blocks
            .iter()
            .find_map(|b| match b {
                model::Block::Table(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert_eq!(t.grid, vec![234.0, 234.0]);
        assert!(t.rows[0].header);
        assert!(t.rows[0].cells[0].borders[0].is_some_and(|b| b.visible()));
        let cell_run = match &t.rows[0].cells[1].blocks[0] {
            model::Block::Para(p) => match &p.items[0] {
                model::Item::Text { rs, .. } => rs.clone(),
                _ => panic!(),
            },
            _ => panic!(),
        };
        assert!(cell_run.b, "firstRow bold from the table style");
        // Table Grid's spacing (after = 0) beats Normal's 8 pt inside the table.
        if let model::Block::Para(p) = &t.rows[1].cells[0].blocks[0] {
            assert_eq!(p.p.after, 0.0);
        }
        assert!(doc.uses_numpages);
        assert!(doc.footnotes.contains_key("1"));
        assert!(doc.parts.contains_key("rIdFooter"));
    }

    #[test]
    fn converts_to_paged_pdf() {
        if !fonts::Fonts::available() {
            eprintln!("SKIPPED: no system fonts");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.pdf");
        let pages = convert(&sample_docx(), "sample", &out, &CancelToken::new()).unwrap();
        assert_eq!(pages, 2, "the page break starts page 2");
        // Read it back through our PDF layout analysis.
        let bytes = std::fs::read(&out).unwrap();
        let l = crate::pdflayout::analyze(&bytes, &CancelToken::new()).unwrap();
        assert_eq!(l.pages.len(), 2);
        assert!((l.pages[0].w - 595.3).abs() < 0.6);
        let text = crate::pdflayout::to_doc(&l);
        let all = format!("{text:?}");
        for needle in [
            "Quarterly report",
            "Highlights",
            "New office",
            "Region",
            "Second page",
            "Unaudited",
        ] {
            assert!(all.contains(needle), "missing {needle:?} in {all}");
        }
        // The footer repeats on both pages, so the analysis files it as furniture.
        let furniture = format!("{:?}", l.footer);
        assert!(
            furniture.contains("Page") && furniture.contains("of"),
            "{furniture}"
        );
    }

    /// A body-only package: the reader falls back to defaults for styles, theme and fonts.
    fn docx_with_body(body: &str) -> Vec<u8> {
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
        let document = format!(r#"<w:document {w}><w:body>{body}</w:body></w:document>"#);
        let root_rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [("_rels/.rels", root_rels), ("word/document.xml", &document)] {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        buf
    }

    /// Tables nested 60 deep convert on a 1 MB stack (a Windows main thread's default):
    /// levels past the cap are left out instead of recursing until the stack overflows.
    #[test]
    fn deeply_nested_tables_stay_within_a_small_stack() {
        if !fonts::Fonts::available() {
            eprintln!("SKIPPED: no system fonts");
            return;
        }
        let depth = 80;
        let mut body = String::new();
        for i in 0..depth {
            body.push_str(&format!(
                r#"<w:tbl><w:tblGrid><w:gridCol w:w="9000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>level {i}</w:t></w:r></w:p>"#
            ));
        }
        body.push_str(&"<w:p/></w:tc></w:tr></w:tbl>".repeat(depth));
        body.push_str("<w:p><w:r><w:t>after</w:t></w:r></w:p>");
        let bytes = docx_with_body(&body);
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("nested.pdf");
        let out2 = out.clone();
        let pages = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || convert(&bytes, "nested", &out2, &CancelToken::new()))
            .unwrap()
            .join()
            .expect("no stack overflow")
            .unwrap();
        assert!(pages >= 1);
        let text = format!(
            "{:?}",
            crate::pdflayout::to_doc(
                &crate::pdflayout::analyze(&std::fs::read(&out).unwrap(), &CancelToken::new())
                    .unwrap()
            )
        );
        assert!(text.contains("level 0") && text.contains("after"), "{text}");
    }

    /// A picture taller and wider than the page, with text after it in the same
    /// paragraph: orphan control once moved the paragraph to the next page forever.
    #[test]
    fn a_line_taller_than_the_page_doesnt_loop() {
        if !fonts::Fonts::available() {
            eprintln!("SKIPPED: no system fonts");
            return;
        }
        let pic = r#"<w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="29260800" cy="18288000"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;
        let body = format!(
            r#"<w:p><w:r><w:t>before</w:t></w:r></w:p><w:p><w:pPr><w:widowControl/></w:pPr>{pic}<w:r><w:t xml:space="preserve"> after the picture</w:t></w:r></w:p><w:p><w:r><w:t>last</w:t></w:r></w:p>"#
        );
        let bytes = docx_with_body(&body);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dir = tempfile::tempdir().unwrap();
            let out = dir.path().join("tall.pdf");
            let _ = tx.send(
                convert(&bytes, "tall", &out, &CancelToken::new()).map_err(|e| e.to_string()),
            );
        });
        let pages = rx
            .recv_timeout(std::time::Duration::from_secs(60))
            .expect("layout finishes")
            .unwrap();
        assert!((2..=4).contains(&pages), "{pages}");
    }

    /// A part nested far past any real document is a clean error, not a crash.
    #[test]
    fn absurd_nesting_is_an_error() {
        let n = 100_000;
        let body = format!("{}{}", "<w:sdt>".repeat(n), "</w:sdt>".repeat(n));
        let r = std::thread::Builder::new()
            .stack_size(1 << 20)
            .spawn(move || {
                convert(
                    &docx_with_body(&body),
                    "x",
                    Path::new("unused.pdf"),
                    &CancelToken::new(),
                )
            })
            .unwrap()
            .join()
            .expect("no stack overflow");
        assert!(matches!(r, Err(EngineError::Decode(_))), "{r:?}");
    }
}
