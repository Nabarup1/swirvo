//! PPTX → PDF without PowerPoint or LibreOffice (docs/16 §8).
//!
//! `read` resolves each slide against its layout and master (placeholders, text styles,
//! theme colours, style references) into positioned items. This module draws them: shapes
//! as paths with solid or gradient fills, pictures, tables, and text laid out by the Word
//! engine's text-box layout (same fonts, line breaking and bullets), then written by the
//! same PDF renderer. Slide titles become PDF bookmarks.

mod geom;
mod read;
pub(crate) mod style;

use std::path::Path;
use std::sync::Arc;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, PdfOptions, StepRequest};

use crate::docxlayout::fonts::Fonts;
use crate::docxlayout::layout::{Boxes, Figure, Op, PageOut, Paint, PathCmd, Pen};
use crate::docxlayout::model::{Block, Document, SectPr, Section};
use crate::docxlayout::props::VAlign;
use crate::docxlayout::tags::{ARTIFACT, Role};
use read::{Deck, Item, Kind, Place, Text};

/// Convert PPTX bytes to a PDF at `out`; returns the slide count.
pub fn convert(
    bytes: &[u8],
    title: &str,
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    convert_as(bytes, title, out, PdfOptions::default(), cancel)
}

/// [`convert`], archival and/or tagged as `opts` asks.
pub fn convert_as(
    bytes: &[u8],
    title: &str,
    out: &Path,
    opts: PdfOptions,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let deck = read::read(bytes)?;
        cancel.check()?;
        if !Fonts::available() {
            return Err(EngineError::Encode(
                "no fonts are installed, so the PDF can't be typeset".into(),
            ));
        }
        let meta = deck.meta.clone();
        let (pages, mut fonts, tags) = draw(
            deck,
            Fonts::new(Default::default()),
            opts.tagged || opts.accessible,
            cancel,
        )?;
        let name = format!("{title}.pptx");
        crate::docxlayout::render::render_from(
            &pages,
            &mut fonts,
            &meta,
            title,
            out,
            opts,
            tags.as_ref(),
            Some(crate::docxlayout::render::Source {
                name: &name,
                kind: "pptx",
                bytes,
            }),
        )?;
        Ok(pages.len())
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this presentation".into()))?
}

/// The first slide as a one-page PDF in memory (thumbnails).
pub fn first_page(bytes: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let deck = read::read(bytes)?;
        cancel.check()?;
        if !Fonts::available() {
            return Err(EngineError::Encode("no fonts".into()));
        }
        let meta = deck.meta.clone();
        let (pages, mut fonts, _) = draw(deck, Fonts::new(Default::default()), false, cancel)?;
        crate::docxlayout::render::render_bytes(&pages[..pages.len().min(1)], &mut fonts, &meta, "")
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this presentation".into()))?
}

/// `local (0..w, 0..h)` → slide, for an item turned and mirrored about its centre. None
/// when the item is upright and unmirrored (it's then drawn in place).
fn matrix(at: &Place, rot: f32, fx: bool, fy: bool) -> Option<[f32; 6]> {
    if rot.rem_euclid(360.0) == 0.0 && !fx && !fy {
        return None;
    }
    let (s, c) = rot.to_radians().sin_cos();
    let (sx, sy) = (if fx { -1.0 } else { 1.0 }, if fy { -1.0 } else { 1.0 });
    let (cx, cy) = (at.x + at.w / 2.0, at.y + at.h / 2.0);
    let (hw, hh) = (at.w / 2.0, at.h / 2.0);
    Some([
        c * sx,
        s * sx,
        -s * sy,
        c * sy,
        cx - c * sx * hw + s * sy * hh,
        cy - s * sx * hw - c * sy * hh,
    ])
}

/// An arrowhead at `tip`, pointing away from `from`.
fn arrow(tip: (f32, f32), from: (f32, f32), pen: &Pen) -> Figure {
    let (dx, dy) = (tip.0 - from.0, tip.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let l = (pen.width * 3.0).max(5.0);
    let w = l * 0.6;
    let base = (tip.0 - ux * l, tip.1 - uy * l);
    Figure {
        x: 0.0,
        y: 0.0,
        path: vec![
            PathCmd::Move(tip.0, tip.1),
            PathCmd::Line(base.0 - uy * w, base.1 + ux * w),
            PathCmd::Line(base.0 + uy * w, base.1 - ux * w),
            PathCmd::Close,
        ],
        fill: Some(Paint::Solid(pen.color, pen.alpha)),
        stroke: None,
        even_odd: false,
    }
}

fn end_point(c: &PathCmd) -> Option<(f32, f32)> {
    match *c {
        PathCmd::Move(x, y) | PathCmd::Line(x, y) => Some((x, y)),
        PathCmd::Quad(_, _, x, y) | PathCmd::Cubic(_, _, _, _, x, y) => Some((x, y)),
        PathCmd::Close => None,
    }
}

/// The point a path's last segment comes from (control point for curves).
fn last_from(path: &[PathCmd]) -> Option<((f32, f32), (f32, f32))> {
    let n = path.len();
    let tip = end_point(path.get(n.checked_sub(1)?)?)?;
    let from = match path[n - 1] {
        PathCmd::Quad(cx, cy, _, _) => (cx, cy),
        PathCmd::Cubic(_, _, cx, cy, _, _) => (cx, cy),
        _ => end_point(path.get(n.checked_sub(2)?)?)?,
    };
    Some((tip, from))
}

fn first_from(path: &[PathCmd]) -> Option<((f32, f32), (f32, f32))> {
    let tip = end_point(path.first()?)?;
    let from = match *path.get(1)? {
        PathCmd::Quad(cx, cy, _, _) | PathCmd::Cubic(cx, cy, _, _, _, _) => (cx, cy),
        ref c => end_point(c)?,
    };
    Some((tip, from))
}

fn draw(
    deck: Deck,
    fonts: Fonts,
    tagged: bool,
    cancel: &CancelToken,
) -> Result<(Vec<PageOut>, Fonts, Option<crate::docxlayout::tags::Tags>), EngineError> {
    let (w, h) = (deck.w, deck.h);
    let doc = Document {
        sections: vec![Section {
            sect: SectPr {
                w,
                h,
                top: 0.0,
                bottom: 0.0,
                left: 0.0,
                right: 0.0,
                ..SectPr::default()
            },
            blocks: Vec::new(),
        }],
        default_tab: 72.0,
        compat15: true,
        ..Document::default()
    };
    let mut boxes = Boxes::new(&doc, fonts);
    if tagged {
        boxes.enable_tags();
    }
    let mut pages = Vec::with_capacity(deck.slides.len());
    for slide in deck.slides {
        cancel.check()?;
        let mut page = PageOut {
            w,
            h,
            ops: Vec::new(),
            links: Vec::new(),
            dests: Vec::new(),
            heads: Vec::new(),
        };
        if boxes.tagging() {
            page.ops.push(Op::Mark {
                node: ARTIFACT,
                y: 0.0,
            });
        }
        let full = vec![
            PathCmd::Move(0.0, 0.0),
            PathCmd::Line(w, 0.0),
            PathCmd::Line(w, h),
            PathCmd::Line(0.0, h),
            PathCmd::Close,
        ];
        if let Some(bg) = slide.bg {
            // Backgrounds are given for a unit box; scale gradients to the slide.
            let paint = match bg {
                Paint::Linear { from, to, stops } => Paint::Linear {
                    from: (from.0 * w, from.1 * h),
                    to: (to.0 * w, to.1 * h),
                    stops,
                },
                Paint::Radial { stops, .. } => Paint::Radial {
                    center: (w / 2.0, h / 2.0),
                    radius: (w * w + h * h).sqrt() / 2.0,
                    stops,
                },
                p => p,
            };
            page.ops.push(Op::Figure(Arc::new(Figure {
                x: 0.0,
                y: 0.0,
                path: full,
                fill: Some(paint),
                stroke: None,
                even_odd: false,
            })));
        }
        if let Some(m) = slide.bg_pic {
            page.ops.push(Op::Image {
                media: m,
                x: 0.0,
                y: 0.0,
                w,
                h,
                crop: [0.0; 4],
            });
        }
        if let Some(t) = &slide.title {
            page.heads.push((0, t.clone(), 0.0));
        }
        for item in slide.items {
            draw_item(&mut boxes, item, &mut page);
        }
        pages.push(page);
    }
    let (fonts, tags) = boxes.into_parts();
    Ok((pages, fonts, tags))
}

fn draw_item(boxes: &mut Boxes<'_>, item: Item, page: &mut PageOut) {
    let tagging = boxes.tagging();
    let mark = |node: u32, page: &mut PageOut| {
        if tagging {
            page.ops.push(Op::Mark { node, y: 0.0 });
        }
    };
    let at = item.at;
    let alt = item.alt.clone();
    let is_title = item.title;
    let m = matrix(&at, at.rot, at.flip_h, at.flip_v);
    // Drawn in local coordinates under a transform, or in place.
    let (ox, oy) = if m.is_some() {
        (0.0, 0.0)
    } else {
        (at.x, at.y)
    };
    match item.kind {
        Kind::Shape {
            path,
            open,
            fill,
            pic,
            pen,
            head,
            tail,
            text,
        } => {
            if let Some(m) = m {
                page.ops.push(Op::Push(m));
            }
            if let Some(media) = pic {
                page.ops.push(Op::Image {
                    media,
                    x: ox,
                    y: oy,
                    w: at.w,
                    h: at.h,
                    crop: [0.0; 4],
                });
            }
            if !path.is_empty() && (fill.is_some() || pen.is_some()) {
                page.ops.push(Op::Figure(Arc::new(Figure {
                    x: ox,
                    y: oy,
                    path: path.clone(),
                    fill: if open { None } else { fill },
                    stroke: pen.clone(),
                    even_odd: false,
                })));
            }
            if let Some(pen) = &pen
                && open
            {
                let ends = [
                    (tail.is_some(), last_from(&path)),
                    (head.is_some(), first_from(&path)),
                ];
                for (want, e) in ends {
                    if let (true, Some((tip, from))) = (want, e) {
                        let mut f = arrow(tip, from, pen);
                        f.x = ox;
                        f.y = oy;
                        page.ops.push(Op::Figure(Arc::new(f)));
                    }
                }
            }
            if m.is_some() {
                page.ops.push(Op::Pop);
            }
            if let Some(t) = text {
                // The title placeholder is the slide's heading.
                let heading = is_title
                    .then(|| blocks_text(&t.blocks))
                    .filter(|s| !s.is_empty() && tagging)
                    .map(|s| boxes.add_tag(Role::H(1, s), None));
                if let Some(h) = heading {
                    boxes.fix_tag(h);
                }
                draw_text(boxes, &at, t, page);
                boxes.unfix_tag();
                mark(ARTIFACT, page);
            }
        }
        Kind::Picture { media, crop, pen } => {
            let fig = boxes.add_tag(Role::Figure(alt.unwrap_or_else(|| "Picture".into())), None);
            mark(fig, page);
            if let Some(m) = m {
                page.ops.push(Op::Push(m));
            }
            page.ops.push(Op::Image {
                media,
                x: ox,
                y: oy,
                w: at.w,
                h: at.h,
                crop,
            });
            if let Some(pen) = pen {
                page.ops.push(Op::Figure(Arc::new(Figure {
                    x: ox,
                    y: oy,
                    path: vec![
                        PathCmd::Move(0.0, 0.0),
                        PathCmd::Line(at.w, 0.0),
                        PathCmd::Line(at.w, at.h),
                        PathCmd::Line(0.0, at.h),
                        PathCmd::Close,
                    ],
                    fill: None,
                    stroke: Some(pen),
                    even_odd: false,
                })));
            }
            if m.is_some() {
                page.ops.push(Op::Pop);
            }
            mark(ARTIFACT, page);
        }
        Kind::Chart(c) => {
            let alt = alt.unwrap_or_else(|| match c.title.as_deref().map(str::trim) {
                Some(t) if !t.is_empty() => format!("Chart: {t}"),
                _ => "Chart".into(),
            });
            let fig = boxes.add_tag(Role::Figure(alt), None);
            mark(fig, page);
            boxes.fix_tag(fig);
            let ops = crate::chart::draw(&c, at.w, at.h, boxes);
            boxes.unfix_tag();
            page.ops
                .extend(ops.into_iter().map(|o| o.moved(at.x, at.y)));
            mark(ARTIFACT, page);
        }
        Kind::Table(t) => {
            let (ops, _, links) = boxes.lay(&[Block::Table(t)], at.w);
            page.ops
                .extend(ops.into_iter().map(|o| o.moved(at.x, at.y)));
            mark(ARTIFACT, page);
            page.links.extend(
                links
                    .into_iter()
                    .map(|(r, l)| ([r[0] + at.x, r[1] + at.y, r[2] + at.x, r[3] + at.y], l)),
            );
        }
    }
}

/// The text of some blocks' paragraphs, for a heading's title.
fn blocks_text(blocks: &[Block]) -> String {
    let mut s = String::new();
    for b in blocks {
        if let Block::Para(p) = b {
            for it in &p.items {
                if let crate::docxlayout::model::Item::Text { text, .. } = it {
                    s.push_str(text);
                }
            }
            s.push(' ');
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lay out a shape's text in its box: insets, vertical anchoring, the shape's rotation
/// (text isn't mirrored; a vertical flip turns it upside down, as in PowerPoint) and
/// vertical text.
fn draw_text(boxes: &mut Boxes<'_>, at: &Place, t: Text, page: &mut PageOut) {
    let turn = match t.vert.as_deref() {
        Some("vert") | Some("eaVert") => 90.0,
        Some("vert270") => 270.0,
        _ => 0.0,
    };
    // The text frame: the box, or the box turned for vertical text.
    let (fw, fh) = if turn == 0.0 {
        (at.w, at.h)
    } else {
        (at.h, at.w)
    };
    let inner_w = (fw - t.ins[0] - t.ins[2]).max(4.0);
    let mut width = inner_w;
    if !t.wrap {
        // No wrapping: as wide as the longest line needs.
        let (ops, _, _) = boxes.lay(&t.blocks, 100_000.0);
        let natural = ops
            .iter()
            .filter_map(|o| match o {
                Op::Text { x, seg, .. } => Some(x + seg.width),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        width = width.max(natural + 1.0);
    }
    let (ops, th, links) = boxes.lay(&t.blocks, width);
    let room = fh - t.ins[1] - t.ins[3];
    let off = match t.anchor {
        VAlign::Center => (room - th) / 2.0,
        VAlign::Bottom => room - th,
        VAlign::Top => 0.0,
    };
    // Centre a too-wide unwrapped block on the frame.
    let dx = t.ins[0] - (width - inner_w) / 2.0 * f32::from(!t.wrap && width > inner_w);
    let rot = at.rot + turn + if at.flip_v { 180.0 } else { 0.0 };
    let frame = Place {
        x: at.x + (at.w - fw) / 2.0,
        y: at.y + (at.h - fh) / 2.0,
        w: fw,
        h: fh,
        rot,
        flip_h: false,
        flip_v: false,
    };
    match matrix(&frame, rot, false, false) {
        Some(m) => {
            page.ops.push(Op::Push(m));
            page.ops
                .extend(ops.into_iter().map(|o| o.moved(dx, t.ins[1] + off)));
            page.ops.push(Op::Pop);
        }
        None => {
            let (x, y) = (frame.x + dx, frame.y + t.ins[1] + off);
            page.ops.extend(ops.into_iter().map(|o| o.moved(x, y)));
            page.links.extend(
                links
                    .into_iter()
                    .map(|(r, l)| ([r[0] + x, r[1] + y, r[2] + x, r[3] + y], l)),
            );
        }
    }
}

pub struct PptxLayoutEngine;

impl Engine for PptxLayoutEngine {
    fn id(&self) -> &'static str {
        "pptx-layout"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("pptx-layout", "pptx", "pdf")
                .loss(2)
                .time(80, 60)
                .note("draws the slides: shapes, pictures, tables, text; charts aren't drawn yet"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let bytes = std::fs::read(req.input)?;
        let title = req
            .input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        convert_as(&bytes, &title, req.output, req.pdf, req.cancel).map(|_| ())
    }
}

#[cfg(test)]
pub(crate) mod tests;
