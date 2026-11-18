//! A played-back [`Picture`] as layout ops, so the PDF writer draws it like any other
//! vector content: paths stay paths, text is shaped with the document's fonts (and their
//! fallbacks), bitmaps are embedded once each.

use std::sync::Arc;

use super::{Item, Picture, TextItem};
use crate::docxlayout::fonts::Fonts;
use crate::docxlayout::layout::{Figure, Op, Paint};
use crate::docxlayout::model::Media;
use crate::docxlayout::props::BorderStyle;
use crate::docxlayout::text::{PGlyph, Seg};

fn figure(path: &[crate::docxlayout::layout::PathCmd], even_odd: bool) -> Figure {
    Figure {
        x: 0.0,
        y: 0.0,
        path: path.to_vec(),
        fill: None,
        stroke: None,
        even_odd,
    }
}

/// Ops in picture coordinates (points, y down, origin top-left, `pic.w` × `pic.h`).
pub(crate) fn ops(pic: &Picture, fonts: &mut Fonts) -> Vec<Op> {
    let mut out = Vec::with_capacity(pic.items.len());
    for it in &pic.items {
        match it {
            Item::Fill {
                path,
                color,
                alpha,
                even_odd,
            } => {
                let mut f = figure(path, *even_odd);
                f.fill = Some(Paint::Solid(*color, *alpha));
                out.push(Op::Figure(Arc::new(f)));
            }
            Item::Stroke { path, pen } => {
                let mut f = figure(path, false);
                f.stroke = Some(pen.clone());
                out.push(Op::Figure(Arc::new(f)));
            }
            Item::Gradient {
                path,
                from,
                to,
                c0,
                c1,
            } => {
                let mut f = figure(path, false);
                f.fill = Some(Paint::Linear {
                    from: *from,
                    to: *to,
                    stops: vec![(0.0, *c0, 1.0), (1.0, *c1, 1.0)],
                });
                out.push(Op::Figure(Arc::new(f)));
            }
            Item::Image { png, m } => {
                out.push(Op::Push(*m));
                out.push(Op::Image {
                    media: Arc::new(Media { bytes: png.clone() }),
                    x: 0.0,
                    y: 0.0,
                    w: 1.0,
                    h: 1.0,
                    crop: [0.0; 4],
                });
                out.push(Op::Pop);
            }
            Item::Clip { path, even_odd } => {
                out.push(Op::ClipPath(Arc::new(figure(path, *even_odd))));
            }
            Item::Unclip => out.push(Op::Pop),
            Item::Text(t) => text(t, fonts, &mut out),
        }
    }
    out
}

/// One text item: shaped run by run (each in a face that has its characters), placed by
/// the metafile's own advances when it gives them, aligned on the reference point.
fn text(t: &TextItem, fonts: &mut Fonts, out: &mut Vec<Op>) {
    let Some(face) = fonts.face(&t.family, t.bold, t.italic) else {
        return;
    };
    let (asc, desc) = {
        let f = fonts.get(face);
        (f.asc, f.desc)
    };
    let size = if t.cell {
        t.size / (asc + desc).max(0.5)
    } else {
        t.size
    };
    if !(size > 0.0 && size.is_finite()) {
        return;
    }
    // Symbol fonts (Wingdings, Symbol) map their characters at U+F000 + code; text
    // written with the plain code reaches the same glyph there, as in Word.
    let chars: Vec<char> = t
        .text
        .chars()
        .map(|c| {
            let f = fonts.get(face);
            match char::from_u32(0xF000 + c as u32) {
                Some(s) if (c as u32) < 0x100 && !f.has(c) && f.has(s) => s,
                _ => c,
            }
        })
        .collect();
    // Runs of characters that one face covers.
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        let f = fonts.cover(face, c, t.bold, t.italic);
        match runs.last_mut() {
            Some((rf, _, end)) if *rf == f => *end = i + 1,
            _ => runs.push((f, i, i + 1)),
        }
    }
    let rs = crate::chart::draw::style(&t.family, size, t.color, t.bold);
    let mut segs: Vec<(f32, Seg)> = Vec::new();
    let mut x = 0.0f32;
    for (f, a, b) in runs {
        let s: String = chars[a..b].iter().collect();
        let shaped = fonts.shape(f, &s, false, true);
        // Byte offset of each char in `s` → its index in the item.
        let starts: Vec<usize> = s.char_indices().map(|(o, _)| o).collect();
        let char_at = |byte: u32| {
            starts
                .partition_point(|&o| o <= byte as usize)
                .saturating_sub(1)
        };
        let mut glyphs = Vec::with_capacity(shaped.len());
        let mut seen = std::collections::HashSet::new();
        for (k, g) in shaped.iter().enumerate() {
            let ci = char_at(g.cluster);
            let next = shaped
                .iter()
                .skip(k + 1)
                .map(|n| n.cluster)
                .find(|&c| c != g.cluster);
            let end_byte = next.map_or(s.len(), |c| c as usize);
            let adv = if t.dx.is_empty() {
                g.adv * size
            } else if seen.insert(g.cluster) {
                // The cluster's characters' advances, on its first glyph.
                let last = char_at(end_byte.saturating_sub(1) as u32);
                (ci..=last.max(ci))
                    .map(|i| t.dx.get(a + i).copied().unwrap_or(0.0))
                    .sum()
            } else {
                0.0
            };
            glyphs.push(PGlyph {
                id: g.id,
                start: g.cluster,
                end: end_byte as u32,
                adv,
                dx: g.dx * size,
                dy: g.dy * size,
                space: s[g.cluster as usize..].starts_with(' '),
            });
        }
        let width: f32 = glyphs.iter().map(|g| g.adv).sum();
        let (fa, fd) = {
            let fc = fonts.get(f);
            (fc.asc * size, fc.desc * size)
        };
        segs.push((
            x,
            Seg {
                face: f,
                size,
                rise: 0.0,
                glyphs,
                text: s,
                width,
                asc: fa,
                desc: fd,
                rs: rs.clone(),
                link: None,
            },
        ));
        x += width;
    }
    let total = x;
    let x0 = match t.halign {
        1 => -total / 2.0,
        2 => -total,
        _ => 0.0,
    };
    let base = match t.valign {
        0 => asc * size,
        2 => -desc * size,
        _ => 0.0,
    };
    out.push(Op::Push(t.m));
    for (sx, seg) in segs {
        out.push(Op::Text {
            x: x0 + sx,
            y: base,
            seg,
            color: t.color,
        });
    }
    let line = |y: f32, out: &mut Vec<Op>| {
        out.push(Op::Line {
            x1: x0,
            y1: y,
            x2: x0 + total,
            y2: y,
            width: (size * 0.06).max(0.1),
            color: t.color,
            style: BorderStyle::Single,
        })
    };
    if t.underline {
        line(base + size * 0.12, out);
    }
    if t.strike {
        line(base - size * 0.28, out);
    }
    out.push(Op::Pop);
}
