//! Archival (PDF/A-2) and tagged output, checked by krilla's validator while writing and
//! by reading the structure tree back.

use std::io::{Read as _, Write as _};

use lopdf::{Document, Object};
use swirvo_core::{CancelToken, PdfOptions};

use super::tests::sample_docx;
use super::{convert_as, fonts};

const ARCHIVAL_TAGGED: PdfOptions = PdfOptions {
    archival: true,
    tagged: true,
    attach_source: false,
    accessible: false,
};

/// A 4×4 red PNG.
fn red_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([220, 20, 20, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

/// The sample document with a language and a described picture added.
fn sample_with_lang_and_picture() -> Vec<u8> {
    let src = sample_docx();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(src)).unwrap();
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o = zip::write::SimpleFileOptions::default();
        for i in 0..zin.len() {
            let mut f = zin.by_index(i).unwrap();
            let name = f.name().unwrap().to_string();
            let mut s = String::new();
            f.read_to_string(&mut s).unwrap();
            let s = match name.as_str() {
                "word/styles.xml" => s.replacen(
                    r#"<w:sz w:val="22"/></w:rPr></w:rPrDefault>"#,
                    r#"<w:sz w:val="22"/><w:lang w:val="en-GB"/></w:rPr></w:rPrDefault>"#,
                    1,
                ),
                "word/document.xml" => s.replacen(
                    "<w:p><w:r><w:br w:type=\"page\"/>",
                    r#"<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Picture 1" descr="A red square"/><a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="rIdImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p><w:p><w:r><w:br w:type="page"/>"#,
                    1,
                ),
                "word/_rels/document.xml.rels" => s.replacen(
                    "</Relationships>",
                    r#"<Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#,
                    1,
                ),
                _ => s,
            };
            z.start_file(name, o).unwrap();
            z.write_all(s.as_bytes()).unwrap();
        }
        z.start_file("word/media/image1.png", o).unwrap();
        z.write_all(&red_png()).unwrap();
        z.finish().unwrap();
    }
    buf
}

fn name(o: &Object) -> String {
    o.as_name()
        .map(|n| String::from_utf8_lossy(n).into_owned())
        .unwrap_or_default()
}

/// Structure types in document order, with each figure's alternative text.
fn roles(doc: &Document) -> (Vec<String>, Vec<String>) {
    fn walk(doc: &Document, o: &Object, out: &mut Vec<String>, alts: &mut Vec<String>, depth: u32) {
        if depth > 64 {
            return;
        }
        let o = match o {
            Object::Reference(id) => match doc.get_object(*id) {
                Ok(x) => x,
                Err(_) => return,
            },
            x => x,
        };
        match o {
            Object::Array(a) => {
                for k in a {
                    walk(doc, k, out, alts, depth + 1);
                }
            }
            Object::Dictionary(d) => {
                if let Ok(s) = d.get(b"S") {
                    let s = name(s);
                    if s == "Figure"
                        && let Ok(Object::String(alt, _)) = d.get(b"Alt")
                    {
                        alts.push(String::from_utf8_lossy(alt).into_owned());
                    }
                    out.push(s);
                }
                if let Ok(k) = d.get(b"K") {
                    walk(doc, k, out, alts, depth + 1);
                }
            }
            _ => {}
        }
    }
    let root = doc
        .catalog()
        .unwrap()
        .get(b"StructTreeRoot")
        .and_then(|r| r.as_reference())
        .and_then(|id| doc.get_object(id))
        .cloned();
    let mut out = Vec::new();
    let mut alts = Vec::new();
    if let Ok(Object::Dictionary(d)) = root
        && let Ok(k) = d.get(b"K")
    {
        walk(doc, k, &mut out, &mut alts, 0);
    }
    (out, alts)
}

fn xmp(doc: &Document) -> String {
    let id = doc
        .catalog()
        .unwrap()
        .get(b"Metadata")
        .and_then(|r| r.as_reference())
        .unwrap();
    let s = doc.get_object(id).unwrap().as_stream().unwrap();
    String::from_utf8_lossy(&s.content).into_owned()
}

/// `(part, conformance)` from the XMP identification, e.g. ("2", "A").
fn pdfa_id(x: &str) -> (String, String) {
    let grab = |tag: &str| {
        let attr = format!("pdfaid:{tag}=\"");
        let el = format!("<pdfaid:{tag}>");
        x.find(&attr)
            .map(|i| &x[i + attr.len()..])
            .and_then(|r| r.split('"').next())
            .or_else(|| {
                x.find(&el)
                    .map(|i| &x[i + el.len()..])
                    .and_then(|r| r.split('<').next())
            })
            .unwrap_or_default()
            .to_string()
    };
    (grab("part"), grab("conformance"))
}

fn page_content(doc: &Document) -> String {
    let mut all = String::new();
    for (_, id) in doc.get_pages() {
        all.push_str(&String::from_utf8_lossy(&doc.get_page_content(id).unwrap()));
    }
    all
}

#[test]
fn word_document_as_accessible_pdfa() {
    if !fonts::Fonts::available() {
        eprintln!("SKIPPED: no system fonts");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("a.pdf");
    let bytes = sample_with_lang_and_picture();
    convert_as(
        &bytes,
        "docx",
        "sample",
        &out,
        ARCHIVAL_TAGGED,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&out).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)), ("2".into(), "A".into()), "PDF/A-2a");
    let cat = doc.catalog().unwrap();
    let marked = cat.get(b"MarkInfo").ok().and_then(|m| match m {
        Object::Dictionary(d) => d.get(b"Marked").ok().cloned(),
        Object::Reference(id) => doc
            .get_dictionary(*id)
            .ok()
            .and_then(|d| d.get(b"Marked").ok().cloned()),
        _ => None,
    });
    assert_eq!(marked, Some(Object::Boolean(true)));
    let (r, alts) = roles(&doc);
    for want in [
        "H1", "P", "L", "LI", "LBody", "Table", "TR", "TH", "TD", "Note", "Figure", "Link",
    ] {
        assert!(r.iter().any(|x| x == want), "no {want} in {r:?}");
    }
    assert_eq!(alts, ["A red square"]);
    // Reading order: the title before the heading, the heading before the list, the table
    // header before its body cells.
    let pos = |s: &str| r.iter().position(|x| x == s).unwrap();
    assert!(pos("H1") < pos("L") && pos("L") < pos("Table"), "{r:?}");
    assert!(pos("TH") < pos("TD"), "{r:?}");
    // The footer ("Page 1 of 2") is a pagination artifact, not content.
    assert!(
        page_content(&doc).contains("/Artifact"),
        "artifacts are marked"
    );
    let (r2, _) = roles(&doc);
    assert_eq!(r, r2);
}

#[test]
fn archival_without_a_language_is_pdfa_2b_and_still_tagged() {
    if !fonts::Fonts::available() {
        eprintln!("SKIPPED: no system fonts");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("b.pdf");
    convert_as(
        &sample_docx(),
        "docx",
        "sample",
        &out,
        ARCHIVAL_TAGGED,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&out).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)), ("2".into(), "B".into()));
    assert!(roles(&doc).0.iter().any(|r| r == "Table"));

    // Archival only: PDF/A-2b, no structure tree; plain: neither.
    let plain_a = d.path().join("c.pdf");
    let opts = PdfOptions {
        archival: true,
        tagged: false,
        ..PdfOptions::default()
    };
    convert_as(
        &sample_docx(),
        "docx",
        "s",
        &plain_a,
        opts,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&plain_a).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)), ("2".into(), "B".into()));
    assert!(doc.catalog().unwrap().get(b"StructTreeRoot").is_err());
    let plain = d.path().join("d.pdf");
    convert_as(
        &sample_docx(),
        "docx",
        "s",
        &plain,
        PdfOptions::default(),
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&plain).unwrap();
    assert!(doc.catalog().unwrap().get(b"StructTreeRoot").is_err());
    assert!(
        doc.catalog()
            .unwrap()
            .get(b"Metadata")
            .ok()
            .is_none_or(|_| pdfa_id(&xmp(&doc)).0.is_empty())
    );
}

#[test]
fn slides_and_sheets_as_tagged_pdfa() {
    if !fonts::Fonts::available() {
        eprintln!("SKIPPED: no system fonts");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let deck = d.path().join("deck.pdf");
    crate::pptxlayout::convert_as(
        &crate::pptxlayout::tests::sample_pptx(),
        "deck",
        &deck,
        ARCHIVAL_TAGGED,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&deck).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)).0, "2");
    let (r, _) = roles(&doc);
    assert!(
        r.iter().any(|x| x == "H1"),
        "slide titles are headings: {r:?}"
    );
    assert!(r.iter().any(|x| x == "P"), "{r:?}");

    let book = d.path().join("book.pdf");
    crate::xlsxlayout::convert_as(
        &crate::xlsxlayout::tests::sample_xlsx(),
        "book",
        &book,
        ARCHIVAL_TAGGED,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&book).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)).0, "2");
    assert!(!roles(&doc).0.is_empty());
}

/// The attachments' file specifications in the catalog's EmbeddedFiles name tree, and
/// the catalog's associated files (`/AF`).
fn attachments(doc: &Document) -> (Vec<(String, String, usize)>, usize) {
    let cat = doc.catalog().unwrap();
    let deref = |o: &Object| -> Object {
        match o {
            Object::Reference(id) => doc.get_object(*id).unwrap().clone(),
            o => o.clone(),
        }
    };
    let af = cat
        .get(b"AF")
        .ok()
        .map(deref)
        .and_then(|a| a.as_array().ok().map(Vec::len))
        .unwrap_or(0);
    let mut out = Vec::new();
    let Ok(names) = cat.get(b"Names").map(deref) else {
        return (out, af);
    };
    let names = names.as_dict().unwrap();
    let Ok(ef) = names.get(b"EmbeddedFiles").map(deref) else {
        return (out, af);
    };
    let arr = deref(ef.as_dict().unwrap().get(b"Names").unwrap());
    for pair in arr.as_array().unwrap().chunks(2) {
        let spec = deref(&pair[1]);
        let spec = spec.as_dict().unwrap();
        let name = match spec.get(b"F").map(deref) {
            Ok(Object::String(b, _)) => String::from_utf8_lossy(&b).into_owned(),
            _ => String::new(),
        };
        let rel = match spec.get(b"AFRelationship") {
            Ok(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
            _ => String::new(),
        };
        let f = deref(spec.get(b"EF").unwrap());
        let stream = deref(f.as_dict().unwrap().get(b"F").unwrap());
        let stream = stream.as_stream().unwrap();
        let len = stream
            .decompressed_content()
            .map_or(stream.content.len(), |c| c.len());
        out.push((name, rel, len));
    }
    (out, af)
}

#[test]
fn pdfa3_carries_the_original_document() {
    if !fonts::Fonts::available() {
        eprintln!("SKIPPED: no system fonts");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let bytes = sample_with_lang_and_picture();
    let out = d.path().join("a3.pdf");
    let opts = PdfOptions {
        attach_source: true,
        ..ARCHIVAL_TAGGED
    };
    convert_as(&bytes, "docx", "sample", &out, opts, &CancelToken::new()).unwrap();
    let doc = Document::load(&out).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)), ("3".into(), "A".into()), "PDF/A-3a");
    let (files, af) = attachments(&doc);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].0, "sample.docx");
    assert_eq!(files[0].1, "Source");
    assert_eq!(files[0].2, bytes.len(), "the original, byte for byte");
    assert_eq!(af, 1, "listed as the document's associated file");

    // Without a language: PDF/A-3b, still with the original. Slides too.
    let out = d.path().join("b3.pdf");
    convert_as(&sample_docx(), "docx", "s", &out, opts, &CancelToken::new()).unwrap();
    let doc = Document::load(&out).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)), ("3".into(), "B".into()));
    let deck = d.path().join("deck.pdf");
    crate::pptxlayout::convert_as(
        &crate::pptxlayout::tests::sample_pptx(),
        "deck",
        &deck,
        opts,
        &CancelToken::new(),
    )
    .unwrap();
    let doc = Document::load(&deck).unwrap();
    assert_eq!(pdfa_id(&xmp(&doc)).0, "3");
    assert_eq!(attachments(&doc).0[0].0, "deck.pptx");

    // PDF/A-2 never attaches anything.
    let out = d.path().join("a2.pdf");
    convert_as(
        &bytes,
        "docx",
        "s",
        &out,
        ARCHIVAL_TAGGED,
        &CancelToken::new(),
    )
    .unwrap();
    assert!(attachments(&Document::load(&out).unwrap()).0.is_empty());
}

#[test]
fn pdfua_is_tagged_titled_and_navigable() {
    if !fonts::Fonts::available() {
        eprintln!("SKIPPED: no system fonts");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let ua = PdfOptions {
        accessible: true,
        ..PdfOptions::default()
    };
    // The plain sample has no language and no described picture: still PDF/UA, with
    // the file name as the title and an outline from its headings.
    for (name, bytes) in [
        ("plain", sample_docx()),
        ("lang", sample_with_lang_and_picture()),
    ] {
        let out = d.path().join(format!("{name}.pdf"));
        convert_as(&bytes, "docx", name, &out, ua, &CancelToken::new()).unwrap();
        let raw = std::fs::read(&out).unwrap();
        assert_eq!(super::pdfua_part(&raw).as_deref(), Some("1"), "{name}");
        assert_eq!(super::pdfa_level(&raw), None, "UA only");
        let doc = Document::load(&out).unwrap();
        let cat = doc.catalog().unwrap();
        assert!(cat.get(b"StructTreeRoot").is_ok(), "tagged");
        assert!(cat.get(b"Outlines").is_ok(), "an outline");
        let vp = match cat.get(b"ViewerPreferences").unwrap() {
            Object::Reference(id) => doc.get_dictionary(*id).unwrap().clone(),
            Object::Dictionary(d) => d.clone(),
            o => panic!("{o:?}"),
        };
        assert_eq!(
            vp.get(b"DisplayDocTitle").unwrap(),
            &Object::Boolean(true),
            "the title shows in the title bar"
        );
        assert!(roles(&doc).0.iter().any(|r| r == "Table"));
    }
    // Sheets have no headings: the outline is the document itself.
    let book = d.path().join("book.pdf");
    crate::xlsxlayout::convert_as(
        &crate::xlsxlayout::tests::sample_xlsx(),
        "book",
        &book,
        ua,
        &CancelToken::new(),
    )
    .unwrap();
    let raw = std::fs::read(&book).unwrap();
    assert_eq!(super::pdfua_part(&raw).as_deref(), Some("1"));
    assert!(
        Document::load(&book)
            .unwrap()
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .is_ok()
    );
}
