//! Laid-out pages → PDF (krilla): glyphs exactly where layout put them, vector rules
//! and shapes, pictures (cropped as in Word), clickable links, jumps to bookmarks, the
//! heading outline and document metadata.

use std::collections::HashMap;
use std::io::Cursor;
use std::ops::Range;
use std::path::Path;

use std::num::NonZeroU16;

use krilla::SerializeSettings;
use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::color::rgb;
use krilla::configure::{Configuration, ValidationError, Validator};
use krilla::destination::{Destination, XyzDestination};
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::outline::{Outline, OutlineNode};
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke, StrokeDash};
use krilla::surface::Surface;
use krilla::tagging::{
    ArtifactType, ContentTag, Identifier, ListNumbering, Node, SpanTag, TableHeaderScope, Tag,
    TagGroup, TagKind, TagTree,
};
use krilla::text::{Glyph, GlyphId};
use swirvo_core::{EngineError, PdfOptions};

use super::fonts::Fonts;
use super::layout::{Op, PageOut};
use super::model::{Geom, Link, Media, Meta};
use super::props::{BorderStyle, Rgb};
use super::tags::{ARTIFACT, FOOTER, HEADER, Numbering, Role, Tags};

struct G {
    id: u16,
    range: Range<usize>,
    adv: f32,
    dx: f32,
    dy: f32,
}

impl Glyph for G {
    fn glyph_id(&self) -> GlyphId {
        GlyphId::new(self.id as u32)
    }
    fn text_range(&self) -> Range<usize> {
        self.range.clone()
    }
    // Stored per em; krilla asks at the font size or at 1.0.
    fn x_advance(&self, size: f32) -> f32 {
        self.adv * size
    }
    fn x_offset(&self, size: f32) -> f32 {
        self.dx * size
    }
    fn y_offset(&self, size: f32) -> f32 {
        self.dy * size
    }
    fn y_advance(&self, _size: f32) -> f32 {
        0.0
    }
    fn location(&self) -> Option<krilla::surface::Location> {
        None
    }
}

fn paint(c: Rgb) -> krilla::paint::Paint {
    rgb::Color::new(c.0, c.1, c.2).into()
}

fn fill(c: Rgb) -> Fill {
    Fill {
        paint: paint(c),
        ..Default::default()
    }
}

fn image_of(m: &Media, interpolate: bool) -> Option<krilla::image::Image> {
    let bytes = m.bytes.as_slice();
    let fmt = image::guess_format(bytes).ok()?;
    match fmt {
        image::ImageFormat::Png => {
            krilla::image::Image::from_png(m.bytes.to_vec().into(), interpolate).ok()
        }
        image::ImageFormat::Jpeg => {
            krilla::image::Image::from_jpeg(m.bytes.to_vec().into(), interpolate).ok()
        }
        image::ImageFormat::Gif => {
            krilla::image::Image::from_gif(m.bytes.to_vec().into(), interpolate).ok()
        }
        _ => {
            // BMP, TIFF, WebP…: decode and embed as PNG.
            let img = image::load_from_memory(bytes).ok()?;
            let mut png = Vec::new();
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .ok()?;
            krilla::image::Image::from_png(png.into(), interpolate).ok()
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn stroke_line(
    s: &mut Surface<'_>,
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    w: f32,
    c: Rgb,
    style: BorderStyle,
) {
    let mut draw = |dx: f32, dy: f32, width: f32| {
        let mut pb = PathBuilder::new();
        pb.move_to(x1 + dx, y1 + dy);
        pb.line_to(x2 + dx, y2 + dy);
        if let Some(p) = pb.finish() {
            s.set_fill(None);
            s.set_stroke(Some(Stroke {
                paint: paint(c),
                width,
                dash: match style {
                    BorderStyle::Dotted => Some(StrokeDash {
                        array: vec![width, width * 1.5],
                        offset: 0.0,
                    }),
                    BorderStyle::Dashed => Some(StrokeDash {
                        array: vec![width * 4.0, width * 2.0],
                        offset: 0.0,
                    }),
                    _ => None,
                },
                ..Default::default()
            }));
            s.draw_path(&p);
        }
    };
    if style == BorderStyle::Double {
        let t = (w / 3.0).max(0.25);
        let horizontal = (y1 - y2).abs() < (x1 - x2).abs();
        let (ox, oy) = if horizontal { (0.0, t) } else { (t, 0.0) };
        draw(-ox, -oy, t);
        draw(ox, oy, t);
    } else {
        draw(0.0, 0.0, w);
    }
    s.set_stroke(None);
}

fn norm(v: f32) -> krilla::num::NormalizedF32 {
    krilla::num::NormalizedF32::new(v.clamp(0.0, 1.0)).unwrap_or(krilla::num::NormalizedF32::ONE)
}

fn stops(v: &[(f32, Rgb, f32)]) -> Vec<krilla::paint::Stop> {
    v.iter()
        .map(|(o, c, a)| krilla::paint::Stop {
            offset: norm(*o),
            color: rgb::Color::new(c.0, c.1, c.2).into(),
            opacity: norm(*a),
        })
        .collect()
}

fn figure_paint(p: &super::layout::Paint, x: f32, y: f32) -> (krilla::paint::Paint, f32) {
    use super::layout::Paint as P;
    match p {
        P::Solid(c, a) => (paint(*c), *a),
        P::Linear {
            from,
            to,
            stops: st,
        } => (
            krilla::paint::LinearGradient {
                x1: x + from.0,
                y1: y + from.1,
                x2: x + to.0,
                y2: y + to.1,
                transform: Transform::identity(),
                spread_method: krilla::paint::SpreadMethod::Pad,
                stops: stops(st),
                anti_alias: false,
            }
            .into(),
            1.0,
        ),
        P::Radial {
            center,
            radius,
            stops: st,
        } => (
            krilla::paint::RadialGradient {
                fx: x + center.0,
                fy: y + center.1,
                fr: 0.0,
                cx: x + center.0,
                cy: y + center.1,
                cr: radius.max(0.1),
                transform: Transform::identity(),
                spread_method: krilla::paint::SpreadMethod::Pad,
                stops: stops(st),
                anti_alias: false,
            }
            .into(),
            1.0,
        ),
    }
}

fn fill_rule(even_odd: bool) -> krilla::paint::FillRule {
    if even_odd {
        krilla::paint::FillRule::EvenOdd
    } else {
        krilla::paint::FillRule::NonZero
    }
}

/// A figure's outline at its position.
fn figure_path(f: &super::layout::Figure) -> Option<krilla::geom::Path> {
    use super::layout::PathCmd as C;
    let mut pb = PathBuilder::new();
    let (x, y) = (f.x, f.y);
    for c in &f.path {
        match *c {
            C::Move(a, b) => pb.move_to(x + a, y + b),
            C::Line(a, b) => pb.line_to(x + a, y + b),
            C::Quad(a, b, c2, d) => pb.quad_to(x + a, y + b, x + c2, y + d),
            C::Cubic(a, b, c2, d, e, g) => pb.cubic_to(x + a, y + b, x + c2, y + d, x + e, y + g),
            C::Close => pb.close(),
        }
    }
    pb.finish()
}

fn draw_figure(s: &mut Surface<'_>, f: &super::layout::Figure) {
    let (x, y) = (f.x, f.y);
    let Some(path) = figure_path(f) else { return };
    s.set_fill(f.fill.as_ref().map(|p| {
        let (paint, opacity) = figure_paint(p, x, y);
        Fill {
            paint,
            opacity: norm(opacity),
            rule: fill_rule(f.even_odd),
        }
    }));
    s.set_stroke(f.stroke.as_ref().filter(|p| p.width > 0.0).map(|p| Stroke {
        paint: paint(p.color),
        width: p.width,
        opacity: norm(p.alpha),
        line_cap: if p.round {
            krilla::paint::LineCap::Round
        } else {
            krilla::paint::LineCap::Butt
        },
        line_join: if p.round {
            krilla::paint::LineJoin::Round
        } else {
            krilla::paint::LineJoin::Miter
        },
        dash: (!p.dash.is_empty()).then(|| StrokeDash {
            array: p.dash.iter().map(|d| d * p.width).collect(),
            offset: 0.0,
        }),
        ..Default::default()
    }));
    s.draw_path(&path);
    s.set_fill(None);
    s.set_stroke(None);
}

fn shape_path(geom: Geom, x: f32, y: f32, w: f32, h: f32) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    match geom {
        Geom::Line => {
            pb.move_to(x, y);
            pb.line_to(x + w, y + h);
        }
        Geom::Ellipse => {
            // Four cubic arcs.
            let (rx, ry) = (w / 2.0, h / 2.0);
            let (cx, cy) = (x + rx, y + ry);
            let k = 0.552_284_8;
            pb.move_to(cx + rx, cy);
            pb.cubic_to(cx + rx, cy + ry * k, cx + rx * k, cy + ry, cx, cy + ry);
            pb.cubic_to(cx - rx * k, cy + ry, cx - rx, cy + ry * k, cx - rx, cy);
            pb.cubic_to(cx - rx, cy - ry * k, cx - rx * k, cy - ry, cx, cy - ry);
            pb.cubic_to(cx + rx * k, cy - ry, cx + rx, cy - ry * k, cx + rx, cy);
            pb.close();
        }
        Geom::RoundRect => {
            let r = (w.min(h) * 0.1667).max(0.0);
            let k = 0.552_284_8 * r;
            pb.move_to(x + r, y);
            pb.line_to(x + w - r, y);
            pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
            pb.line_to(x + w, y + h - r);
            pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
            pb.line_to(x + r, y + h);
            pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
            pb.line_to(x, y + r);
            pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
            pb.close();
        }
        Geom::Rect => pb.push_rect(Rect::from_xywh(x, y, w.max(0.01), h.max(0.01))?),
    }
    pb.finish()
}

struct OutlineItem {
    level: u8,
    text: String,
    page: usize,
    y: f32,
    kids: Vec<OutlineItem>,
}

fn to_node(it: OutlineItem) -> OutlineNode {
    let mut n = OutlineNode::new(
        it.text,
        XyzDestination::new(it.page, Point::from_xy(0.0, it.y)),
    );
    for k in it.kids {
        n.push_child(to_node(k));
    }
    n
}

/// Pictures embedded once each, and metafiles played back once each.
#[derive(Default)]
struct Cache {
    images: HashMap<*const Media, Option<krilla::image::Image>>,
    metas: HashMap<*const Media, Option<MetaOps>>,
    /// PDF/A forbids asking viewers to smooth pictures.
    interpolate: bool,
}

/// A metafile's ops in its own coordinates, and its natural size.
type MetaOps = std::sync::Arc<(Vec<Op>, f32, f32)>;

/// Draw a picture's box: clip to it when cropped, then call `inner` with the transform
/// that maps the uncropped picture's (`pw` × `ph`) space onto the frame.
#[allow(clippy::too_many_arguments)]
fn in_frame(
    s: &mut Surface<'_>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    crop: [f32; 4],
    always_clip: bool,
    inner: impl FnOnce(&mut Surface<'_>, f32, f32),
) {
    let [l, t, r, b] = crop;
    let fw = w / (1.0 - l - r).max(0.01);
    let fh = h / (1.0 - t - b).max(0.01);
    let clip = always_clip || crop.iter().any(|c| c.abs() > 0.0005);
    if clip {
        let Some(rect) = Rect::from_xywh(x, y, w.max(0.01), h.max(0.01)) else {
            return;
        };
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        let Some(p) = pb.finish() else { return };
        s.push_clip_path(&p, &krilla::paint::FillRule::NonZero);
    }
    s.push_transform(&Transform::from_translate(x - l * fw, y - t * fh));
    inner(s, fw, fh);
    s.pop();
    if clip {
        s.pop();
    }
}

fn draw_ops(s: &mut Surface<'_>, ops: &[Op], fonts: &mut Fonts, cache: &mut Cache, depth: u8) {
    for op in ops {
        draw_op(s, op, fonts, cache, depth);
    }
}

/// Draw one op. `depth` is 1 inside a metafile, whose own pictures are bitmaps.
fn draw_op(s: &mut Surface<'_>, op: &Op, fonts: &mut Fonts, cache: &mut Cache, depth: u8) {
    match op {
        Op::Text { x, y, seg, color } => {
            let face = fonts.get(seg.face);
            let size = seg.size;
            let glyphs: Vec<G> = seg
                .glyphs
                .iter()
                .map(|g| G {
                    id: g.id,
                    range: g.start as usize..(g.end as usize).max(g.start as usize),
                    adv: g.adv / size,
                    dx: g.dx / size,
                    dy: g.dy / size,
                })
                .collect();
            if glyphs.is_empty() {
                return;
            }
            s.set_fill(Some(fill(*color)));
            if face.synth_bold {
                s.set_stroke(Some(Stroke {
                    paint: paint(*color),
                    width: size * 0.03,
                    ..Default::default()
                }));
            } else {
                s.set_stroke(None);
            }
            let skew = face.synth_italic;
            let hscale = seg.rs.scale;
            if skew || (hscale - 1.0).abs() > 0.01 {
                // Slant around the baseline; horizontal scale (w:w) from the start.
                let k = if skew { -0.21 } else { 0.0 };
                s.push_transform(&Transform::from_row(1.0, 0.0, k, 1.0, -k * y, 0.0));
                s.draw_glyphs(
                    Point::from_xy(*x, *y),
                    &glyphs,
                    face.pdf.clone(),
                    &seg.text,
                    size,
                    false,
                );
                s.pop();
            } else {
                s.draw_glyphs(
                    Point::from_xy(*x, *y),
                    &glyphs,
                    face.pdf.clone(),
                    &seg.text,
                    size,
                    false,
                );
            }
            s.set_stroke(None);
        }
        Op::Rect {
            x,
            y,
            w,
            h,
            fill: c,
        } => {
            if let Some(r) = Rect::from_xywh(*x, *y, w.max(0.01), h.max(0.01)) {
                let mut pb = PathBuilder::new();
                pb.push_rect(r);
                if let Some(p) = pb.finish() {
                    s.set_stroke(None);
                    s.set_fill(Some(fill(*c)));
                    s.draw_path(&p);
                }
            }
        }
        Op::Line {
            x1,
            y1,
            x2,
            y2,
            width,
            color,
            style,
        } => stroke_line(s, *x1, *y1, *x2, *y2, *width, *color, *style),
        Op::Image {
            media,
            x,
            y,
            w,
            h,
            crop,
        } => {
            let key = std::sync::Arc::as_ptr(media);
            // Metafiles are drawn as vectors (never nested: their own pictures are
            // bitmaps), clipped to the frame like Office does.
            if depth == 0 && crate::emf::is_metafile(&media.bytes) {
                let meta = cache
                    .metas
                    .entry(key)
                    .or_insert_with(|| crate::emf::layout_ops(&media.bytes, fonts))
                    .clone();
                let Some(meta) = meta else { return };
                let (inner, pw, ph) = (&meta.0, meta.1.max(0.01), meta.2.max(0.01));
                in_frame(s, *x, *y, *w, *h, *crop, true, |s, fw, fh| {
                    s.push_transform(&Transform::from_scale(fw / pw, fh / ph));
                    draw_ops(s, inner, fonts, cache, depth + 1);
                    s.pop();
                });
                return;
            }
            let interpolate = cache.interpolate;
            let img = cache
                .images
                .entry(key)
                .or_insert_with(|| image_of(media, interpolate))
                .clone();
            let Some(img) = img else { return };
            in_frame(s, *x, *y, *w, *h, *crop, false, |s, fw, fh| {
                if let Some(sz) = Size::from_wh(fw.max(0.01), fh.max(0.01)) {
                    s.draw_image(img, sz);
                }
            });
        }
        Op::Shape {
            geom,
            x,
            y,
            w,
            h,
            fill: f,
            line,
        } => {
            if let Some(p) = shape_path(*geom, *x, *y, *w, *h) {
                s.set_fill(f.filter(|_| *geom != Geom::Line).map(fill));
                s.set_stroke(line.map(|(c, width)| Stroke {
                    paint: paint(c),
                    width,
                    ..Default::default()
                }));
                s.draw_path(&p);
                s.set_stroke(None);
            }
        }
        Op::Figure(f) => draw_figure(s, f),
        Op::Push(m) => {
            s.push_transform(&Transform::from_row(m[0], m[1], m[2], m[3], m[4], m[5]));
        }
        Op::Clip(r) => {
            let mut pb = PathBuilder::new();
            if let Some(rect) = Rect::from_xywh(r[0], r[1], r[2].max(0.01), r[3].max(0.01)) {
                pb.push_rect(rect);
            }
            match pb.finish() {
                Some(p) => s.push_clip_path(&p, &krilla::paint::FillRule::NonZero),
                // Keep Push/Pop balanced even for an empty clip.
                None => s.push_transform(&Transform::identity()),
            }
        }
        Op::ClipPath(f) => match figure_path(f) {
            Some(p) => s.push_clip_path(&p, &fill_rule(f.even_odd)),
            None => s.push_transform(&Transform::identity()),
        },
        Op::Pop => s.pop(),
        Op::Mark { .. } => {}
    }
}

/// Write `pages` as a PDF, archival (PDF/A-2) and/or tagged as `opts` asks. Tagging needs
/// the structure `tags` recorded by layout. An archival file that can't be accessible
/// PDF/A-2a (no document language, say) falls back to PDF/A-2b, still tagged.
pub fn render(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
    out: &Path,
) -> Result<(), EngineError> {
    render_with(pages, fonts, meta, title, out, PdfOptions::default(), None)
}

/// [`render`] into memory.
pub fn render_bytes(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
) -> Result<Vec<u8>, EngineError> {
    build(pages, fonts, meta, title, Validator::None, None)
}

/// The document a PDF was made from, embedded in a PDF/A-3 as its source.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// File name shown in the viewer's attachments ("Report.docx").
    pub name: &'a str,
    /// What the source is, by format key ("docx", "odt", "rtf", "pptx", "xlsx").
    pub kind: &'a str,
    pub bytes: &'a [u8],
}

impl Source<'_> {
    fn mime(&self) -> &'static str {
        match self.kind {
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "odt" => "application/vnd.oasis.opendocument.text",
            "rtf" => "application/rtf",
            _ => "application/octet-stream",
        }
    }
}

/// The standards to try, best first: a tagged archival file that can't be "a" (no
/// document language) falls back to "b", still tagged.
fn validators(opts: PdfOptions, tagged: bool) -> &'static [Validator] {
    match (opts.accessible, opts.archival, opts.attach_source, tagged) {
        (true, ..) => &[Validator::UA1],
        (false, true, true, true) => &[Validator::A3_A, Validator::A3_B],
        (false, true, true, false) => &[Validator::A3_B],
        (false, true, false, true) => &[Validator::A2_A, Validator::A2_B],
        (false, true, false, false) => &[Validator::A2_B],
        (false, false, ..) => &[Validator::None],
    }
}

pub fn render_with(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
    out: &Path,
    opts: PdfOptions,
    tags: Option<&Tags>,
) -> Result<(), EngineError> {
    render_from(pages, fonts, meta, title, out, opts, tags, None)
}

/// [`render_with`], with the source document for PDF/A-3 (`opts.attach_source`).
/// PDF/UA (`opts.accessible`) needs `tags`: without them it fails with that said.
#[allow(clippy::too_many_arguments)]
pub fn render_from(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
    out: &Path,
    opts: PdfOptions,
    tags: Option<&Tags>,
    source: Option<Source<'_>>,
) -> Result<(), EngineError> {
    let tags = tags.filter(|_| opts.tagged || opts.accessible);
    if opts.accessible && tags.is_none() {
        return Err(EngineError::Invalid(
            "PDF/UA needs the document's structure, which wasn't recorded".into(),
        ));
    }
    let source = source.filter(|_| opts.archival && opts.attach_source && !opts.accessible);
    let mut first_err = None;
    for &v in validators(opts, tags.is_some()) {
        match build_with(pages, fonts, meta, title, v, tags, source) {
            Ok(bytes) => {
                std::fs::write(out, bytes)?;
                return Ok(());
            }
            Err(e) => {
                first_err.get_or_insert(e);
            }
        }
    }
    Err(first_err.unwrap_or_else(|| EngineError::Encode("PDF: nothing to write".into())))
}

/// A krilla error in words a user can act on; `standard` names what was asked for.
fn describe(e: KrillaError, standard: &str) -> EngineError {
    let KrillaError::Validation(errs) = &e else {
        return EngineError::Encode(format!("PDF: {e:?}"));
    };
    let mut why: Vec<String> = Vec::new();
    for v in errs {
        let s = match v {
            ValidationError::ContainsNotDefGlyph(_, _, text) => format!(
                "no installed font has a character in \u{201c}{}\u{201d}, and PDF/A needs every character drawn",
                text.chars().take(20).collect::<String>()
            ),
            ValidationError::RestrictedLicense(_) => {
                "a font's licence doesn't allow embedding, which PDF/A needs".to_string()
            }
            ValidationError::MissingCMYKProfile => {
                "CMYK colours need a colour profile for PDF/A".to_string()
            }
            ValidationError::NoDocumentTitle => "the document has no title".to_string(),
            ValidationError::MissingAltText(_) => {
                "a picture or chart has no alternative text".to_string()
            }
            ValidationError::MissingHeadingTitle => "a heading has no text".to_string(),
            ValidationError::MissingDocumentOutline => {
                "the document has no outline (bookmarks)".to_string()
            }
            ValidationError::MissingAnnotationAltText(_) => "a link has no description".to_string(),
            ValidationError::MissingTagging => {
                "the document's structure (tags) is missing".to_string()
            }
            ValidationError::EmbeddedFile(..) => {
                "the attached original can't be embedded in this standard".to_string()
            }
            other => format!("{other:?}").chars().take(80).collect(),
        };
        if !why.contains(&s) {
            why.push(s);
        }
    }
    why.truncate(3);
    EngineError::Invalid(format!("can't make this {standard}: {}", why.join("; ")))
}

/// What a run of drawing operations is in the structure tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Node(u32),
    Artifact,
    Header,
    Footer,
}

/// Collects marked content into the structure tree in reading order: a node takes its
/// place under its parent when its first content is drawn.
struct Tree {
    tags: Tags,
    kids: HashMap<u32, Vec<Kid>>,
    root: Vec<Kid>,
    placed: std::collections::HashSet<u32>,
}

enum Kid {
    Node(u32),
    Leaf(Identifier),
    Group(TagGroup),
}

impl Tree {
    fn new(tags: &Tags) -> Tree {
        Tree {
            tags: tags.clone(),
            kids: HashMap::new(),
            root: Vec::new(),
            placed: Default::default(),
        }
    }

    fn is_figure(&self, n: u32) -> bool {
        matches!(self.tags.role(n), Some(Role::Figure(_)))
    }

    fn place(&mut self, node: u32) {
        let mut n = node;
        while self.placed.insert(n) {
            match self.tags.parent(n) {
                Some(p) => {
                    self.kids.entry(p).or_default().push(Kid::Node(n));
                    n = p;
                }
                None => {
                    self.root.push(Kid::Node(n));
                    break;
                }
            }
        }
    }

    fn leaf(&mut self, node: u32, id: Identifier) {
        self.kids.entry(node).or_default().push(Kid::Leaf(id));
        self.place(node);
    }

    fn group(&mut self, node: Option<u32>, g: TagGroup) {
        match node {
            Some(n) => {
                self.kids.entry(n).or_default().push(Kid::Group(g));
                self.place(n);
            }
            None => self.root.push(Kid::Group(g)),
        }
    }

    fn kind(&self, n: u32) -> TagKind {
        match self.tags.role(n) {
            // A heading with no text (an empty heading paragraph) reads as a paragraph.
            Some(Role::H(_, text)) if text.trim().is_empty() => Tag::P.into(),
            Some(Role::H(level, text)) => Tag::Hn(
                NonZeroU16::new(*level as u16).unwrap_or(NonZeroU16::MIN),
                Some(text.clone()),
            )
            .into(),
            Some(Role::L(num)) => Tag::L(match num {
                Numbering::None => ListNumbering::None,
                Numbering::Disc => ListNumbering::Disc,
                Numbering::Decimal => ListNumbering::Decimal,
                Numbering::LowerRoman => ListNumbering::LowerRoman,
                Numbering::UpperRoman => ListNumbering::UpperRoman,
                Numbering::LowerAlpha => ListNumbering::LowerAlpha,
                Numbering::UpperAlpha => ListNumbering::UpperAlpha,
            })
            .into(),
            Some(Role::LI) => Tag::LI.into(),
            Some(Role::LBody) => Tag::LBody.into(),
            Some(Role::Table) => Tag::Table.into(),
            Some(Role::TR) => Tag::TR.into(),
            Some(Role::TH) => Tag::TH(TableHeaderScope::Column).into(),
            Some(Role::TD) => Tag::TD.into(),
            // Every figure says something: an empty description becomes "Picture".
            Some(Role::Figure(alt)) => Tag::Figure(Some(if alt.trim().is_empty() {
                "Picture".to_string()
            } else {
                alt.clone()
            }))
            .into(),
            Some(Role::Note) => Tag::Note.into(),
            Some(Role::P) | None => Tag::P.into(),
        }
    }

    fn build_kids(&mut self, kids: Vec<Kid>) -> Vec<Node> {
        kids.into_iter()
            .map(|k| match k {
                Kid::Leaf(id) => Node::from(id),
                Kid::Group(g) => Node::from(g),
                Kid::Node(n) => {
                    let own = self.kids.remove(&n).unwrap_or_default();
                    let children = self.build_kids(own);
                    Node::from(TagGroup::with_children(self.kind(n), children))
                }
            })
            .collect()
    }

    fn finish(mut self, lang: Option<String>) -> TagTree {
        let root = std::mem::take(&mut self.root);
        let mut tree = TagTree::new().with_lang(lang);
        for n in self.build_kids(root) {
            tree.push(n);
        }
        tree
    }
}

/// The current UTC time, for the document's creation date (PDF/A requires one).
fn now() -> krilla::metadata::DateTime {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = crate::archive::civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    krilla::metadata::DateTime::new(y.clamp(1970, 9999) as u16)
        .month(m as u8)
        .day(d as u8)
        .hour((rem / 3600) as u8)
        .minute((rem / 60 % 60) as u8)
        .second((rem % 60) as u8)
        .utc_offset_hour(0)
        .utc_offset_minute(0)
}

fn build(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
    validator: Validator,
    tags: Option<&Tags>,
) -> Result<Vec<u8>, EngineError> {
    build_with(pages, fonts, meta, title, validator, tags, None)
}

fn build_with(
    pages: &[PageOut],
    fonts: &mut Fonts,
    meta: &Meta,
    title: &str,
    validator: Validator,
    tags: Option<&Tags>,
    source: Option<Source<'_>>,
) -> Result<Vec<u8>, EngineError> {
    // PDF/UA allows smooth pictures; the PDF/A levels don't.
    let archival = !matches!(validator, Validator::None | Validator::UA1);
    let standard = match validator {
        Validator::UA1 => "PDF/UA",
        _ => "PDF/A",
    };
    // A title the viewer shows (PDF/UA): the document's own, else the file name.
    let shown = meta
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| title.to_string());
    let shown = if shown.trim().is_empty() {
        "Document".to_string()
    } else {
        shown
    };
    let mut doc = krilla::Document::new_with(SerializeSettings {
        configuration: Configuration::new_with_validator(validator),
        enable_tagging: tags.is_some(),
        ..Default::default()
    });
    let mut md = krilla::metadata::Metadata::new().title(shown.clone());
    if let Some(a) = &meta.author {
        md = md.authors(vec![a.clone()]);
    }
    if let Some(s) = &meta.subject {
        md = md.description(s.clone());
    }
    if let Some(k) = &meta.keywords {
        md = md.keywords(
            k.split([',', ';'])
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
        );
    }
    if let Some(l) = &meta.language {
        md = md.language(l.clone());
    }
    if validator != Validator::None {
        md = md.creation_date(now());
    }
    doc.set_metadata(md);
    if let Some(src) = source {
        let file = krilla::embed::EmbeddedFile {
            path: src.name.to_string(),
            mime_type: krilla::embed::MimeType::new(src.mime()),
            description: Some("The original document this PDF was made from".into()),
            association_kind: krilla::embed::AssociationKind::Source,
            data: src.bytes.to_vec().into(),
            modification_date: Some(now()),
            // Office files are zips already; RTF is plain text and packs well.
            compress: Some(src.kind == "rtf"),
            location: None,
        };
        if doc.embed_file(file).is_none() {
            return Err(EngineError::Encode(
                "couldn't attach the original document".into(),
            ));
        }
    }

    let dests: HashMap<&str, (usize, f32)> = pages
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.dests.iter().map(move |(n, y)| (n.as_str(), (i, *y))))
        .collect();
    let mut cache = Cache {
        interpolate: !archival,
        ..Cache::default()
    };
    let mut tree = tags.map(Tree::new);

    for page in pages {
        let settings = PageSettings::from_wh(page.w.max(1.0), page.h.max(1.0))
            .ok_or_else(|| EngineError::Encode("bad page size".into()))?;
        let mut pg = doc.start_page_with(settings);
        let mut s = pg.surface();
        // Where each tagged run of text is, to tie links to the paragraph they're in.
        let mut text_at: Vec<([f32; 4], u32)> = Vec::new();
        match &mut tree {
            None => {
                for op in &page.ops {
                    draw_op(&mut s, op, fonts, &mut cache, 0);
                }
            }
            Some(tree) => {
                let mut cur: Option<u32> = None;
                let mut open: Option<Class> = None;
                for op in &page.ops {
                    match op {
                        Op::Mark { node, .. } => {
                            cur = Some(*node);
                            continue;
                        }
                        // Marked content never crosses a save/restore of the graphics
                        // state: the run is closed and reopened around it.
                        Op::Push(_) | Op::Clip(_) | Op::ClipPath(_) | Op::Pop => {
                            if open.take().is_some() {
                                s.end_tagged();
                            }
                        }
                        _ => {
                            let class = op_class(tree, op, &mut cur);
                            if open != Some(class) {
                                if open.is_some() {
                                    s.end_tagged();
                                }
                                start(&mut s, tree, class);
                                open = Some(class);
                            }
                        }
                    }
                    if let (Op::Text { x, y, seg, .. }, Some(Class::Node(n))) = (op, open) {
                        text_at.push(([*x, y - seg.asc, x + seg.width, y + seg.desc], n));
                    }
                    draw_op(&mut s, op, fonts, &mut cache, 0);
                }
                if open.is_some() {
                    s.end_tagged();
                }
            }
        }
        s.finish();
        for (r, link) in &page.links {
            let Some(rect) =
                Rect::from_ltrb(r[0], r[1], r[2].max(r[0] + 0.5), r[3].max(r[1] + 0.5))
            else {
                continue;
            };
            let (target, alt) = match &**link {
                Link::Url(u) => (
                    Target::Action(Action::Link(LinkAction::new(u.clone()))),
                    u.clone(),
                ),
                Link::Anchor(a) => match dests.get(a.as_str()) {
                    Some(&(pi, y)) => (
                        Target::Destination(Destination::Xyz(XyzDestination::new(
                            pi,
                            Point::from_xy(0.0, y),
                        ))),
                        format!("Go to page {}", pi + 1),
                    ),
                    None => continue,
                },
            };
            let annot = Annotation::new_link(LinkAnnotation::new(rect, target), Some(alt));
            match &mut tree {
                None => pg.add_annotation(annot),
                Some(tree) => {
                    let id = pg.add_tagged_annotation(annot);
                    let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
                    let node = text_at
                        .iter()
                        .find(|(b, _)| cx >= b[0] && cx <= b[2] && cy >= b[1] && cy <= b[3])
                        .map(|(_, n)| *n);
                    tree.group(node, TagGroup::with_children(Tag::Link, vec![id.into()]));
                }
            }
        }
        pg.finish();
    }

    // Outline from the headings, nested by level.
    let mut roots: Vec<OutlineItem> = Vec::new();
    for (pi, p) in pages.iter().enumerate() {
        for (level, text, y) in &p.heads {
            let item = OutlineItem {
                level: *level,
                text: text.chars().take(120).collect(),
                page: pi,
                y: *y,
                kids: Vec::new(),
            };
            let mut list = &mut roots;
            loop {
                let deeper = list.last().is_some_and(|l| l.level < item.level);
                if !deeper {
                    break;
                }
                list = &mut list.last_mut().expect("checked non-empty").kids;
            }
            list.push(item);
        }
    }
    // PDF/UA wants an outline even without headings: one entry for the document.
    if roots.is_empty() && validator == Validator::UA1 {
        roots.push(OutlineItem {
            level: 1,
            text: shown.chars().take(120).collect(),
            page: 0,
            y: 0.0,
            kids: Vec::new(),
        });
    }
    if !roots.is_empty() {
        let mut outline = Outline::new();
        for r in roots {
            outline.push_child(to_node(r));
        }
        doc.set_outline(outline);
    }
    if let Some(tree) = tree {
        doc.set_tag_tree(tree.finish(meta.language.clone()));
    }
    doc.finish().map_err(|e| describe(e, standard))
}

/// What `op` is, given the node marked last; content with no mark at all (slides and
/// sheets drawn without structure) gets a paragraph, pictures a figure of their own.
fn op_class(tree: &mut Tree, op: &Op, cur: &mut Option<u32>) -> Class {
    let fixed = |n: u32| match n {
        HEADER => Some(Class::Header),
        FOOTER => Some(Class::Footer),
        ARTIFACT => Some(Class::Artifact),
        _ => None,
    };
    if let Some(c) = cur.and_then(fixed) {
        return c;
    }
    let figure = cur.filter(|&n| tree.is_figure(n));
    match op {
        Op::Text { .. } => match cur {
            Some(n) => Class::Node(*n),
            None => {
                let n = tree.tags.add(Role::P, None);
                *cur = Some(n);
                Class::Node(n)
            }
        },
        Op::Image { .. } => match figure {
            Some(n) => Class::Node(n),
            None => Class::Node(tree.tags.add(Role::Figure("Picture".into()), None)),
        },
        _ => figure.map_or(Class::Artifact, Class::Node),
    }
}

fn start(s: &mut Surface<'_>, tree: &mut Tree, class: Class) {
    match class {
        Class::Artifact => {
            s.start_tagged(ContentTag::Artifact(ArtifactType::Other));
        }
        Class::Header => {
            s.start_tagged(ContentTag::Artifact(ArtifactType::Header));
        }
        Class::Footer => {
            s.start_tagged(ContentTag::Artifact(ArtifactType::Footer));
        }
        Class::Node(n) => {
            let tag = if tree.is_figure(n) {
                ContentTag::Other
            } else {
                ContentTag::Span(SpanTag::empty())
            };
            let id = s.start_tagged(tag);
            tree.leaf(n, id);
        }
    }
}
