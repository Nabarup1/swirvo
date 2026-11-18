use super::*;

fn sample() -> Doc {
    read::markdown(
        "# Quarterly report\n\nRevenue grew **12 %** this *quarter*, see [the site](https://example.com).\n\n\
         ## Highlights\n\n- New office\n- Two hires\n  1. Design\n  2. Backend\n\n> Keep going.\n\n\
         ```\nfn main() {}\n```\n\n| Region | Sales |\n|---|---|\n| North | 10 |\n| South & East | 7 |\n\n---\n\nUnicode: café — 日本語 ✓\n",
    )
}

#[test]
fn markdown_reader_builds_the_model() {
    let d = sample();
    assert!(matches!(&d.blocks[0], Block::Heading(1, i) if plain(i) == "Quarterly report"));
    let Block::Para(p) = &d.blocks[1] else {
        panic!("{:?}", d.blocks[1])
    };
    assert!(p.iter().any(|i| i.style.bold && i.text == "12 %"));
    assert!(
        p.iter()
            .any(|i| i.style.link.as_deref() == Some("https://example.com"))
    );
    let Some(Block::List {
        ordered: false,
        items,
    }) = d.blocks.iter().find(|b| matches!(b, Block::List { .. }))
    else {
        panic!()
    };
    assert_eq!(items.len(), 2);
    assert!(
        matches!(items[1].last(), Some(Block::List { ordered: true, items }) if items.len() == 2)
    );
    assert!(d.blocks.iter().any(
        |b| matches!(b, Block::Table { rows, header: true, .. } if rows[2][0] == "South & East")
    ));
    assert!(
        d.blocks
            .iter()
            .any(|b| matches!(b, Block::Code(c) if c == "fn main() {}"))
    );
}

fn round_trip(to: &str) -> Doc {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join(format!("out.{to}"));
    write_path(&sample(), to, &p).unwrap();
    read_path(&p, to).unwrap()
}

fn texts(d: &Doc) -> String {
    write::text(d)
}

#[test]
fn docx_round_trip_keeps_structure() {
    let d = round_trip("docx");
    assert!(
        matches!(&d.blocks[0], Block::Heading(1, i) if plain(i) == "Quarterly report"),
        "{:?}",
        d.blocks[0]
    );
    assert!(
        d.blocks
            .iter()
            .any(|b| matches!(b, Block::List { ordered: false, .. })),
        "{:?}",
        d.blocks
    );
    assert!(d.blocks.iter().any(|b| matches!(b, Block::Table { rows, .. } if rows.len() == 3 && rows[2][0] == "South & East")));
    let Block::Para(p) = &d.blocks[1] else {
        panic!()
    };
    assert!(p.iter().any(|i| i.style.bold && i.text.contains("12 %")));
    assert!(
        p.iter()
            .any(|i| i.style.link.as_deref() == Some("https://example.com"))
    );
    assert!(texts(&d).contains("café — 日本語 ✓"));
}

#[test]
fn odt_round_trip_keeps_structure() {
    let d = round_trip("odt");
    assert!(matches!(&d.blocks[0], Block::Heading(1, _)));
    assert!(
        d.blocks
            .iter()
            .any(|b| matches!(b, Block::Table { rows, .. } if rows.len() == 3))
    );
    assert!(d.blocks.iter().any(|b| matches!(b, Block::List { .. })));
    assert!(texts(&d).contains("Revenue grew 12 % this quarter"));
}

#[test]
fn html_and_epub_round_trip() {
    for to in ["html", "epub"] {
        let d = round_trip(to);
        assert!(
            d.blocks
                .iter()
                .any(|b| matches!(b, Block::Heading(2, i) if plain(i) == "Highlights")),
            "{to}: {:?}",
            d.blocks
        );
        assert!(texts(&d).contains("South & East"), "{to}");
    }
}

#[test]
fn markdown_writer_round_trips() {
    let d = sample();
    let md = write::markdown(&d);
    let back = read::markdown(&md);
    assert_eq!(write::text(&back), write::text(&d), "markdown was:\n{md}");
}

#[test]
fn pdf_typesets_and_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("out.pdf");
    match write_path(&sample(), "pdf", &p) {
        Err(EngineError::Encode(m)) if m.contains("no system font") => return, // headless CI box
        r => r.unwrap(),
    }
    let bytes = std::fs::read(&p).unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    let back = read::pdf(&bytes).unwrap();
    let t = texts(&back)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        t.contains("Quarterly report") && t.contains("Revenue grew 12 % this quarter"),
        "{t}"
    );
    assert!(t.contains("South & East"), "{t}");
    assert!(
        back.blocks
            .iter()
            .any(|b| matches!(b, Block::List { items, .. } if items.len() >= 2)),
        "bullets become a list: {:?}",
        back.blocks
    );
}

#[test]
fn rtf_reader() {
    let src = br"{\rtf1\ansi{\fonttbl{\f0 Arial;}}{\*\generator x;}\f0 Hello {\b bold} caf\'e9\par Second\u8212? line\par}";
    let d = read::rtf(src);
    assert_eq!(d.blocks.len(), 2, "{:?}", d.blocks);
    let Block::Para(p) = &d.blocks[0] else {
        panic!()
    };
    assert!(p.iter().any(|i| i.style.bold && i.text == "bold"));
    assert_eq!(plain(p), "Hello bold café");
    assert_eq!(
        plain(match &d.blocks[1] {
            Block::Para(p) => p,
            _ => panic!(),
        }),
        "Second— line"
    );
}

#[test]
fn html_reader_ignores_scripts_and_collapses_space() {
    let d = read::html(
        "<html><head><title>T</title><script>x()</script></head><body><p>a   <b>b</b>\n c</p><ul><li>one</li><li>two</li></ul></body></html>",
    );
    assert_eq!(d.title.as_deref(), Some("T"));
    assert!(matches!(&d.blocks[0], Block::Para(p) if plain(p) == "a b c"));
    assert!(matches!(&d.blocks[1], Block::List { items, .. } if items.len() == 2));
}

#[test]
fn capabilities_are_honest() {
    let caps = DocEngine.capabilities();
    let get = |f: &str, t: &str| {
        caps.iter()
            .find(|c| c.from == f && c.to == t)
            .map(|c| c.loss)
    };
    assert_eq!(get("pdf", "docx"), None, "pdflayout owns PDF → DOCX");
    assert!(
        get("pdf", "md") >= Some(20),
        "reflowing a PDF loses its layout"
    );
    assert_eq!(get("txt", "docx"), Some(0));
    assert_eq!(
        get("md", "html"),
        None,
        "markdown engine handles it directly"
    );
    assert_eq!(get("csv", "epub"), None);
}

// ---------------------------------------------------------------- pictures

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([200, 40, 40]));
    let mut v = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
        .unwrap();
    v
}

fn zip_of(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write as _;
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        for (name, body) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(body).unwrap();
        }
        z.finish().unwrap();
    }
    buf
}

fn pictures(d: &Doc) -> Vec<&Picture> {
    d.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Image(p) => Some(p),
            _ => None,
        })
        .collect()
}

/// `<img>` tags in a page, and how many embed their picture.
fn imgs(html: &str) -> (usize, usize) {
    (
        html.matches("<img ").count(),
        html.matches("src=\"data:image/").count(),
    )
}

#[test]
fn word_pictures_reach_the_web_page_and_linked_files_dont() {
    let doc = br#"<w:document xmlns:w="w" xmlns:wp="wp" xmlns:a="a" xmlns:r="r" xmlns:pic="pic"><w:body>
<w:p><w:r><w:t>Before</w:t></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1270000" cy="952500"/><wp:docPr id="1" name="Picture 1" descr="A red square"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="12700" cy="12700"/><wp:docPr id="2" name="Linked"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId6"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:t>After</w:t></w:r></w:p>
</w:body></w:document>"#;
    let rels = br#"<Relationships><Relationship Id="rId5" Type="image" Target="media/image1.png"/><Relationship Id="rId6" Type="image" Target="file:///C:/Users/someone/secret.png" TargetMode="External"/></Relationships>"#;
    let pic = png(40, 30);
    let bytes = zip_of(&[
        ("word/document.xml", doc),
        ("word/_rels/document.xml.rels", rels),
        ("word/media/image1.png", &pic),
    ]);
    let d = read::docx(&bytes).unwrap();
    let p = pictures(&d);
    assert_eq!(p.len(), 1, "the linked picture is left out: {:?}", d.blocks);
    assert_eq!(
        (p[0].alt.as_str(), p[0].width_pt, p[0].bytes.as_slice()),
        ("A red square", Some(100), pic.as_slice())
    );
    // In reading order, between the two paragraphs.
    assert!(matches!(&d.blocks[1], Block::Image(_)), "{:?}", d.blocks);
    let html = write::html(&d, "x");
    assert_eq!(imgs(&html), (1, 1));
    assert!(html.contains("alt=\"A red square\"") && html.contains("width:100pt"));
}

#[test]
fn odt_rtf_and_html_pictures() {
    let pic = png(20, 20);
    // ODT: a frame with the picture and its title.
    let content = br#"<office:document-content xmlns:office="o" xmlns:text="t" xmlns:draw="d" xmlns:svg="s" xmlns:xlink="x"><office:body><office:text>
<text:p>Map<draw:frame svg:width="5.08cm" svg:height="2cm"><draw:image xlink:href="Pictures/map.png"/><svg:title>The map</svg:title></draw:frame></text:p>
</office:text></office:body></office:document-content>"#;
    let odt = zip_of(&[
        ("mimetype", b"application/vnd.oasis.opendocument.text"),
        ("content.xml", content),
        ("Pictures/map.png", &pic),
    ]);
    let d = read::odt(&odt).unwrap();
    let p = pictures(&d);
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].alt.as_str(), p[0].width_pt), ("The map", Some(144)));

    // RTF: Word's picture (shppict) is read once; its fallback copy isn't.
    let hex: String = pic.iter().map(|b| format!("{b:02x}")).collect();
    let rtf = format!(
        "{{\\rtf1\\ansi Hello {{\\*\\shppict{{\\pict\\pngblip\\picwgoal1440\\pichgoal1440 {hex}}}}}{{\\nonshppict{{\\pict\\wmetafile8 0102}}}} world}}"
    );
    let d = read::rtf(rtf.as_bytes());
    let p = pictures(&d);
    assert_eq!(p.len(), 1, "{:?}", d.blocks);
    assert_eq!(
        (p[0].width_pt, p[0].bytes.as_slice()),
        (Some(72), pic.as_slice())
    );
    assert!(texts(&d).contains("Hello") && texts(&d).contains("world"));

    // HTML: an embedded picture is kept; one that names a file next to the page isn't
    // loaded, even when that file exists.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("secret.png"), &pic).unwrap();
    let b64 = crate::emf::base64(&pic);
    let page = dir.path().join("page.html");
    std::fs::write(
        &page,
        format!(
            "<p>One</p><img src=\"data:image/png;base64,{b64}\" alt=\"Inline\"><img src=\"secret.png\" alt=\"Linked\"><p>Two</p>"
        ),
    )
    .unwrap();
    let d = read_path(&page, "html").unwrap();
    let p = pictures(&d);
    assert_eq!(p.len(), 1);
    assert_eq!(
        (p[0].alt.as_str(), p[0].bytes.as_slice()),
        ("Inline", pic.as_slice())
    );
    assert!(texts(&d).contains("[Linked]"), "{}", texts(&d));
}

#[test]
fn epub_keeps_pictures_both_ways() {
    let pic = png(30, 10);
    let mut d = read::markdown("# Trip\n\nDay one.\n");
    let shared = std::sync::Arc::new(pic.clone());
    for alt in ["Beach", "Beach again"] {
        d.blocks.push(Block::Image(Picture {
            bytes: shared.clone(),
            alt: alt.into(),
            width_pt: Some(200),
        }));
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("trip.epub");
    write::epub(&d, "trip", &out).unwrap();
    // The same picture is stored once, listed in the manifest, and read back twice.
    let bytes = std::fs::read(&out).unwrap();
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
    let names: Vec<String> = (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    let images: Vec<&String> = names.iter().filter(|n| n.contains("images/")).collect();
    assert_eq!(images, ["OEBPS/images/image-001.png"]);
    let mut opf = String::new();
    std::io::Read::read_to_string(&mut z.by_name("OEBPS/content.opf").unwrap(), &mut opf).unwrap();
    assert!(opf.contains("href=\"images/image-001.png\" media-type=\"image/png\""));
    let back = read::epub(&bytes).unwrap();
    let p = pictures(&back);
    assert_eq!(p.len(), 2);
    assert_eq!(
        (p[1].alt.as_str(), p[1].bytes.as_slice()),
        ("Beach again", pic.as_slice())
    );
    // And the e-book's pictures land in a web page.
    assert_eq!(imgs(&write::html(&back, "t")), (2, 2));
}

#[test]
fn pictures_browsers_cant_show_are_converted() {
    let bmp = {
        let img = image::RgbImage::from_pixel(4, 4, image::Rgb([0, 0, 255]));
        let mut v = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Bmp)
            .unwrap();
        v
    };
    let p = Picture {
        bytes: std::sync::Arc::new(bmp),
        alt: String::new(),
        width_pt: None,
    };
    let (mime, ext, bytes) = write::web_picture(&p).unwrap();
    assert_eq!((mime, ext), ("image/png", "png"));
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::Png
    );
    // Not a picture at all: left out rather than written as a broken image.
    let junk = Picture {
        bytes: std::sync::Arc::new(b"not a picture".to_vec()),
        alt: String::new(),
        width_pt: None,
    };
    assert!(write::web_picture(&junk).is_none());
    // The model's debug output names the size, not every byte.
    assert!(format!("{p:?}").contains("bytes #"));
}

/// A PDF with text and a picture → a web page with that picture (through Word → PDF, so
/// it needs system fonts).
#[test]
fn pdf_pictures_reach_the_web_page() {
    if !crate::docxlayout::fonts::Fonts::available() {
        return;
    }
    let doc = br#"<w:document xmlns:w="w" xmlns:wp="wp" xmlns:a="a" xmlns:r="r" xmlns:pic="pic"><w:body>
<w:p><w:r><w:t>Quarterly report with a chart below.</w:t></w:r></w:p>
<w:p><w:r><w:drawing><wp:inline><wp:extent cx="2540000" cy="1905000"/><wp:docPr id="1" name="Chart"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="rId5"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>
<w:p><w:r><w:t>Closing words.</w:t></w:r></w:p>
<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>
</w:body></w:document>"#;
    let rels = br#"<Relationships><Relationship Id="rId5" Type="image" Target="media/image1.png"/></Relationships>"#;
    let ct = br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="png" ContentType="image/png"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let pic = png(80, 60);
    let docx = zip_of(&[
        ("[Content_Types].xml", ct),
        ("word/document.xml", doc),
        ("word/_rels/document.xml.rels", rels),
        ("word/media/image1.png", &pic),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let pdf = dir.path().join("r.pdf");
    crate::docxlayout::convert(&docx, "r", &pdf, &swirvo_core::CancelToken::new()).unwrap();
    let d = read_path(&pdf, "pdf").unwrap();
    let p = pictures(&d);
    assert_eq!(p.len(), 1, "{:?}", d.blocks);
    assert_eq!(p[0].width_pt, Some(200));
    let html = write::html(&d, "r");
    assert_eq!(imgs(&html), (1, 1));
    let (i_text, i_img) = (
        html.find("Quarterly report").unwrap(),
        html.find("<img ").unwrap(),
    );
    assert!(i_text < i_img && i_img < html.find("Closing words").unwrap());
}

/// A web page with an embedded picture, as the doc model sees it.
fn page_with_picture(pic: &[u8]) -> Doc {
    let b64 = crate::emf::base64(pic);
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("page.html");
    std::fs::write(
        &page,
        format!(
            "<h1>Trip</h1><p>Before</p><img src=\"data:image/png;base64,{b64}\" alt=\"The beach\" style=\"width:150pt\"><p>After</p>"
        ),
    )
    .unwrap();
    read_path(&page, "html").unwrap()
}

fn zip_names(path: &std::path::Path) -> (Vec<String>, zip::ZipArchive<std::fs::File>) {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let names = (0..z.len())
        .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    (names, z)
}

fn zip_text(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> String {
    let mut s = String::new();
    std::io::Read::read_to_string(&mut z.by_name(name).unwrap(), &mut s).unwrap();
    s
}

#[test]
fn web_page_pictures_reach_word() {
    let pic = png(40, 30);
    let d = page_with_picture(&pic);
    assert_eq!(pictures(&d).len(), 1, "{:?}", d.blocks);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("trip.docx");
    write_path(&d, "docx", &out).unwrap();
    let (names, mut z) = zip_names(&out);
    assert!(
        names.contains(&"word/media/image1.png".to_string()),
        "{names:?}"
    );
    let doc = zip_text(&mut z, "word/document.xml");
    assert!(doc.contains("<w:drawing>") && doc.contains("r:embed=\"rIdP0\""));
    assert!(doc.contains("descr=\"The beach\""));
    // 150 pt wide, 4:3 → 112.5 pt high, in EMU.
    assert!(doc.contains("cx=\"1905000\" cy=\"1428750\""), "{doc}");
    let rels = zip_text(&mut z, "word/_rels/document.xml.rels");
    assert!(rels.contains("Id=\"rIdP0\"") && rels.contains("Target=\"media/image1.png\""));
    let types = zip_text(&mut z, "[Content_Types].xml");
    assert!(types.contains("Extension=\"png\" ContentType=\"image/png\""));
    // Our own reader finds it again, in its place.
    let back = read_path(&out, "docx").unwrap();
    let p = pictures(&back);
    assert_eq!(p.len(), 1, "{:?}", back.blocks);
    assert_eq!(
        (p[0].alt.as_str(), p[0].width_pt, p[0].bytes.as_slice()),
        ("The beach", Some(150), pic.as_slice())
    );
    let t = texts(&back);
    assert!(t.find("Before").unwrap() < t.find("After").unwrap());
}

#[test]
fn web_page_pictures_reach_odt() {
    let pic = png(40, 30);
    let d = page_with_picture(&pic);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("trip.odt");
    write_path(&d, "odt", &out).unwrap();
    let (names, mut z) = zip_names(&out);
    assert!(
        names.contains(&"Pictures/image1.png".to_string()),
        "{names:?}"
    );
    let manifest = zip_text(&mut z, "META-INF/manifest.xml");
    assert!(
        manifest.contains("full-path=\"Pictures/image1.png\" manifest:media-type=\"image/png\"")
    );
    let content = zip_text(&mut z, "content.xml");
    assert!(
        content.contains("svg:width=\"150.00pt\" svg:height=\"112.50pt\""),
        "{content}"
    );
    let back = read_path(&out, "odt").unwrap();
    let p = pictures(&back);
    assert_eq!(p.len(), 1, "{:?}", back.blocks);
    assert_eq!(
        (p[0].alt.as_str(), p[0].width_pt, p[0].bytes.as_slice()),
        ("The beach", Some(150), pic.as_slice())
    );
}

#[test]
fn web_page_pictures_reach_the_typeset_pdf() {
    let pic = png(40, 30);
    let mut d = page_with_picture(&pic);
    // The same picture again, much too wide: drawn once more, embedded once, fitted to
    // the column.
    let shared = pictures(&d)[0].bytes.clone();
    d.blocks.push(Block::Image(Picture {
        bytes: shared,
        alt: String::new(),
        width_pt: Some(2000),
    }));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("trip.pdf");
    if pdf::write(&d, "trip", &out).is_err() {
        // No system font to typeset with on this machine.
        return;
    }
    let doc = lopdf::Document::load(&out).unwrap();
    let images = doc
        .objects
        .values()
        .filter(|o| {
            o.as_stream().is_ok_and(|s| {
                s.dict.get(b"Subtype").and_then(|v| v.as_name()).ok() == Some(b"Image")
            })
        })
        .count();
    assert_eq!(images, 1, "one image XObject for the shared picture");
    let page = doc.get_pages().into_values().next().unwrap();
    let content = String::from_utf8_lossy(&doc.get_page_content(page).unwrap()).into_owned();
    assert_eq!(content.matches(" Do").count(), 2, "{content}");
}

#[test]
fn pictures_office_cant_read_are_converted_and_fitted() {
    // Word, LibreOffice and our PDF writer can't take BMP, TIFF or WebP as they are: the
    // writers convert them to PNG (EMF and WMF go into Word and ODT files as they are).
    let bmp = {
        let img = image::RgbImage::from_pixel(8, 4, image::Rgb([0, 0, 255]));
        let mut v = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Bmp)
            .unwrap();
        v
    };
    let p = Picture {
        bytes: std::sync::Arc::new(bmp),
        alt: String::new(),
        width_pt: None,
    };
    let placed = picture::place(&p, 464.0, 690.0, true).unwrap();
    assert_eq!((placed.mime, placed.w, placed.h), ("image/png", 6.0, 3.0));
    // Fitted into the box, keeping its shape.
    let wide = Picture {
        width_pt: Some(1000),
        ..p.clone()
    };
    let placed = picture::place(&wide, 400.0, 690.0, true).unwrap();
    assert_eq!((placed.w, placed.h), (400.0, 200.0));
    let junk = Picture {
        bytes: std::sync::Arc::new(b"not a picture".to_vec()),
        ..p
    };
    assert!(picture::place(&junk, 464.0, 690.0, true).is_none());
}

// ---------------------------------------------------------------- pictures in cells, Markdown

/// The pictures in a document's tables: (row, col, alt).
fn cell_pics(d: &Doc) -> Vec<(usize, usize, String)> {
    d.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table { pictures, .. } => Some(pictures),
            _ => None,
        })
        .flatten()
        .map(|p| (p.row, p.col, p.picture.alt.clone()))
        .collect()
}

/// A web page with a table whose second row has a picture in its first cell.
fn table_with_picture(pic: &[u8]) -> Doc {
    let b64 = crate::emf::base64(pic);
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("t.html");
    std::fs::write(
        &page,
        format!(
            "<table><tr><th>Item</th><th>Price</th></tr><tr><td>Hat <img src=\"data:image/png;base64,{b64}\" alt=\"Red hat\"></td><td>12</td></tr></table>"
        ),
    )
    .unwrap();
    read_path(&page, "html").unwrap()
}

#[test]
fn pictures_in_table_cells_go_everywhere() {
    let pic = png(400, 300);
    let d = table_with_picture(&pic);
    assert_eq!(
        cell_pics(&d),
        [(1, 0, "Red hat".to_string())],
        "{:?}",
        d.blocks
    );
    assert!(pictures(&d).is_empty(), "it stays in its cell");

    // HTML: inside the cell, after its text.
    let html = write::html(&d, "t");
    let cell = &html[html.find("<td>Hat").unwrap()..];
    assert!(cell[..cell.find("</td>").unwrap()].contains("<img src=\"data:image/png"));

    let dir = tempfile::tempdir().unwrap();
    for to in ["docx", "odt", "epub", "md"] {
        let out = dir.path().join(format!("t.{to}"));
        write_path(&d, to, &out).unwrap();
        let back = read_path(&out, to).unwrap();
        assert_eq!(
            cell_pics(&back),
            [(1, 0, "Red hat".to_string())],
            "{to}: {:?}",
            back.blocks
        );
        let shown = back
            .blocks
            .iter()
            .find_map(|b| match b {
                Block::Table { pictures, .. } => pictures.first().map(|p| p.picture.clone()),
                _ => None,
            })
            .unwrap();
        // Word and ODT keep the very bytes; Markdown and EPUB too (PNG stays PNG).
        assert_eq!(shown.bytes.as_slice(), pic.as_slice(), "{to}");
    }
    // Word: the drawing is inside the cell, fitted to its width (2 columns of 9000
    // twips: 4500 less margins = 214.2 pt).
    let (_, mut z) = zip_names(&dir.path().join("t.docx"));
    let doc = zip_text(&mut z, "word/document.xml");
    let tc = doc.split("<w:tc>").nth(3).unwrap();
    assert!(tc.contains("<w:drawing>"), "{doc}");
    let cx: f32 = tc
        .split("cx=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!((cx / 12_700.0 - 214.2).abs() < 0.5, "{cx}");

    // Typeset PDF: drawn once, inside the table.
    let out = dir.path().join("t.pdf");
    if pdf::write(&d, "t", &out).is_ok() {
        let doc = lopdf::Document::load(&out).unwrap();
        let page = doc.get_pages().into_values().next().unwrap();
        let content = String::from_utf8_lossy(&doc.get_page_content(page).unwrap()).into_owned();
        assert_eq!(content.matches(" Do").count(), 1, "{content}");
    }
}

/// A PNG that doesn't compress: `w` × `h` pixels of noise.
fn noisy_png(w: u32, h: u32) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    let img = image::RgbImage::from_fn(w, h, |_, _| {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        image::Rgb([x as u8, (x >> 8) as u8, (x >> 16) as u8])
    });
    let mut v = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut v), image::ImageFormat::Png)
        .unwrap();
    v
}

#[test]
fn markdown_keeps_small_pictures_inside_and_notes_big_ones() {
    let small = png(40, 30);
    let big = noisy_png(400, 400);
    assert!(big.len() > write::MD_PICTURE);
    let pic = |bytes: &[u8], alt: &str| {
        Block::Image(Picture {
            bytes: std::sync::Arc::new(bytes.to_vec()),
            alt: alt.into(),
            width_pt: None,
        })
    };
    let d = Doc {
        title: None,
        blocks: vec![pic(&small, "Logo [v2]"), pic(&big, "Photo")],
    };
    let md = write::markdown(&d);
    assert!(
        md.contains("![Logo \\[v2\\]](data:image/png;base64,"),
        "{md}"
    );
    assert!(
        md.contains("picture left out: too large for Markdown: Photo"),
        "{md}"
    );
    let back = read::markdown(&md);
    let p = pictures(&back);
    assert_eq!(p.len(), 1);
    assert_eq!(
        (p[0].alt.as_str(), p[0].bytes.as_slice()),
        ("Logo [v2]", small.as_slice())
    );
}

#[test]
fn markdown_reads_pictures_only_from_its_own_folder() {
    let dir = tempfile::tempdir().unwrap();
    let pic = png(20, 20);
    std::fs::create_dir(dir.path().join("notes files")).unwrap();
    std::fs::create_dir(dir.path().join("elsewhere")).unwrap();
    std::fs::write(dir.path().join("notes files/a.png"), &pic).unwrap();
    std::fs::write(dir.path().join("elsewhere/b.png"), &pic).unwrap();
    std::fs::write(dir.path().join("c.png"), &pic).unwrap();
    let md = dir.path().join("notes.md");
    std::fs::write(
        &md,
        "![Kept](notes%20files/a.png)\n\n![Other folder](elsewhere/b.png)\n\n![Next to it](c.png)\n\n![Escape](notes%20files/../c.png)\n\n![Web](https://example.com/x.png)\n\n![Absolute](/etc/passwd)\n",
    )
    .unwrap();
    let d = read_path(&md, "md").unwrap();
    let p = pictures(&d);
    assert_eq!(p.len(), 1, "{:?}", d.blocks);
    assert_eq!(
        (p[0].alt.as_str(), p[0].bytes.as_slice()),
        ("Kept", pic.as_slice())
    );
    // The others keep their words.
    let t = texts(&d);
    for w in ["Other folder", "Next to it", "Escape", "Web", "Absolute"] {
        assert!(t.contains(w), "{w}: {t}");
    }
}
