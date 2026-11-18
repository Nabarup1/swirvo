//! Page assembly (docs/13 §3): classify graphics, find tables and figures, decorate text, split
//! off headers and footers, order the rest and build paragraphs. Two passes: every page is
//! prepared first, because furniture is "what repeats across pages".

use super::extract::{self, Mark, RawGlyph, RawPage};
use super::model::*;
use super::order::{self, Atom};
use super::paras::{self, Ctx};
use super::raster::{self, Req};
use super::tables::{self, Grid};
use super::text::{self, TSeg};
use std::collections::HashMap;
use swirvo_core::{CancelToken, EngineError};

const MAX_FLOATS: usize = 600;

/// A page after classification, before reading order.
struct Prepared {
    w: f32,
    h: f32,
    glyphs: Vec<RawGlyph>,
    tables: Vec<(Grid, Vec<Vec<usize>>)>,
    /// Inline pictures (bbox, media index).
    pictures: Vec<(BBox, u32)>,
    floats: Vec<Float>,
    segs: Vec<TSeg>,
    /// Per segment: 0 body, 1 header, 2 footer.
    furniture: Vec<u8>,
}

/// Run the whole analysis.
pub fn analyze(bytes: &[u8], cancel: &CancelToken) -> Result<Layout, EngineError> {
    let ex = extract::extract(bytes, cancel)?;
    let fonts = ex.fonts.fonts;
    let mut media = ex.media;
    let base = media.len() as u32;
    let mut reqs: Vec<Req> = Vec::new();
    let mut pages: Vec<Prepared> = Vec::with_capacity(ex.pages.len());
    for (i, raw) in ex.pages.into_iter().enumerate() {
        cancel.check()?;
        pages.push(prepare(raw, i, base, &mut reqs));
    }
    let has_text = pages.iter().any(|p| p.glyphs.iter().any(|g| g.ch != ' '));
    let (header, footer) = furniture(&mut pages);

    // Horizontal margins are shared by pages of the same size, so headers line up.
    let mut lr: HashMap<(i32, i32), (f32, f32)> = HashMap::new();
    for p in &pages {
        let e = body_extent(p);
        if e.is_void() {
            continue;
        }
        let k = (p.w.round() as i32, p.h.round() as i32);
        let m = lr.entry(k).or_insert((f32::INFINITY, f32::INFINITY));
        m.0 = m.0.min(e.x0.max(0.0));
        m.1 = m.1.min((p.w - e.x1).max(0.0));
    }
    let margins_for = |p: &Prepared| {
        let m = lr
            .get(&(p.w.round() as i32, p.h.round() as i32))
            .copied()
            .unwrap_or((72.0, 72.0));
        (m.0.min(p.w / 3.0), m.1.min(p.w / 3.0))
    };

    let mut out = Layout {
        fonts,
        has_text,
        ..Default::default()
    };
    let mut header_out = None;
    let mut footer_out = None;
    for (i, p) in pages.iter().enumerate() {
        cancel.check()?;
        let (ml, mr) = margins_for(p);
        out.pages.push(layout_page(p, &out.fonts, ml, mr));
        if let Some((hp, ref ids)) = header
            && hp == i
        {
            header_out = Some(furniture_block(
                p,
                &out.fonts,
                ids,
                (ml, mr),
                true,
                (i, pages.len()),
            ));
        }
        if let Some((fp, ref ids)) = footer
            && fp == i
        {
            footer_out = Some(furniture_block(
                p,
                &out.fonts,
                ids,
                (ml, mr),
                false,
                (i, pages.len()),
            ));
        }
    }
    for (pg, p) in out.pages.iter_mut().zip(&pages) {
        pg.header = header_out.is_some() && p.furniture.contains(&1);
        pg.footer = footer_out.is_some() && p.furniture.contains(&2);
    }
    out.header = header_out;
    out.footer = footer_out;

    // Figures and backgrounds, rendered by hayro.
    let rendered = raster::render(bytes, &reqs);
    for m in rendered {
        media.push(m.unwrap_or_else(raster::blank));
    }
    out.media = media;
    out.links = ex.links;
    finish(&mut out);
    Ok(out)
}

fn body_extent(p: &Prepared) -> BBox {
    let mut e = BBox::EMPTY;
    for (s, seg) in p.segs.iter().enumerate() {
        if p.furniture[s] == 0 {
            e = e.union(seg.bbox);
        }
    }
    for (g, _) in &p.tables {
        e = e.union(g.bbox());
    }
    for (b, _) in &p.pictures {
        e = e.union(*b);
    }
    e
}

fn prepare(raw: RawPage, pi: usize, media_base: u32, reqs: &mut Vec<Req>) -> Prepared {
    let RawPage {
        w,
        h,
        mut glyphs,
        marks,
        images,
        links,
    } = raw;
    let page = BBox::new(0.0, 0.0, w, h);
    text::dedupe(&mut glyphs);
    let invisible = glyphs.iter().filter(|g| g.invisible && g.ch != ' ').count();
    let visible = glyphs
        .iter()
        .filter(|g| !g.invisible && g.ch != ' ')
        .count();
    // An OCR text layer over a scan: the text becomes the page, the scan goes.
    let ocr = invisible > 20 && invisible > 2 * visible;
    if ocr {
        for g in &mut glyphs {
            if g.invisible {
                g.invisible = false;
                g.color = BLACK;
            }
        }
    }
    glyphs.retain(|g| !g.invisible && page.inflate(2.0).contains(g.cx(), g.base));

    let mut consumed = vec![false; marks.len()];
    for (i, m) in marks.iter().enumerate() {
        if let Mark::Box { bbox, fill } = m
            && bbox.area() >= 0.9 * page.area()
            && fill.iter().all(|&c| c >= 245)
        {
            consumed[i] = true; // white page background
        }
    }
    vector_bullets(&mut glyphs, &marks, &mut consumed);

    // Tables: a grid needs text in at least two cells.
    let mut tables: Vec<(Grid, Vec<Vec<usize>>)> = Vec::new();
    let mut in_table = vec![false; glyphs.len()];
    for grid in tables::find(&marks, &consumed) {
        let bb = grid.bbox();
        if tables
            .iter()
            .any(|(t, _)| t.bbox().overlap(&bb) > 0.5 * bb.area().min(t.bbox().area()))
        {
            continue;
        }
        let mut cells: Vec<Vec<usize>> = vec![Vec::new(); grid.cells.len()];
        for (i, g) in glyphs.iter().enumerate() {
            if !in_table[i]
                && let Some(c) = grid.cell_at(g.cx(), g.mid_y())
            {
                cells[c].push(i);
            }
        }
        let filled = cells
            .iter()
            .filter(|c| c.iter().any(|&i| glyphs[i].ch != ' '))
            .count();
        if filled < 2 {
            continue;
        }
        for c in &cells {
            for &i in c {
                in_table[i] = true;
            }
        }
        for &m in &grid.consumed {
            consumed[m] = true;
        }
        tables.push((grid, cells));
    }

    decorate(&mut glyphs, &marks, &mut consumed);
    for g in &mut glyphs {
        if let Some((_, l)) = links.iter().find(|(b, _)| b.contains(g.cx(), g.mid_y())) {
            g.link = Some(*l);
        }
    }

    let mut body: Vec<usize> = (0..glyphs.len()).filter(|&i| !in_table[i]).collect();
    let table_boxes: Vec<BBox> = tables.iter().map(|(g, _)| g.bbox()).collect();
    let mut floats: Vec<Float> = Vec::new();
    let mut pictures = Vec::new();
    let mut taken = vec![false; images.len()];
    for f in figures(&marks, &mut consumed, &glyphs, &body, page) {
        for (k, im) in images.iter().enumerate() {
            if im.bbox.overlap(&f.bbox) >= 0.8 * im.bbox.area() {
                taken[k] = true;
            }
        }
        if f.figure {
            body.retain(|&i| !f.bbox.contains(glyphs[i].cx(), glyphs[i].mid_y()));
        }
        let media = media_base + reqs.len() as u32;
        reqs.push(Req {
            page: pi,
            bbox: f.bbox,
            textless: !f.figure,
        });
        let small = f.bbox.h() < 24.0 || f.bbox.w() < 24.0;
        let over_table = table_boxes.iter().any(|t| t.overlap(&f.bbox) > 0.0);
        if f.figure && !small && !over_table {
            pictures.push((f.bbox, media));
        } else {
            floats.push(Float::Picture {
                media,
                bbox: f.bbox,
                behind: !f.figure,
            });
        }
    }
    for (k, im) in images.iter().enumerate() {
        if taken[k] || (ocr && im.bbox.area() >= 0.7 * page.area()) {
            continue;
        }
        let covered = body
            .iter()
            .filter(|&&i| {
                glyphs[i].ch != ' ' && im.bbox.contains(glyphs[i].cx(), glyphs[i].mid_y())
            })
            .count();
        let over_table = table_boxes.iter().any(|t| t.overlap(&im.bbox) > 0.0);
        if covered >= 4 || im.bbox.area() >= 0.6 * page.area() {
            floats.push(Float::Picture {
                media: im.media,
                bbox: im.bbox,
                behind: true,
            });
        } else if im.bbox.h() < 24.0 || im.bbox.w() < 24.0 || over_table {
            floats.push(Float::Picture {
                media: im.media,
                bbox: im.bbox,
                behind: false,
            });
        } else {
            pictures.push((im.bbox, im.media));
        }
    }
    // Shapes go behind everything else: insert them first, in drawing order.
    let mut shapes = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        if consumed[i] || shapes.len() >= MAX_FLOATS {
            continue;
        }
        match *m {
            Mark::Box { bbox, fill } => shapes.push(Float::Rect {
                bbox,
                fill: Some(fill),
                stroke: None,
            }),
            Mark::Frame { bbox, color, width } => shapes.push(Float::Rect {
                bbox,
                fill: None,
                stroke: Some(Border { width, color }),
            }),
            Mark::Rule { color, .. } => shapes.push(Float::Rect {
                bbox: m.bbox(),
                fill: Some(color),
                stroke: None,
            }),
            Mark::Art { .. } => {}
        }
    }
    shapes.append(&mut floats);
    shapes.truncate(MAX_FLOATS);

    let rows = text::rows(&glyphs, &body);
    let segs = text::segments(&glyphs, &rows);
    let furniture = vec![0; segs.len()];
    Prepared {
        w,
        h,
        glyphs,
        tables,
        pictures,
        floats: shapes,
        segs,
        furniture,
    }
}

/// Small round or square vector marks at the start of a text line are list bullets.
fn vector_bullets(glyphs: &mut Vec<RawGlyph>, marks: &[Mark], consumed: &mut [bool]) {
    let mut found = 0;
    for (i, m) in marks.iter().enumerate() {
        if consumed[i] || found > 300 {
            continue;
        }
        let b = match m {
            Mark::Art { bbox } | Mark::Box { bbox, .. } => *bbox,
            _ => continue,
        };
        if !(1.5..=8.0).contains(&b.w()) || !(1.5..=8.0).contains(&b.h()) {
            continue;
        }
        if (b.w() - b.h()).abs() > 0.35 * b.w().max(b.h()) {
            continue;
        }
        let (cy, x1) = (b.cy(), b.x1);
        let next = glyphs
            .iter()
            .filter(|g| {
                g.ch != ' '
                    && cy <= g.base
                    && cy >= g.base - 0.9 * g.size
                    && g.size >= 1.2 * b.w()
                    && g.x0 - x1 >= 0.5
                    && g.x0 - x1 <= 3.0 * g.size
            })
            .min_by(|a, c| a.x0.total_cmp(&c.x0));
        let Some(n) = next else { continue };
        let blocked = glyphs.iter().any(|g| {
            g.ch != ' '
                && (g.base - n.base).abs() < 0.2 * n.size
                && g.x1 <= b.x0 + 0.5
                && b.x0 - g.x1 < 1.5 * n.size
        });
        if blocked {
            continue;
        }
        let mut bullet = n.clone();
        bullet.ch = '•';
        bullet.x0 = b.x0;
        bullet.x1 = b.x1;
        bullet.bold = false;
        bullet.underline = None;
        bullet.link = None;
        glyphs.push(bullet);
        consumed[i] = true;
        found += 1;
    }
}

/// Rules under or through text become underline and strike-through; boxes hugging text
/// become highlight.
fn decorate(glyphs: &mut [RawGlyph], marks: &[Mark], consumed: &mut [bool]) {
    for (i, m) in marks.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        match *m {
            Mark::Rule {
                horizontal: true,
                at,
                lo,
                hi,
                width,
                color,
            } if width <= 3.0 => {
                let pick = |under: bool, g: &RawGlyph| {
                    let d = at - g.base;
                    g.ch != ' '
                        && g.cx() >= lo - 1.0
                        && g.cx() <= hi + 1.0
                        && if under {
                            d >= -0.05 * g.size && d <= 0.35 * g.size
                        } else {
                            d >= -0.45 * g.size && d <= -0.18 * g.size
                        }
                };
                for under in [true, false] {
                    let ids: Vec<usize> = (0..glyphs.len())
                        .filter(|&k| pick(under, &glyphs[k]))
                        .collect();
                    if ids.is_empty() {
                        continue;
                    }
                    let x0 = ids
                        .iter()
                        .map(|&k| glyphs[k].x0)
                        .fold(f32::INFINITY, f32::min);
                    let x1 = ids
                        .iter()
                        .map(|&k| glyphs[k].x1)
                        .fold(f32::NEG_INFINITY, f32::max);
                    let size = glyphs[ids[0]].size;
                    if x1 - x0 >= 0.8 * (hi - lo) && hi - lo <= x1 - x0 + 2.0 * size {
                        for &k in &ids {
                            if under {
                                glyphs[k].underline = Some(color);
                            } else {
                                glyphs[k].strike = true;
                            }
                        }
                        consumed[i] = true;
                        break;
                    }
                }
            }
            Mark::Box { bbox, fill } if bbox.h() <= 40.0 => {
                let ids: Vec<usize> = (0..glyphs.len())
                    .filter(|&k| {
                        glyphs[k].ch != ' ' && bbox.contains(glyphs[k].cx(), glyphs[k].mid_y())
                    })
                    .collect();
                if ids.is_empty() {
                    continue;
                }
                let size = ids.iter().map(|&k| glyphs[k].size).fold(0.0, f32::max);
                let x0 = ids
                    .iter()
                    .map(|&k| glyphs[k].x0)
                    .fold(f32::INFINITY, f32::min);
                let x1 = ids
                    .iter()
                    .map(|&k| glyphs[k].x1)
                    .fold(f32::NEG_INFINITY, f32::max);
                let one_line = ids
                    .iter()
                    .all(|&k| (glyphs[k].base - glyphs[ids[0]].base).abs() < 0.3 * size);
                if one_line
                    && bbox.h() <= 2.0 * size
                    && x1 - x0 >= 0.6 * bbox.w()
                    && bbox.w() <= x1 - x0 + 3.0 * size
                {
                    for &k in &ids {
                        glyphs[k].highlight = Some(fill);
                    }
                    consumed[i] = true;
                }
            }
            _ => {}
        }
    }
}

struct Fig {
    bbox: BBox,
    /// true: render with its text (a chart, a logo); false: a background behind live text.
    figure: bool,
}

/// Cluster art (and the boxes and rules that touch it) into figures and backgrounds.
fn figures(
    marks: &[Mark],
    consumed: &mut [bool],
    glyphs: &[RawGlyph],
    body: &[usize],
    page: BBox,
) -> Vec<Fig> {
    const NEAR: f32 = 6.0;
    const CELL: f32 = 32.0;
    let items: Vec<usize> = (0..marks.len())
        .filter(|&i| {
            !consumed[i]
                && match &marks[i] {
                    Mark::Art { .. } => true,
                    m => m.bbox().area() <= 0.25 * page.area(),
                }
        })
        .collect();
    if !items.iter().any(|&i| matches!(marks[i], Mark::Art { .. })) {
        return Vec::new();
    }
    let mut parent: Vec<usize> = (0..items.len()).collect();
    fn find(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (k, &i) in items.iter().enumerate() {
        let b = marks[i].bbox().inflate(NEAR / 2.0);
        let (cx0, cy0) = ((b.x0 / CELL).floor() as i32, (b.y0 / CELL).floor() as i32);
        let (cx1, cy1) = ((b.x1 / CELL).floor() as i32, (b.y1 / CELL).floor() as i32);
        if (cx1 - cx0 + 1) * (cy1 - cy0 + 1) > 4096 {
            continue;
        }
        for x in cx0..=cx1 {
            for y in cy0..=cy1 {
                let cell = grid.entry((x, y)).or_default();
                for &o in cell.iter() {
                    if marks[items[o]].bbox().touches(&marks[i].bbox(), NEAR) {
                        let (a, c) = (find(&mut parent, o), find(&mut parent, k));
                        if a != c {
                            parent[c] = a;
                        }
                    }
                }
                cell.push(k);
            }
        }
    }
    let mut clusters: HashMap<usize, Vec<usize>> = HashMap::new();
    for (k, &item) in items.iter().enumerate() {
        let r = find(&mut parent, k);
        clusters.entry(r).or_default().push(item);
    }
    // Short text right next to a figure is its labels (axes, legends, callouts).
    let rows = text::rows(glyphs, body);
    let segs = text::segments(glyphs, &rows);
    let mut out = Vec::new();
    for members in clusters.values() {
        let arts = members
            .iter()
            .filter(|&&i| matches!(marks[i], Mark::Art { .. }))
            .count();
        if arts == 0 && members.len() < 12 {
            continue; // plain boxes and rules: native shapes
        }
        let bbox = members
            .iter()
            .fold(BBox::EMPTY, |b, &i| b.union(marks[i].bbox()))
            .intersect(page);
        for &i in members {
            consumed[i] = true;
        }
        if bbox.is_void() || bbox.area() < 4.0 {
            continue;
        }
        let density = |b: &BBox| {
            let n = body
                .iter()
                .filter(|&&i| glyphs[i].ch != ' ' && b.contains(glyphs[i].cx(), glyphs[i].mid_y()))
                .count();
            n as f32 / b.area().max(1.0)
        };
        let figure = density(&bbox) < 0.004 && bbox.area() < 0.7 * page.area();
        let mut grown = bbox;
        if figure {
            let near = bbox.inflate(14.0);
            for s in &segs {
                let n = s.ids.iter().filter(|&&i| glyphs[i].ch != ' ').count();
                if n <= 12 && s.bbox.w() <= 90.0 && !near.intersect(s.bbox).is_void() {
                    grown = grown.union(s.bbox.inflate(1.0));
                }
            }
            grown = grown.intersect(page);
        }
        out.push(Fig {
            bbox: grown,
            figure: figure && density(&grown) < 0.006,
        });
    }
    out.sort_by(|a, b| a.bbox.y0.total_cmp(&b.bbox.y0));
    out
}

fn seg_text(p: &Prepared, s: usize) -> String {
    let mut t = String::new();
    for &i in &p.segs[s].ids {
        t.push(p.glyphs[i].ch);
    }
    t
}

fn norm_digits(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut prev_digit = false;
    for c in t.split_whitespace().collect::<Vec<_>>().join(" ").chars() {
        if c.is_ascii_digit() {
            if !prev_digit {
                out.push('#');
            }
            prev_digit = true;
        } else {
            out.push(c);
            prev_digit = false;
        }
    }
    out
}

type Furn = Option<(usize, Vec<usize>)>;

/// Headers and footers: top/bottom segments whose text (digits aside) repeats at the same
/// height on most pages. Returns the template page and its segments for each.
fn furniture(pages: &mut [Prepared]) -> (Furn, Furn) {
    let n = pages.len();
    if n < 2 {
        return (None, None);
    }
    let key = |p: &Prepared, s: usize| -> Option<(u8, String, i32)> {
        let seg = &p.segs[s];
        let kind = if seg.base < 0.12 * p.h {
            1
        } else if seg.base > 0.88 * p.h {
            2
        } else {
            return None;
        };
        Some((
            kind,
            norm_digits(&seg_text(p, s)),
            (seg.base / 3.0).round() as i32,
        ))
    };
    let mut seen: HashMap<(u8, String, i32), Vec<usize>> = HashMap::new();
    for (pi, p) in pages.iter().enumerate() {
        for s in 0..p.segs.len() {
            if let Some(k) = key(p, s) {
                let v = seen.entry(k).or_default();
                if v.last() != Some(&pi) {
                    v.push(pi);
                }
            }
        }
    }
    let need = 2.max((0.6 * n as f32).ceil() as usize);
    let count = |k: &(u8, String, i32)| {
        let mut pages: Vec<usize> = Vec::new();
        for dy in -1..=1 {
            if let Some(v) = seen.get(&(k.0, k.1.clone(), k.2 + dy)) {
                pages.extend(v);
            }
        }
        pages.sort_unstable();
        pages.dedup();
        pages.len()
    };
    let mut first: [Option<usize>; 3] = [None; 3];
    for (pi, p) in pages.iter_mut().enumerate() {
        for s in 0..p.segs.len() {
            if let Some(k) = key(p, s)
                && count(&k) >= need
            {
                p.furniture[s] = k.0;
                first[k.0 as usize].get_or_insert(pi);
            }
        }
    }
    let pick = |kind: u8| {
        first[kind as usize].map(|pi| {
            let ids = (0..pages[pi].segs.len())
                .filter(|&s| pages[pi].furniture[s] == kind)
                .collect();
            (pi, ids)
        })
    };
    (pick(1), pick(2))
}

/// Header or footer paragraphs from the template page; page numbers become fields.
fn furniture_block(
    p: &Prepared,
    fonts: &[FontInfo],
    ids: &[usize],
    (ml, mr): (f32, f32),
    header: bool,
    (index, total): (usize, usize),
) -> Furniture {
    let cx = Ctx {
        g: &p.glyphs,
        fonts,
        segs: &p.segs,
    };
    let lines = text::lines(&p.segs, ids);
    let mut paras = paras::paragraphs(&cx, lines, (ml, p.w - mr), p.w - mr);
    for para in &mut paras {
        for line in &mut para.lines {
            *line = fields(std::mem::take(line), index + 1, total);
        }
    }
    let offset = if header {
        paras.first().map_or(36.0, |q| q.top().max(0.0))
    } else {
        paras.last().map_or(36.0, |q| (p.h - q.bottom()).max(0.0))
    };
    Furniture { paras, offset }
}

/// Replace the page number (and the page count) in a template line with fields.
fn fields(line: Vec<Piece>, page: usize, total: usize) -> Vec<Piece> {
    let mut out = Vec::with_capacity(line.len());
    for piece in line {
        let Piece::Run(r) = piece else {
            out.push(piece);
            continue;
        };
        let mut rest = r.text.as_str();
        let mut buf = String::new();
        let mut changed = false;
        while let Some(start) = rest.find(|c: char| c.is_ascii_digit()) {
            let len = rest[start..]
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len() - start);
            let num: usize = rest[start..start + len].parse().unwrap_or(0);
            let field = if num == page {
                Some(true)
            } else if num == total && total != page {
                Some(false)
            } else {
                None
            };
            buf.push_str(&rest[..start]);
            if let Some(is_page) = field {
                if !buf.is_empty() {
                    out.push(Piece::Run(Run {
                        text: std::mem::take(&mut buf),
                        ..r.clone()
                    }));
                }
                let f = Run {
                    text: rest[start..start + len].to_string(),
                    ..r.clone()
                };
                out.push(if is_page {
                    Piece::PageNumber(f)
                } else {
                    Piece::PageCount(f)
                });
                changed = true;
            } else {
                buf.push_str(&rest[start..start + len]);
            }
            rest = &rest[start + len..];
        }
        buf.push_str(rest);
        if !changed {
            out.push(Piece::Run(r));
        } else if !buf.is_empty() {
            out.push(Piece::Run(Run { text: buf, ..r }));
        }
    }
    out
}

enum Kind {
    Seg(usize),
    Table(usize),
    Picture(usize),
}

fn layout_page(p: &Prepared, fonts: &[FontInfo], ml: f32, mr: f32) -> Page {
    let mut atoms = Vec::new();
    let mut kinds = Vec::new();
    for (s, seg) in p.segs.iter().enumerate() {
        if p.furniture[s] == 0 {
            atoms.push(Atom {
                bbox: seg.bbox,
                row: Some(seg.row),
            });
            kinds.push(Kind::Seg(s));
        }
    }
    for (t, (g, _)) in p.tables.iter().enumerate() {
        atoms.push(Atom {
            bbox: g.bbox(),
            row: None,
        });
        kinds.push(Kind::Table(t));
    }
    for (k, (b, _)) in p.pictures.iter().enumerate() {
        atoms.push(Atom {
            bbox: *b,
            row: None,
        });
        kinds.push(Kind::Picture(k));
    }
    let mut page = Page {
        w: p.w,
        h: p.h,
        margins: Margins {
            top: 36.0,
            right: mr,
            bottom: 36.0,
            left: ml,
        },
        bands: Vec::new(),
        floats: p.floats.clone(),
        header: false,
        footer: false,
    };
    if atoms.is_empty() {
        return page;
    }
    let mut sizes: Vec<f32> = p.segs.iter().map(|s| s.size).collect();
    sizes.sort_by(f32::total_cmp);
    let median = sizes.get(sizes.len() / 2).copied().unwrap_or(10.0);
    // Bands are separated by gaps clearly larger than the page's normal line spacing, so
    // loosely set text (1.5–2× leading) isn't cut into one band per line.
    let leading = typical_leading(p).unwrap_or(1.2 * median);
    let line_gap = (leading - median) + 0.4 * median;
    let ids: Vec<usize> = (0..atoms.len()).collect();
    let node = order::cut(&atoms, ids, line_gap.max(0.6 * median).max(4.0));
    let cx = Ctx {
        g: &p.glyphs,
        fonts,
        segs: &p.segs,
    };
    let (left, right) = (ml, p.w - mr);
    for spec in order::bands(&atoms, node) {
        let n = spec.cols.len();
        let exts: Vec<BBox> = spec
            .cols
            .iter()
            .map(|groups| {
                groups
                    .iter()
                    .flatten()
                    .fold(BBox::EMPTY, |b, &a| b.union(atoms[a].bbox))
            })
            .collect();
        let mut cols = Vec::with_capacity(n);
        for (ci, groups) in spec.cols.iter().enumerate() {
            let ext = exts[ci];
            let x0 = if ci == 0 { left } else { ext.x0 };
            let x1 = if ci + 1 == n { right } else { ext.x1 };
            // Text in a column could have run up to just before the next column.
            let avail = exts
                .get(ci + 1)
                .map_or(right, |next| (next.x0 - 6.0).max(x1));
            // Column alignments over the whole column (all its text lines).
            let seg_ids: Vec<usize> = groups
                .iter()
                .flatten()
                .filter_map(|&a| match kinds[a] {
                    Kind::Seg(s) => Some(s),
                    _ => None,
                })
                .collect();
            let aligned = text::aligned(&p.segs, &text::lines(&p.segs, &seg_ids));
            let mut blocks = Vec::new();
            for g in groups {
                group_blocks(
                    &cx,
                    p,
                    fonts,
                    &atoms,
                    &kinds,
                    g,
                    (x0, x1),
                    avail,
                    &aligned,
                    &mut blocks,
                );
            }
            cols.push(Column { x0, x1, blocks });
        }
        if cols.iter().any(|c| !c.blocks.is_empty()) {
            page.bands.push(Band { cols });
        }
    }
    let top = page.bands.first().map_or(36.0, Band::top);
    let bottom = page
        .bands
        .iter()
        .map(Band::bottom)
        .fold(f32::NEG_INFINITY, f32::max);
    page.margins.top = top.clamp(0.0, p.h / 2.0);
    // A little below the content, never more than half an inch: room for small metric
    // differences without a stray line spilling onto a new page.
    page.margins.bottom = ((p.h - bottom).clamp(0.0, 36.0) - 2.0).max(0.0);
    page
}

/// The page's usual baseline-to-baseline distance between consecutive lines of one size
/// that overlap horizontally.
fn typical_leading(p: &Prepared) -> Option<f32> {
    let mut order: Vec<usize> = (0..p.segs.len()).collect();
    order.sort_by(|&a, &b| p.segs[a].base.total_cmp(&p.segs[b].base));
    let mut pitches = Vec::new();
    for (k, &a) in order.iter().enumerate() {
        let sa = &p.segs[a];
        // The next segment below that overlaps this one horizontally.
        let below = order[k + 1..].iter().map(|&b| &p.segs[b]).find(|sb| {
            sb.base > sa.base + 0.5 * sa.size && sb.bbox.x0 < sa.bbox.x1 && sb.bbox.x1 > sa.bbox.x0
        });
        if let Some(sb) = below
            && (sb.size - sa.size).abs() < 0.1 * sa.size
        {
            let d = sb.base - sa.base;
            if d < 2.5 * sa.size {
                pitches.push(d);
            }
        }
    }
    if pitches.len() < 3 {
        return None;
    }
    pitches.sort_by(f32::total_cmp);
    Some(pitches[pitches.len() / 2])
}

#[allow(clippy::too_many_arguments)]
fn group_blocks(
    cx: &Ctx<'_>,
    p: &Prepared,
    fonts: &[FontInfo],
    atoms: &[Atom],
    kinds: &[Kind],
    group: &[usize],
    col: (f32, f32),
    avail: f32,
    aligned: &text::Aligned,
    out: &mut Vec<Block>,
) {
    let mut sorted = group.to_vec();
    sorted.sort_by(|&a, &b| atoms[a].bbox.y0.total_cmp(&atoms[b].bbox.y0));
    let mut text_ids: Vec<usize> = Vec::new();
    let flush = |ids: &mut Vec<usize>, out: &mut Vec<Block>| {
        if ids.is_empty() {
            return;
        }
        let lines = text::lines(&p.segs, ids);
        out.extend(
            paras::paragraphs_with(cx, lines, col, avail, aligned)
                .into_iter()
                .map(Block::Para),
        );
        ids.clear();
    };
    for a in sorted {
        match kinds[a] {
            Kind::Seg(s) => text_ids.push(s),
            Kind::Table(t) => {
                flush(&mut text_ids, out);
                let (g, cells) = &p.tables[t];
                out.push(Block::Table(table_block(p, fonts, g, cells)));
            }
            Kind::Picture(k) => {
                flush(&mut text_ids, out);
                let (bbox, media) = p.pictures[k];
                out.push(Block::Picture(Picture { media, bbox }));
            }
        }
    }
    flush(&mut text_ids, out);
}

fn table_block(p: &Prepared, fonts: &[FontInfo], g: &Grid, cells: &[Vec<usize>]) -> Table {
    let nc = g.xs.len() - 1;
    let nr = g.ys.len() - 1;
    // Horizontal inset: the typical gap between a cell's edge and its text.
    let mut insets: Vec<f32> = Vec::new();
    for (k, c) in g.cells.iter().enumerate() {
        let x0 = cells[k]
            .iter()
            .filter(|&&i| p.glyphs[i].ch != ' ')
            .map(|&i| p.glyphs[i].x0)
            .fold(f32::INFINITY, f32::min);
        if x0.is_finite() {
            insets.push(x0 - g.xs[c.c0]);
        }
    }
    insets.sort_by(f32::total_cmp);
    let pad = insets
        .get(insets.len() / 2)
        .copied()
        .unwrap_or(4.0)
        .clamp(1.0, 8.0);
    let mut built: Vec<Cell> = Vec::with_capacity(g.cells.len());
    for (k, c) in g.cells.iter().enumerate() {
        let bb = c.bbox(g);
        let ids = &cells[k];
        let rows = text::rows(&p.glyphs, ids);
        let segs = text::segments(&p.glyphs, &rows);
        let all: Vec<usize> = (0..segs.len()).collect();
        let lines = text::lines(&segs, &all);
        let cx = Ctx {
            g: &p.glyphs,
            fonts,
            segs: &segs,
        };
        let paras = paras::paragraphs(&cx, lines, (bb.x0 + pad, bb.x1 - pad), bb.x1 - pad);
        let (mut valign, mut pad_top) = (VAlign::Top, 0.0);
        if let (Some(f), Some(l)) = (paras.first(), paras.last()) {
            let st = f.top() - bb.y0;
            let sb = bb.y1 - l.bottom();
            if (st - sb).abs() < 2.0 && st > 1.0 {
                valign = VAlign::Center;
            } else if sb < st - 2.0 && st > 4.0 {
                valign = VAlign::Bottom;
            } else {
                pad_top = st.max(0.0);
            }
        }
        built.push(Cell {
            span: c.c1 - c.c0,
            vmerge: if c.r1 - c.r0 > 1 {
                VMerge::Start
            } else {
                VMerge::None
            },
            borders: c.borders,
            fill: c.fill,
            valign,
            pad_top,
            paras,
        });
    }
    let mut rows = Vec::with_capacity(nr);
    for r in 0..nr {
        let mut cells_out = Vec::new();
        let mut c = 0;
        while c < nc {
            let Some(k) = g
                .cells
                .iter()
                .position(|k| (k.r0..k.r1).contains(&r) && (k.c0..k.c1).contains(&c))
            else {
                c += 1;
                continue;
            };
            let gc = &g.cells[k];
            if gc.r0 == r {
                cells_out.push(built[k].clone());
            } else {
                cells_out.push(Cell {
                    span: gc.c1 - gc.c0,
                    vmerge: VMerge::Continue,
                    borders: gc.borders,
                    fill: gc.fill,
                    ..Default::default()
                });
            }
            c = gc.c1;
        }
        rows.push(Row {
            height: g.ys[r + 1] - g.ys[r],
            cells: cells_out,
        });
    }
    Table {
        bbox: g.bbox(),
        cols: g.xs.windows(2).map(|w| w[1] - w[0]).collect(),
        rows,
        pad,
    }
}

fn each_para(l: &mut Layout, mut f: impl FnMut(&mut Para)) {
    fn blocks(b: &mut [Block], f: &mut dyn FnMut(&mut Para)) {
        for b in b {
            match b {
                Block::Para(p) => f(p),
                Block::Table(t) => {
                    for r in &mut t.rows {
                        for c in &mut r.cells {
                            for p in &mut c.paras {
                                f(p);
                            }
                        }
                    }
                }
                Block::Picture(_) => {}
            }
        }
    }
    for page in &mut l.pages {
        for band in &mut page.bands {
            for col in &mut band.cols {
                blocks(&mut col.blocks, &mut f);
            }
        }
    }
}

/// Document-level passes: body font and size, headings, title.
fn finish(l: &mut Layout) {
    let mut fonts: HashMap<(u16, i32), usize> = HashMap::new();
    each_para(l, |p| {
        for line in &p.lines {
            for piece in line {
                if let Piece::Run(r) = piece {
                    let n = r.text.chars().filter(|c| !c.is_whitespace()).count();
                    *fonts.entry((r.font, (r.size * 2.0) as i32)).or_default() += n;
                }
            }
        }
    });
    let ((font, half), _) = fonts
        .iter()
        .max_by_key(|(k, v)| (**v, std::cmp::Reverse(k.1)))
        .map(|(k, v)| (*k, *v))
        .unwrap_or(((0, 22), 0));
    l.body_font = font;
    l.body_size = half as f32 / 2.0;
    let body = l.body_size;
    // Headings: larger than body, short, not sentences. Levels by size, largest first.
    let mut sizes: Vec<i32> = Vec::new();
    each_para(l, |p| {
        if is_heading(p, body) {
            sizes.push((p.size * 2.0) as i32);
        }
    });
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    sizes.truncate(3);
    let mut title = None;
    each_para(l, |p| {
        if is_heading(p, body)
            && let Some(level) = sizes.iter().position(|&s| s == (p.size * 2.0) as i32)
        {
            p.heading = Some(level as u8 + 1);
            if level == 0 && title.is_none() {
                title = Some(p.text().trim().to_string());
            }
        }
    });
    l.title = title.filter(|t| !t.is_empty() && t.chars().count() <= 120);
}

fn is_heading(p: &Para, body: f32) -> bool {
    let t = p.text();
    let t = t.trim();
    p.size >= 1.15 * body
        && p.lines.len() <= 3
        && !t.is_empty()
        && t.chars().count() <= 160
        && !t.ends_with(['.', ',', ';'])
        && p.bullet.is_none()
}
