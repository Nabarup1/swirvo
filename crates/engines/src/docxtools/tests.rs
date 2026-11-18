use super::*;

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn docx(body: &str, extra: &[(&str, &str)]) -> Vec<u8> {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W}><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let mut parts: Vec<(String, String)> = vec![
        ("[Content_Types].xml".into(), r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/></Types>"#.into()),
        ("_rels/.rels".into(), r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.into()),
        ("word/document.xml".into(), doc),
        ("word/_rels/document.xml.rels".into(), r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdC" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/></Relationships>"#.into()),
        ("docProps/core.xml".into(), r#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Letter</dc:title><dc:creator>Jane Secret</dc:creator><cp:lastModifiedBy>John Secret</cp:lastModifiedBy></cp:coreProperties>"#.into()),
    ];
    for (n, b) in extra {
        parts.push((n.to_string(), b.to_string()));
    }
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o = zip::write::SimpleFileOptions::default();
        for (n, b) in parts {
            z.start_file(n, o).unwrap();
            z.write_all(b.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    buf
}

fn body_text(bytes: &[u8]) -> String {
    let pkg = Pkg::read(bytes).unwrap();
    let d = pkg.xml("word/document.xml").unwrap();
    let mut s = String::new();
    fn walk(e: &Elem, s: &mut String) {
        if e.local() == "p" {
            s.push_str(&para_text(e));
            s.push('|');
            return;
        }
        for c in e.children() {
            walk(c, s);
        }
    }
    walk(&d.root, &mut s);
    s
}

#[test]
fn replace_across_runs() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.docx");
    // "Quarterly" split by Word into three runs with different formatting.
    std::fs::write(&src, docx(r#"<w:p><w:r><w:t>Qua</w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>rter</w:t></w:r><w:r><w:t xml:space="preserve">ly report, quarterly.</w:t></w:r></w:p>"#, &[])).unwrap();
    let out = d.path().join("b.docx");
    assert_eq!(
        replace(&src, &out, "quarterly", "Annual", false, true).unwrap(),
        2
    );
    assert_eq!(
        body_text(&std::fs::read(&out).unwrap()),
        "Annual report, Annual.|"
    );
    // Case-sensitive: only the lower-case one.
    assert_eq!(
        replace(&src, &out, "quarterly", "yearly", true, false).unwrap(),
        1
    );
    assert_eq!(
        body_text(&std::fs::read(&out).unwrap()),
        "Quarterly report, yearly.|"
    );
}

#[test]
fn clean_removes_people_comments_and_accepts_changes() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.docx");
    let body = r#"<w:p w:rsidR="00AB12" w:rsidRDefault="00AB12"><w:commentRangeStart w:id="0"/><w:r><w:t xml:space="preserve">Keep </w:t></w:r><w:ins w:id="1" w:author="Jane"><w:r><w:t>added</w:t></w:r></w:ins><w:del w:id="2" w:author="Jane"><w:r><w:delText>gone</w:delText></w:r></w:del><w:commentRangeEnd w:id="0"/><w:r><w:commentReference w:id="0"/></w:r></w:p>"#;
    std::fs::write(&src, docx(body, &[("word/comments.xml", r#"<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="0" w:author="Jane"><w:p><w:r><w:t>secret note</w:t></w:r></w:p></w:comment></w:comments>"#)])).unwrap();
    let out = d.path().join("b.docx");
    let r = clean(&src, &out, true).unwrap();
    assert!(
        r.items.contains(&"author and company") && r.items.contains(&"comments"),
        "{r:?}"
    );
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(body_text(&bytes), "Keep added|");
    let pkg = Pkg::read(&bytes).unwrap();
    assert!(pkg.get("word/comments.xml").is_none());
    let all: String = pkg
        .parts
        .iter()
        .map(|(_, b)| String::from_utf8_lossy(b).into_owned())
        .collect();
    for gone in [
        "Jane Secret",
        "John Secret",
        "secret note",
        "rsidR",
        "commentReference",
    ] {
        assert!(!all.contains(gone), "{gone} survived");
    }
    // It still lays out.
    if crate::docxlayout::fonts::Fonts::available() {
        crate::docxlayout::convert(&bytes, "x", &d.path().join("x.pdf"), &CancelToken::new())
            .unwrap();
    }
}

#[test]
fn mail_merge_fills_placeholders_and_merge_fields() {
    let d = tempfile::tempdir().unwrap();
    let tpl = d.path().join("letter.docx");
    let body = r#"<w:p><w:r><w:t xml:space="preserve">Dear {{</w:t></w:r><w:r><w:t>Name}},</w:t></w:r></w:p><w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve"> MERGEFIELD City </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>«City»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t xml:space="preserve"> welcomes you.</w:t></w:r></w:p>"#;
    std::fs::write(&tpl, docx(body, &[])).unwrap();
    let data = d.path().join("people.csv");
    std::fs::write(&data, "Name,City\nAda,London\nAlan,Manchester\n").unwrap();
    let rows = data_rows(&data).unwrap();
    assert_eq!(rows.len(), 2);
    let outdir = d.path().join("out");
    std::fs::create_dir(&outdir).unwrap();
    let outs = mail_merge(
        &tpl,
        &rows,
        &outdir,
        "letter",
        Some("Name"),
        false,
        &CancelToken::new(),
    )
    .unwrap();
    assert_eq!(outs.len(), 2);
    assert!(outs[0].ends_with("letter Ada.docx"));
    let t = body_text(&std::fs::read(&outs[1]).unwrap());
    assert_eq!(t, "Dear Alan,|Manchester welcomes you.|");
}

#[test]
fn stats_count_words() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("a.docx");
    std::fs::write(&src, docx(r#"<w:p><w:r><w:t>One two three.</w:t></w:r></w:p><w:p/><w:p><w:r><w:t>Four</w:t></w:r></w:p>"#, &[])).unwrap();
    let s = stats(&src).unwrap();
    assert_eq!((s.words, s.paragraphs, s.chars), (4, 2, 18));
}
