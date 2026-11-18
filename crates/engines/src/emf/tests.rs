use std::io::Write as _;

use swirvo_core::{CancelToken, Engine, Preset, StepRequest};

use super::*;
use crate::pdf_tools::PageRenderer;

fn le(v: &[i32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn le16(v: &[i16]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// COLORREF bytes: red, green, blue, 0.
fn cref(r: u8, g: u8, b: u8) -> Vec<u8> {
    vec![r, g, b, 0]
}

/// An EMF built record by record. The default device is 1000 × 500 px over
/// 100 × 50 mm, and the frame covers it all: a 283 × 142 pt picture.
#[derive(Default)]
struct Emf {
    recs: Vec<u8>,
    n: u32,
}

impl Emf {
    fn rec(&mut self, ty: u32, body: &[u8]) -> &mut Self {
        let mut body = body.to_vec();
        while !body.len().is_multiple_of(4) {
            body.push(0);
        }
        self.recs.extend(ty.to_le_bytes());
        self.recs.extend(((body.len() + 8) as u32).to_le_bytes());
        self.recs.extend(body);
        self.n += 1;
        self
    }

    fn brush(&mut self, ih: i32, c: (u8, u8, u8)) -> &mut Self {
        let mut b = le(&[ih, 0]);
        b.extend(cref(c.0, c.1, c.2));
        b.extend(le(&[0]));
        self.rec(39, &b).rec(37, &le(&[ih]))
    }

    fn no_pen(&mut self) -> &mut Self {
        self.rec(37, &0x8000_0008u32.to_le_bytes())
    }

    fn rect(&mut self, ty: u32, r: [i32; 4]) -> &mut Self {
        self.rec(ty, &le(&r))
    }

    fn finish(&mut self) -> Vec<u8> {
        self.rec(14, &le(&[0, 0, 16]));
        let mut h = Vec::new();
        h.extend(1u32.to_le_bytes());
        h.extend(88u32.to_le_bytes());
        h.extend(le(&[0, 0, 999, 499])); // bounds (device px)
        h.extend(le(&[0, 0, 10_000, 5_000])); // frame (0.01 mm)
        h.extend(0x464D_4520u32.to_le_bytes());
        h.extend(0x0001_0000u32.to_le_bytes());
        h.extend(((88 + self.recs.len()) as u32).to_le_bytes());
        h.extend((self.n + 1).to_le_bytes());
        h.extend(16u16.to_le_bytes());
        h.extend(0u16.to_le_bytes());
        h.extend(le(&[0, 0, 0])); // description, palette
        h.extend(le(&[1000, 500])); // device px
        h.extend(le(&[100, 50])); // millimetres
        h.extend(&self.recs);
        h
    }
}

/// A WMF built record by record (parameters as the file stores them).
#[derive(Default)]
struct Wmf {
    recs: Vec<u8>,
    max: u32,
}

impl Wmf {
    fn rec(&mut self, func: u16, params: &[u8]) -> &mut Self {
        let mut p = params.to_vec();
        if !p.len().is_multiple_of(2) {
            p.push(0);
        }
        let words = (6 + p.len()) as u32 / 2;
        self.max = self.max.max(words);
        self.recs.extend(words.to_le_bytes());
        self.recs.extend(func.to_le_bytes());
        self.recs.extend(p);
        self
    }

    /// With a placeable header for `bbox` at `inch` units per inch, or bare.
    fn finish(&mut self, placeable: Option<([i16; 4], u16)>) -> Vec<u8> {
        self.rec(0, &[]);
        let mut out = Vec::new();
        if let Some((bb, inch)) = placeable {
            out.extend(0x9AC6_CDD7u32.to_le_bytes());
            out.extend(0u16.to_le_bytes());
            out.extend(le16(&bb));
            out.extend(inch.to_le_bytes());
            out.extend(0u32.to_le_bytes());
            out.extend(0u16.to_le_bytes());
        }
        out.extend(1u16.to_le_bytes());
        out.extend(9u16.to_le_bytes());
        out.extend(0x0300u16.to_le_bytes());
        out.extend(((18 + self.recs.len()) as u32 / 2).to_le_bytes());
        out.extend(4u16.to_le_bytes()); // objects
        out.extend(self.max.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(&self.recs);
        out
    }
}

fn shapes_emf() -> Vec<u8> {
    let mut e = Emf::default();
    e.no_pen()
        .brush(1, (220, 20, 20))
        .rect(43, [0, 0, 500, 500])
        .brush(2, (20, 20, 220))
        .rect(42, [600, 100, 900, 400]);
    e.finish()
}

fn convert(bytes: &[u8], to: &str) -> Vec<u8> {
    let d = tempfile::tempdir().unwrap();
    let input = d.path().join("in.emf");
    let output = d.path().join(format!("out.{to}"));
    std::fs::write(&input, bytes).unwrap();
    let cancel = CancelToken::new();
    MetafileEngine
        .run(&StepRequest {
            input: &input,
            output: &output,
            from: "emf",
            to,
            preset: Preset::default(),
            pdf: Default::default(),
            cancel: &cancel,
        })
        .unwrap();
    std::fs::read(&output).unwrap()
}

/// The PDF's first page as pixels.
fn pdf_pixels(pdf: &[u8]) -> image::RgbaImage {
    let r = PageRenderer::from_bytes(pdf.to_vec()).unwrap();
    r.session(|s| s.render_fit(0, 400, 400).unwrap())
}

fn at(img: &image::RgbaImage, fx: f32, fy: f32) -> [u8; 3] {
    let p = img.get_pixel(
        ((img.width() as f32 * fx) as u32).min(img.width() - 1),
        ((img.height() as f32 * fy) as u32).min(img.height() - 1),
    );
    [p[0], p[1], p[2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 40)
}

/// Bounding box of every fill's path.
fn fill_box(pic: &Picture, k: usize) -> [f32; 4] {
    let fills: Vec<&Arc<Vec<PathCmd>>> = pic
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Fill { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for c in fills[k].iter() {
        let pts: Vec<(f32, f32)> = match *c {
            PathCmd::Move(x, y) | PathCmd::Line(x, y) => vec![(x, y)],
            PathCmd::Cubic(_, _, _, _, x, y) => vec![(x, y)],
            _ => vec![],
        };
        for (x, y) in pts {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
    }
    b
}

fn close(a: [f32; 4], b: [f32; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.5)
}

#[test]
fn detects_both_kinds() {
    assert!(is_metafile(&shapes_emf()));
    let w = Wmf::default().rec(0x001E, &[]).finish(None);
    assert!(is_metafile(&w));
    assert!(drawable(&w));
    assert!(!is_metafile(b"%PDF-1.7"));
}

/// The frame sets the size; device pixels become points; brushes fill and the null pen
/// draws nothing.
#[test]
fn plays_back_shapes_at_the_frame_size() {
    let pic = parse(&shapes_emf()).unwrap();
    assert!((pic.w - 283.46).abs() < 0.1 && (pic.h - 141.73).abs() < 0.1);
    assert!(close(fill_box(&pic, 0), [0.0, 0.0, 141.73, 141.73]));
    assert!(!pic.items.iter().any(|i| matches!(i, Item::Stroke { .. })));
    let img = pdf_pixels(&convert(&shapes_emf(), "pdf"));
    assert!((img.width() as f32 / img.height() as f32 - 2.0).abs() < 0.02);
    assert!(
        near(at(&img, 0.25, 0.5), [220, 20, 20]),
        "{:?}",
        at(&img, 0.25, 0.5)
    );
    assert!(
        near(at(&img, 0.75, 0.5), [20, 20, 220]),
        "{:?}",
        at(&img, 0.75, 0.5)
    );
    // Outside the ellipse's box, and in its corner: background.
    assert!(near(at(&img, 0.97, 0.5), [255, 255, 255]));
    assert!(near(at(&img, 0.61, 0.22), [255, 255, 255]));
}

/// Window → viewport scaling (anisotropic) and the world transform both apply.
#[test]
fn mapping_modes_and_world_transforms() {
    let mut e = Emf::default();
    e.rec(17, &le(&[8]))
        .rec(9, &le(&[100, 100]))
        .rec(11, &le(&[1000, 500]))
        .no_pen()
        .brush(1, (0, 0, 0))
        .rect(43, [0, 0, 50, 100]);
    let pic = parse(&e.finish()).unwrap();
    assert!(close(fill_box(&pic, 0), [0.0, 0.0, 141.73, 141.73]));

    let mut e = Emf::default();
    let two: Vec<u8> = [2.0f32, 0.0, 0.0, 2.0, 100.0, 0.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    e.rec(35, &two)
        .no_pen()
        .brush(1, (0, 0, 0))
        .rect(43, [0, 0, 100, 50]);
    let pic = parse(&e.finish()).unwrap();
    // Device x 100..300, y 0..100 → points ×0.2835.
    assert!(close(fill_box(&pic, 0), [28.35, 0.0, 85.04, 28.35]));

    // Isotropic keeps the aspect; the y-up LOMETRIC mode flips.
    let mut e = Emf::default();
    e.rec(17, &le(&[2])) // 0.1 mm, y up
        .rec(12, &le(&[0, 500]))
        .no_pen()
        .brush(1, (0, 0, 0))
        .rect(43, [0, 0, 100, 100]); // 10 mm square at the bottom-left
    let pic = parse(&e.finish()).unwrap();
    assert!(close(fill_box(&pic, 0), [0.0, 113.39, 28.35, 141.73]));
}

/// A 2 × 2 bottom-up DIB keeps its orientation, and every quadrant its colour.
#[test]
fn bitmaps_keep_their_orientation() {
    let mut bmi = le(&[40, 2, 2]);
    bmi.extend(1u16.to_le_bytes());
    bmi.extend(24u16.to_le_bytes());
    bmi.extend(le(&[0, 16, 0, 0, 0, 0]));
    // Rows bottom first, BGR, padded to 4 bytes.
    let bits = [
        255, 0, 0, 255, 255, 255, 0, 0, // bottom: blue, white
        0, 0, 255, 0, 255, 0, 0, 0, // top: red, green
    ];
    let mut r = le(&[0, 0, 999, 499]); // bounds
    r.extend(le(&[0, 0, 0, 0, 2, 2])); // dest x y, src x y w h
    r.extend(le(&[80, 40, 120, 16, 0, 0x00CC_0020, 1000, 500]));
    r.extend(&bmi);
    r.extend(bits);
    let mut e = Emf::default();
    e.rec(81, &r);
    let bytes = e.finish();
    let img = pdf_pixels(&convert(&bytes, "pdf"));
    for (fx, fy, c) in [
        (0.25, 0.25, [255, 0, 0]),
        (0.75, 0.25, [0, 255, 0]),
        (0.25, 0.75, [0, 0, 255]),
        (0.75, 0.75, [255, 255, 255]),
    ] {
        assert!(
            near(at(&img, fx, fy), c),
            "{fx},{fy}: {:?}",
            at(&img, fx, fy)
        );
    }
}

/// A path used as a clip, and the two polygon fill rules. Checked on the PNG (the same
/// playback): hayro's rasteriser (vello_common 0.3) underflows in debug builds on a clip
/// whose edge lies on the page edge, so the PDF can't be checked by rendering it here.
fn png_pixels(png: &[u8]) -> image::RgbaImage {
    let mut img = image::load_from_memory(png).unwrap().to_rgba8();
    // On white paper, like the PDF.
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        for c in 0..3 {
            p[c] = ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    img
}

#[test]
fn clip_paths_and_fill_rules() {
    let mut e = Emf::default();
    e.rec(59, &[])
        .rect(42, [0, 0, 1000, 500])
        .rec(60, &[])
        .rec(67, &le(&[5]))
        .no_pen()
        .brush(1, (220, 20, 20))
        .rect(43, [0, 0, 1000, 500]);
    let img = png_pixels(&convert(&e.finish(), "png"));
    assert!(near(at(&img, 0.5, 0.5), [220, 20, 20]));
    assert!(
        near(at(&img, 0.02, 0.04), [255, 255, 255]),
        "corner is clipped away"
    );

    let rings = |mode: i32| {
        let mut body = le(&[0, 0, 999, 499, 2, 8, 4, 4]);
        body.extend(le(&[100, 50, 900, 50, 900, 450, 100, 450]));
        body.extend(le(&[400, 150, 600, 150, 600, 350, 400, 350]));
        let mut e = Emf::default();
        e.rec(19, &le(&[mode]))
            .no_pen()
            .brush(1, (20, 20, 220))
            .rec(8, &body);
        png_pixels(&convert(&e.finish(), "png"))
    };
    let alternate = rings(1);
    assert!(
        near(at(&alternate, 0.5, 0.5), [255, 255, 255]),
        "even-odd leaves a hole"
    );
    assert!(near(at(&alternate, 0.2, 0.5), [20, 20, 220]));
    let winding = rings(2);
    assert!(
        near(at(&winding, 0.5, 0.5), [20, 20, 220]),
        "non-zero fills it"
    );
}

/// A placeable WMF: the window maps onto the box, objects take the lowest free slot,
/// and parameters come last-first.
#[test]
fn placeable_wmf_with_text() {
    let mut w = Wmf::default();
    w.rec(0x020B, &le16(&[0, 0]))
        .rec(0x020C, &le16(&[100, 100])) // y, x
        .rec(
            0x02FC,
            &[&le16(&[0])[..], &cref(20, 160, 20), &le16(&[0])].concat(),
        )
        .rec(0x012D, &le16(&[0]))
        .rec(0x02FA, &[&le16(&[5, 0, 0])[..], &cref(0, 0, 0)].concat())
        .rec(0x012D, &le16(&[1]))
        .rec(0x0324, &le16(&[3, 0, 100, 50, 0, 100, 100]));
    let mut font = le16(&[-20, 0, 0, 0, 700]);
    font.extend([0u8, 0, 0, 0, 0, 0, 0, 0]);
    font.extend(b"Arial\0");
    w.rec(0x02FB, &font)
        .rec(0x012D, &le16(&[2]))
        .rec(0x0521, &[&le16(&[2])[..], b"Hi", &le16(&[90, 10])].concat());
    let bytes = w.finish(Some(([0, 0, 1000, 1000], 1000)));
    let pic = parse(&bytes).unwrap();
    assert!((pic.w - 72.0).abs() < 0.01 && (pic.h - 72.0).abs() < 0.01);
    let text: Vec<&TextItem> = pic
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Text(t) => Some(&**t),
            _ => None,
        })
        .collect();
    assert_eq!(text.len(), 1);
    assert_eq!((text[0].text.as_str(), text[0].bold), ("Hi", true));
    assert!((text[0].m[4] - 7.2).abs() < 0.01 && (text[0].m[5] - 64.8).abs() < 0.01);
    let png = convert(&bytes, "png");
    let img = image::load_from_memory(&png).unwrap().to_rgba8();
    assert_eq!(img.width(), 144);
    let p = img.get_pixel(72, 110);
    assert!(
        near([p[0], p[1], p[2]], [20, 160, 20]) && p[3] > 200,
        "{p:?}"
    );
    assert_eq!(
        img.get_pixel(4, 4)[3],
        0,
        "outside the triangle stays transparent"
    );

    // Without the placeable header the first window gives the box.
    let bare = {
        let mut b = bytes[22..].to_vec();
        b.truncate(b.len());
        b
    };
    let pic = parse(&bare).unwrap();
    assert!(pic.w > 0.0 && pic.items.len() >= 2);
}

#[test]
fn text_is_shaped_with_the_metafile_advances() {
    let mut e = Emf::default();
    let mut f = le(&[3, -40, 0, 0, 0, 400]);
    f.extend([0u8, 0, 0, 0, 0, 0, 0, 0]);
    let name: Vec<u8> = "Arial"
        .encode_utf16()
        .chain(std::iter::repeat(0))
        .take(32)
        .flat_map(|c| c.to_le_bytes())
        .collect();
    f.extend(name);
    e.rec(82, &f).rec(37, &le(&[3]));
    e.rec(24, &cref(20, 20, 220)).rec(18, &le(&[1]));
    // EXTTEXTOUTW "Hello" at (100, 100) with 50-unit advances.
    let text: Vec<u8> = "Hello"
        .encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    let mut r = le(&[0, 0, 999, 499, 1]);
    r.extend(1.0f32.to_le_bytes());
    r.extend(1.0f32.to_le_bytes());
    r.extend(le(&[100, 100, 5, 76, 0, 0, 0, 0, 0, 88]));
    r.extend(&text);
    r.extend([0, 0]);
    r.extend(le(&[50, 50, 50, 50, 50]));
    e.rec(84, &r);
    let bytes = e.finish();
    let pic = parse(&bytes).unwrap();
    let Some(Item::Text(t)) = pic.items.iter().find(|i| matches!(i, Item::Text(_))) else {
        panic!("no text");
    };
    assert_eq!((t.text.as_str(), t.dx.len(), t.cell), ("Hello", 5, false));
    if !crate::docxlayout::fonts::Fonts::available() {
        return;
    }
    let mut fonts = crate::docxlayout::fonts::Fonts::new(Default::default());
    let ops = ops::ops(&pic, &mut fonts);
    let width: f32 = ops
        .iter()
        .filter_map(|o| match o {
            Op::Text { seg, .. } => Some(seg.width),
            _ => None,
        })
        .sum();
    assert!(
        (width - 250.0).abs() < 0.5,
        "advances come from the file: {width}"
    );
    let pdf = convert(&bytes, "pdf");
    let doc = lopdf::Document::load_mem(&pdf).unwrap();
    assert!(doc.extract_text(&[1]).unwrap().contains("Hello"));
}

/// Hatched brushes, pens with styles and widths, arcs and pies all play back.
#[test]
fn hatches_pens_and_arcs() {
    let mut e = Emf::default();
    let mut hatch = le(&[1, 2]);
    hatch.extend(cref(0, 0, 0));
    hatch.extend(le(&[4])); // cross
    e.rec(39, &hatch).rec(37, &le(&[1]));
    let mut pen = le(&[2, 1]); // ih, PS_DASH
    pen.extend(le(&[10, 0]));
    pen.extend(cref(200, 0, 0));
    e.rec(38, &pen).rec(37, &le(&[2]));
    e.rect(43, [0, 0, 400, 400]);
    e.rec(47, &le(&[500, 0, 900, 400, 900, 200, 700, 0])); // pie
    e.rec(45, &le(&[500, 0, 900, 400, 700, 400, 500, 200])); // arc
    e.rec(44, &le(&[0, 420, 400, 499, 40, 40])); // round rect
    let pic = parse(&e.finish()).unwrap();
    let strokes: Vec<&Pen> = pic
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Stroke { pen, .. } => Some(pen),
            _ => None,
        })
        .collect();
    // Hatch lines (thin, solid) and the dashed red outline.
    assert!(
        strokes
            .iter()
            .any(|p| p.dash.is_empty() && p.color == Rgb(0, 0, 0))
    );
    let red = strokes.iter().find(|p| p.color == Rgb(200, 0, 0)).unwrap();
    assert_eq!(red.dash, vec![3.0, 1.0]);
    assert!(
        (red.width - 2.835).abs() < 0.01,
        "10 px = 2.83 pt: {}",
        red.width
    );
    assert!(pic.items.iter().any(|i| matches!(i, Item::Clip { .. })));
    let svg = svg::to_svg(&pic);
    assert!(svg.contains("stroke-dasharray") && svg.contains("clip-path"));
}

/// Office pictures: an EMF inside a Word document and an RTF `\emfblip` are drawn.
#[test]
fn documents_draw_their_metafiles() {
    if !crate::docxlayout::fonts::Fonts::available() {
        return;
    }
    let emf = shapes_emf();
    let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;
    // 4 × 2 inches.
    let document = format!(
        r#"<w:document {w}><w:body><w:p><w:r><w:drawing><wp:inline><wp:extent cx="3657600" cy="1828800"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:blipFill><a:blip r:embed="rIdEmf"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );
    let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdEmf" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.emf"/></Relationships>"#;
    let root = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let mut docx = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut docx));
        let o = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("_rels/.rels", root.as_bytes()),
            ("word/document.xml", document.as_bytes()),
            ("word/_rels/document.xml.rels", rels.as_bytes()),
            ("word/media/image1.emf", &emf),
        ] {
            z.start_file(name, o).unwrap();
            z.write_all(body).unwrap();
        }
        z.finish().unwrap();
    }
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("w.pdf");
    crate::docxlayout::convert(&docx, "w", &out, &CancelToken::new()).unwrap();
    let pdf = std::fs::read(&out).unwrap();
    let r = PageRenderer::from_bytes(pdf).unwrap();
    // The picture sits at the top-left margin (72 pt), 288 × 144 pt.
    let img = r.session(|s| s.render_dpi(0, 72.0).unwrap());
    let px = |x: u32, y: u32| {
        let p = img.get_pixel(x, y);
        [p[0], p[1], p[2]]
    };
    assert!(
        near(px(72 + 72, 72 + 72), [220, 20, 20]),
        "{:?}",
        px(144, 144)
    );
    assert!(
        near(px(72 + 216, 72 + 72), [20, 20, 220]),
        "{:?}",
        px(288, 144)
    );

    // RTF: the same picture as \emfblip hex.
    let hex: String = emf.iter().map(|b| format!("{b:02x}")).collect();
    let rtf = format!(r"{{\rtf1\ansi{{\pict\emfblip\picwgoal5760\pichgoal2880 {hex}}}\par}}");
    let out = d.path().join("r.pdf");
    crate::docxlayout::convert_rtf(rtf.as_bytes(), "r", &out, &CancelToken::new()).unwrap();
    let r = PageRenderer::open(&out).unwrap();
    let img = r.session(|s| s.render_dpi(0, 72.0).unwrap());
    let reds = img
        .pixels()
        .filter(|p| p[0] > 180 && p[1] < 80 && p[2] < 80)
        .count();
    assert!(reds > 5_000, "the RTF picture is drawn: {reds} red pixels");
}

/// Truncated and corrupted files never panic, and huge counts are refused cheaply.
#[test]
fn damaged_files_are_safe() {
    let mut seeds = vec![shapes_emf()];
    let mut w = Wmf::default();
    w.rec(0x0324, &le16(&[3, 0, 100, 50, 0, 100, 100])).rec(
        0x0538,
        &le16(&[2, 3, 3, 0, 0, 9, 9, 0, 9, 1, 1, 5, 5, 1, 5]),
    );
    seeds.push(w.finish(Some(([0, 0, 100, 100], 96))));
    let mut fonts = crate::docxlayout::fonts::Fonts::new(Default::default());
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut rand = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for seed in &seeds {
        for cut in (0..seed.len()).step_by(7) {
            let _ = parse(&seed[..cut]);
        }
        for _ in 0..400 {
            let mut b = seed.clone();
            for _ in 0..(rand() % 6 + 1) {
                let i = (rand() as usize) % b.len();
                b[i] = match rand() % 4 {
                    0 => 0xFF,
                    1 => 0,
                    2 => 0x7F,
                    _ => rand() as u8,
                };
            }
            // Straight calls, so a panic fails the test instead of being caught.
            let pic = if emr::is_emf(&b) {
                emr::parse(&b)
            } else {
                wmr::parse(&b)
            };
            if let Ok(pic) = pic {
                let _ = svg::to_svg(&pic);
                let _ = ops::ops(&pic, &mut fonts);
            }
        }
    }
    // A polyline claiming 2^31 points in a tiny record.
    let mut e = Emf::default();
    e.rec(4, &le(&[0, 0, 1, 1, i32::MAX, 0, 0]));
    let pic = parse(&e.finish()).unwrap();
    assert!(pic.items.is_empty());
}
