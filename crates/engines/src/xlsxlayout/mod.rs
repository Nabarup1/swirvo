//! XLSX → PDF without Excel or LibreOffice (docs/16 §9): every visible sheet printed the
//! way Excel prints it. That means the print area (or the used range), page setup
//! (paper, orientation, margins, scale, fit to pages), repeated title rows and columns,
//! manual page breaks, page order, cell styles (fonts, fills, borders, alignment,
//! wrapping, number formats), merged cells, text overflowing into empty neighbours,
//! gridlines when asked for, pictures, links, and headers and footers with their codes.

pub mod numfmt;
mod read;

use std::path::Path;
use std::sync::Arc;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, PdfOptions, StepRequest};

use crate::docxlayout::fonts::Fonts;
use crate::docxlayout::layout::{Boxes, Op, PageOut};
use crate::docxlayout::model::{
    Block, Document, Item, Link, Para, ParaStyle, RunStyle, SectPr, Section,
};
use crate::docxlayout::props::{Align, BorderStyle, LineRule, Rgb};
use crate::docxlayout::tags::{FOOTER, HEADER, Role, Tags};
use read::{Book, Cell, Drawn, Font, HAlign, Sheet, VAlign, Value};

/// Space between a cell's edge and its text (Excel's ~2 px).
const PAD: f32 = 2.0;

/// Convert XLSX bytes to a PDF at `out`; returns the page count.
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
        let book = read::read(bytes)?;
        cancel.check()?;
        if !Fonts::available() {
            return Err(EngineError::Encode(
                "no fonts are installed, so the PDF can't be typeset".into(),
            ));
        }
        let (pages, mut fonts, tags) = layout(
            &book,
            Fonts::new(Default::default()),
            title,
            opts.tagged || opts.accessible,
            cancel,
        )?;
        if pages.is_empty() {
            return Err(EngineError::Invalid(
                "this workbook has nothing to print (all sheets are empty)".into(),
            ));
        }
        let name = format!("{title}.xlsx");
        crate::docxlayout::render::render_from(
            &pages,
            &mut fonts,
            &book.meta,
            title,
            out,
            opts,
            tags.as_ref(),
            Some(crate::docxlayout::render::Source {
                name: &name,
                kind: "xlsx",
                bytes,
            }),
        )?;
        Ok(pages.len())
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this workbook".into()))?
}

/// The first printed page as a one-page PDF in memory (thumbnails).
pub fn first_page(bytes: &[u8], cancel: &CancelToken) -> Result<Vec<u8>, EngineError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let book = read::read(bytes)?;
        cancel.check()?;
        if !Fonts::available() {
            return Err(EngineError::Encode("no fonts".into()));
        }
        let (pages, mut fonts, _) =
            layout(&book, Fonts::new(Default::default()), "", false, cancel)?;
        if pages.is_empty() {
            return Err(EngineError::Invalid("nothing to print".into()));
        }
        crate::docxlayout::render::render_bytes(&pages[..1], &mut fonts, &book.meta, "")
    }))
    .map_err(|_| EngineError::Encode("couldn't lay out this workbook".into()))?
}

fn para(text: &str, rs: Arc<RunStyle>, align: Align) -> Block {
    Block::Para(Box::new(Para {
        p: ParaStyle {
            align,
            left: 0.0,
            right: 0.0,
            first: 0.0,
            before: 0.0,
            after: 0.0,
            line: LineRule::Auto(1.0),
            contextual: false,
            keep_next: false,
            keep_lines: false,
            page_break_before: false,
            widow: false,
            tabs: Vec::new(),
            shd: None,
            borders: [None; 5],
            bidi: false,
            style: String::new(),
        },
        mark: rs.clone(),
        items: text
            .split('\n')
            .enumerate()
            .flat_map(|(i, line)| {
                let mut v = Vec::new();
                if i > 0 {
                    v.push(Item::Break(crate::docxlayout::model::BreakKind::Line));
                }
                if !line.is_empty() {
                    v.push(Item::Text {
                        text: line.to_string(),
                        rs: rs.clone(),
                        link: None,
                    });
                }
                v
            })
            .collect(),
        label: None,
        outline: None,
    }))
}

fn run_style(f: &Font, color: Option<Rgb>) -> RunStyle {
    RunStyle {
        fonts: [
            f.name.clone(),
            f.name.clone(),
            f.name.clone(),
            f.name.clone(),
        ],
        b: f.b,
        i: f.i,
        b_cs: f.b,
        i_cs: f.i,
        sz: f.sz.clamp(1.0, 409.0),
        sz_cs: f.sz.clamp(1.0, 409.0),
        color: color.or(f.color).unwrap_or(Rgb(0, 0, 0)),
        auto_color: false,
        u: f.u,
        u_color: None,
        strike: f.strike,
        dstrike: false,
        caps: false,
        small_caps: false,
        vanish: false,
        vert: f.vert,
        position: 0.0,
        spacing: 0.0,
        scale: 1.0,
        kern: None,
        highlight: None,
        shd: None,
        bdr: None,
        rtl: false,
        cs: false,
    }
}

/// What one sheet prints: its pages as (rows, cols) ranges, and the geometry.
struct Plan<'a> {
    sheet: &'a Sheet,
    col_w: Vec<f32>,
    row_h: Vec<f32>,
    /// First row and column of the area (indexes into `row_h`/`col_w` are offsets).
    r0: u32,
    c0: u32,
    scale: f32,
    paper: (f32, f32),
    /// (rows first..last, cols first..last), inclusive, in sheet coordinates.
    pages: Vec<((u32, u32), (u32, u32))>,
}

impl Plan<'_> {
    fn cw(&self, c: u32) -> f32 {
        self.col_w
            .get((c.wrapping_sub(self.c0)) as usize)
            .copied()
            .unwrap_or_else(|| {
                self.sheet
                    .col_w
                    .get(&c)
                    .map_or(self.sheet.default_col_w, |(w, h)| if *h { 0.0 } else { *w })
            })
    }

    fn rh(&self, r: u32) -> f32 {
        self.row_h
            .get((r.wrapping_sub(self.r0)) as usize)
            .copied()
            .unwrap_or_else(|| {
                self.sheet
                    .row_h
                    .get(&r)
                    .map_or(self.sheet.default_row_h, |(h, hid)| {
                        if *hid {
                            0.0
                        } else {
                            h.unwrap_or(self.sheet.default_row_h)
                        }
                    })
            })
    }
}

/// Split `[a, b]` into runs whose sizes fit `room`, also breaking after `breaks`.
fn blocks(a: u32, b: u32, size: impl Fn(u32) -> f32, room: f32, breaks: &[u32]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut start = a;
    let mut acc = 0.0;
    for i in a..=b {
        let s = size(i);
        if i > start && (acc + s > room + 0.01 || breaks.contains(&(i - 1))) {
            out.push((start, i - 1));
            start = i;
            acc = 0.0;
        }
        acc += s;
    }
    out.push((start, b));
    out
}

fn plan<'a>(book: &Book, sheet: &'a Sheet, boxes: &mut Boxes<'_>) -> Option<Plan<'a>> {
    let [r0, c0, r1, c1] = match sheet.print_area {
        Some(a) => a,
        None => {
            let (r0, c0, r1, c1) = sheet.used?;
            // Excel starts printing at A1's corner of the used range's top-left.
            [r0, c0, r1, c1]
        }
    };
    let (r1, c1) = (r1.min(r0 + 100_000), c1.min(c0 + 2_000));
    let st = sheet.setup;
    let paper = if st.landscape {
        (st.paper.0.max(st.paper.1), st.paper.0.min(st.paper.1))
    } else {
        (st.paper.0.min(st.paper.1), st.paper.0.max(st.paper.1))
    };
    let mut p = Plan {
        sheet,
        col_w: Vec::new(),
        row_h: Vec::new(),
        r0,
        c0,
        scale: st.scale / 100.0,
        paper,
        pages: Vec::new(),
    };
    p.col_w = (c0..=c1).map(|c| p.cw(c)).collect();
    // Rows without a stored height grow to fit wrapped text and large fonts.
    let mut heights = Vec::with_capacity((r1 - r0 + 1) as usize);
    for r in r0..=r1 {
        let stored = sheet.row_h.get(&r);
        let h = match stored {
            Some((_, true)) => 0.0,
            Some((Some(h), false)) => *h,
            _ => {
                let mut h = sheet.default_row_h;
                for c in c0..=c1 {
                    let Some(cell) = sheet.cells.get(&(r, c)) else {
                        continue;
                    };
                    if cell.v == Value::Empty {
                        continue;
                    }
                    let xf = book.styles.xf(cell.xf);
                    let f = book.styles.font(xf.font);
                    if xf.wrap {
                        let (text, color) = display(book, cell);
                        let rs = Arc::new(run_style(&f, color));
                        let w = (p.col_w[(c - c0) as usize] - 2.0 * PAD).max(4.0);
                        let (_, th, _) = boxes.lay(&[para(&text, rs, Align::Left)], w);
                        h = h.max(th + 2.0);
                    } else {
                        // Excel's default row fits the default font; bigger fonts grow it.
                        let base = book.styles.font(0).sz.max(1.0);
                        h = h.max(sheet.default_row_h * f.sz / base);
                    }
                }
                h
            }
        };
        heights.push(h);
    }
    p.row_h = heights;
    let m = st.margins;
    let avail_w = paper.0 - m[0] - m[1];
    let avail_h = paper.1 - m[2] - m[3];
    let titles_w: f32 = sheet
        .title_cols
        .map_or(0.0, |(a, b)| (a..=b).map(|c| p.cw(c)).sum());
    let titles_h: f32 = sheet
        .title_rows
        .map_or(0.0, |(a, b)| (a..=b).map(|r| p.rh(r)).sum());
    if let Some((fw, fh)) = st.fit {
        let total_w: f32 = p.col_w.iter().sum::<f32>().max(1.0);
        let total_h: f32 = p.row_h.iter().sum::<f32>().max(1.0);
        let sw = if fw > 0 {
            avail_w * fw as f32 / total_w
        } else {
            f32::MAX
        };
        let sh = if fh > 0 {
            avail_h * fh as f32 / total_h
        } else {
            f32::MAX
        };
        p.scale = sw.min(sh).clamp(0.1, 1.0);
    }
    let room_w = (avail_w / p.scale - titles_w).max(20.0);
    let room_h = (avail_h / p.scale - titles_h).max(20.0);
    let cols = blocks(c0, c1, |c| p.cw(c), room_w, &sheet.col_breaks);
    let rows = blocks(r0, r1, |r| p.rh(r), room_h, &sheet.row_breaks);
    if st.over_then_down {
        for rb in &rows {
            for cb in &cols {
                p.pages.push((*rb, *cb));
            }
        }
    } else {
        for cb in &cols {
            for rb in &rows {
                p.pages.push((*rb, *cb));
            }
        }
    }
    Some(p)
}

/// A cell's text and the colour its number format asks for.
fn display(book: &Book, cell: &Cell) -> (String, Option<Rgb>) {
    let xf = book.styles.xf(cell.xf);
    let code = book.styles.fmt(xf.num_fmt);
    match &cell.v {
        Value::Num(n) => numfmt::format(*n, code, book.date1904),
        Value::Str(r) => (numfmt::format_text(&r.plain(), code), None),
        Value::Bool(b) => ((if *b { "TRUE" } else { "FALSE" }).into(), None),
        Value::Err(e) => (e.clone(), None),
        Value::Empty => (String::new(), None),
    }
}

fn layout(
    book: &Book,
    fonts: Fonts,
    file: &str,
    tagged: bool,
    cancel: &CancelToken,
) -> Result<(Vec<PageOut>, Fonts, Option<Tags>), EngineError> {
    let doc = Document {
        sections: vec![Section {
            sect: SectPr::default(),
            blocks: Vec::new(),
        }],
        default_tab: 36.0,
        compat15: true,
        ..Document::default()
    };
    let mut boxes = Boxes::new(&doc, fonts);
    if tagged {
        boxes.enable_tags();
    }
    let plans: Vec<Plan<'_>> = book
        .sheets
        .iter()
        .filter_map(|s| plan(book, s, &mut boxes))
        .collect();
    let total: usize = plans.iter().map(|p| p.pages.len()).sum();
    let mut pages = Vec::with_capacity(total);
    for p in &plans {
        let first = p.sheet.setup.first_page.unwrap_or(pages.len() as u32 + 1);
        for (number, (pi, &(rows, cols))) in (first..).zip(p.pages.iter().enumerate()) {
            cancel.check()?;
            let mut page = draw_page(book, p, rows, cols, &mut boxes);
            if pi == 0 {
                page.heads.push((0, p.sheet.name.clone(), 0.0));
            }
            header_footer(p, &mut page, &mut boxes, number, total, file);
            pages.push(page);
        }
    }
    let (fonts, tags) = boxes.into_parts();
    Ok((pages, fonts, tags))
}

fn line_op(x1: f32, y1: f32, x2: f32, y2: f32, b: &read::Edge) -> Op {
    Op::Line {
        x1,
        y1,
        x2,
        y2,
        width: b.width,
        color: b.color,
        style: b.style,
    }
}

/// The merged range whose top-left is (r, c), and whether (r, c) is inside another one.
fn merge_at(sheet: &Sheet, r: u32, c: u32) -> (Option<[u32; 4]>, bool) {
    for m in &sheet.merges {
        if m[0] == r && m[1] == c {
            return (Some(*m), false);
        }
        if r >= m[0] && r <= m[2] && c >= m[1] && c <= m[3] {
            return (None, true);
        }
    }
    (None, false)
}

fn draw_page(
    book: &Book,
    p: &Plan<'_>,
    rows: (u32, u32),
    cols: (u32, u32),
    boxes: &mut Boxes<'_>,
) -> PageOut {
    let sheet = p.sheet;
    let st = sheet.setup;
    let mut page = PageOut {
        w: p.paper.0,
        h: p.paper.1,
        ops: Vec::new(),
        links: Vec::new(),
        dests: Vec::new(),
        heads: Vec::new(),
    };
    // Rows and columns on this page: repeated titles first (unless already in view).
    let mut rlist: Vec<u32> = Vec::new();
    if let Some((a, b)) = sheet.title_rows
        && rows.0 > b
    {
        rlist.extend(a..=b);
    }
    rlist.extend(rows.0..=rows.1);
    let mut clist: Vec<u32> = Vec::new();
    if let Some((a, b)) = sheet.title_cols
        && cols.0 > b
    {
        clist.extend(a..=b);
    }
    clist.extend(cols.0..=cols.1);
    let mut xs = Vec::with_capacity(clist.len() + 1);
    let mut x = 0.0;
    for &c in &clist {
        xs.push(x);
        x += p.cw(c);
    }
    xs.push(x);
    let mut ys = Vec::with_capacity(rlist.len() + 1);
    let mut y = 0.0;
    for &r in &rlist {
        ys.push(y);
        y += p.rh(r);
    }
    ys.push(y);
    let (bw, bh) = (x, y);
    let m = st.margins;
    let mut ox = m[0];
    let mut oy = m[2];
    if st.center_h {
        ox += ((p.paper.0 - m[0] - m[1]) - bw * p.scale).max(0.0) / 2.0;
    }
    if st.center_v {
        oy += ((p.paper.1 - m[2] - m[3]) - bh * p.scale).max(0.0) / 2.0;
    }
    page.ops
        .push(Op::Push([p.scale, 0.0, 0.0, p.scale, ox, oy]));
    let col_of = |c: u32| clist.iter().position(|&x| x == c);
    let row_of = |r: u32| rlist.iter().position(|&x| x == r);
    // The rectangle of a cell or of the merged range starting at it.
    let rect = |ri: usize, ci: usize, merge: Option<[u32; 4]>| -> [f32; 4] {
        let (mut x1, mut y1) = (xs[ci + 1], ys[ri + 1]);
        if let Some(mr) = merge {
            if let Some(last) = col_of(mr[3])
                .or_else(|| (mr[3] > *clist.last().unwrap_or(&0)).then(|| clist.len() - 1))
            {
                x1 = xs[last + 1];
            }
            if let Some(last) = row_of(mr[2])
                .or_else(|| (mr[2] > *rlist.last().unwrap_or(&0)).then(|| rlist.len() - 1))
            {
                y1 = ys[last + 1];
            }
        }
        [xs[ci], ys[ri], x1, y1]
    };
    // 1. Fills.
    let mut texts = Vec::new();
    for (ri, &r) in rlist.iter().enumerate() {
        if ys[ri + 1] - ys[ri] <= 0.0 {
            continue;
        }
        for (ci, &c) in clist.iter().enumerate() {
            if xs[ci + 1] - xs[ci] <= 0.0 {
                continue;
            }
            let (merge, inside) = merge_at(sheet, r, c);
            if inside {
                continue;
            }
            let Some(cell) = sheet.cells.get(&(r, c)) else {
                continue;
            };
            let xf = book.styles.xf(cell.xf);
            let [x0, y0, x1, y1] = rect(ri, ci, merge);
            if let Some(fill) = book.styles.fills.get(xf.fill).copied().flatten() {
                page.ops.push(Op::Rect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                    fill,
                });
            }
            if cell.v != Value::Empty {
                texts.push((ri, ci, r, c, merge));
            }
        }
    }
    // 2. Gridlines, when the sheet prints them.
    if st.grid {
        let g = read::Edge {
            style: BorderStyle::Single,
            width: 0.25,
            color: Rgb(192, 192, 192),
        };
        for &yy in &ys {
            page.ops.push(line_op(0.0, yy, bw, yy, &g));
        }
        for &xx in &xs {
            page.ops.push(line_op(xx, 0.0, xx, bh, &g));
        }
    }
    // 3. Borders (edges of merged ranges come from their top-left cell).
    for (ri, &r) in rlist.iter().enumerate() {
        for (ci, &c) in clist.iter().enumerate() {
            let (merge, inside) = merge_at(sheet, r, c);
            if inside {
                continue;
            }
            let Some(cell) = sheet.cells.get(&(r, c)) else {
                continue;
            };
            let xf = book.styles.xf(cell.xf);
            let Some(b) = book.styles.borders.get(xf.border) else {
                continue;
            };
            let [x0, y0, x1, y1] = rect(ri, ci, merge);
            if let Some(e) = &b.e[0] {
                page.ops.push(line_op(x0, y0, x0, y1, e));
            }
            if let Some(e) = &b.e[1] {
                page.ops.push(line_op(x1, y0, x1, y1, e));
            }
            if let Some(e) = &b.e[2] {
                page.ops.push(line_op(x0, y0, x1, y0, e));
            }
            if let Some(e) = &b.e[3] {
                page.ops.push(line_op(x0, y1, x1, y1, e));
            }
        }
    }
    // 4. Text, clipped to its cell or to the empty cells it spills into.
    for (ri, ci, r, c, merge) in texts {
        let Some(cell) = sheet.cells.get(&(r, c)) else {
            continue;
        };
        let xf = book.styles.xf(cell.xf);
        let font = book.styles.font(xf.font);
        let (text, fmt_color) = display(book, cell);
        if text.is_empty() {
            continue;
        }
        let numeric = matches!(cell.v, Value::Num(_));
        let align = match xf.h {
            HAlign::Left | HAlign::Fill | HAlign::Justify => Align::Left,
            HAlign::Center | HAlign::CenterAcross => Align::Center,
            HAlign::Right => Align::Right,
            HAlign::General => match cell.v {
                Value::Num(_) => Align::Right,
                Value::Bool(_) | Value::Err(_) => Align::Center,
                _ => Align::Left,
            },
        };
        let [x0, y0, x1, y1] = rect(ri, ci, merge);
        let indent = xf.indent as f32 * 7.5;
        // Wrapped text is aligned by the layout within the cell's width; unwrapped text
        // is measured left-aligned and placed here.
        let lay_align = if xf.wrap { align } else { Align::Left };
        // Rich text keeps its runs' fonts; everything else is one run.
        let block = match &cell.v {
            Value::Str(rich) if rich.runs.len() > 1 || rich.runs.iter().any(|r| r.1.is_some()) => {
                let mut b = para("", Arc::new(run_style(&font, fmt_color)), lay_align);
                if let Block::Para(pp) = &mut b {
                    for (t, f) in &rich.runs {
                        let rs = Arc::new(run_style(f.as_ref().unwrap_or(&font), None));
                        for (i, line) in t.split('\n').enumerate() {
                            if i > 0 {
                                pp.items
                                    .push(Item::Break(crate::docxlayout::model::BreakKind::Line));
                            }
                            if !line.is_empty() {
                                pp.items.push(Item::Text {
                                    text: line.to_string(),
                                    rs: rs.clone(),
                                    link: None,
                                });
                            }
                        }
                    }
                }
                b
            }
            _ => para(&text, Arc::new(run_style(&font, fmt_color)), lay_align),
        };
        let cell_w = x1 - x0;
        let (mut ops, th, width) = if xf.wrap {
            let w = (cell_w - 2.0 * PAD - indent).max(2.0);
            let (ops, th, _) = boxes.lay(&[block], w);
            (ops, th, w)
        } else {
            let (ops, th, _) = boxes.lay(&[block], 100_000.0);
            let tw = ops
                .iter()
                .filter_map(|o| match o {
                    Op::Text { x, seg, .. } => Some(x + seg.width),
                    _ => None,
                })
                .fold(0.0f32, f32::max);
            (ops, th, tw)
        };
        // Numbers that don't fit show as ####, as in Excel.
        if numeric && !xf.wrap && width > cell_w - 2.0 * PAD {
            let hash = para("#", Arc::new(run_style(&font, fmt_color)), Align::Left);
            let (hops, _, _) = boxes.lay(&[hash], 1000.0);
            let hw = hops
                .iter()
                .filter_map(|o| match o {
                    Op::Text { seg, .. } => Some(seg.width),
                    _ => None,
                })
                .fold(0.0f32, f32::max)
                .max(1.0);
            let n = ((cell_w - 2.0 * PAD) / hw).floor().max(1.0) as usize;
            let b = para(
                &"#".repeat(n),
                Arc::new(run_style(&font, fmt_color)),
                Align::Left,
            );
            let (o, _, _) = boxes.lay(&[b], 100_000.0);
            ops = o;
        }
        let tw = if xf.wrap {
            width
        } else {
            ops.iter()
                .filter_map(|o| match o {
                    Op::Text { x, seg, .. } => Some(x + seg.width),
                    _ => None,
                })
                .fold(0.0f32, f32::max)
        };
        // Horizontal position (wrapped text is already aligned within its width).
        let tx = if xf.wrap {
            x0 + PAD + if align == Align::Left { indent } else { 0.0 }
        } else {
            match align {
                Align::Right => x1 - PAD - tw - indent,
                Align::Center => (x0 + x1) / 2.0 - tw / 2.0,
                _ => x0 + PAD + indent,
            }
        };
        let ty = match xf.v {
            VAlign::Top => y0 + 1.0,
            VAlign::Center => (y0 + y1) / 2.0 - th / 2.0,
            VAlign::Bottom => y1 - th - 1.0,
        };
        // Clip: the cell, widened into empty neighbours for overflowing text.
        let (mut cx0, mut cx1) = (x0, x1);
        if !xf.wrap && !numeric && merge.is_none() {
            let empty = |cc: u32| {
                sheet
                    .cells
                    .get(&(r, cc))
                    .is_none_or(|x| x.v == Value::Empty)
                    && !merge_at(sheet, r, cc).1
                    && merge_at(sheet, r, cc).0.is_none()
            };
            if matches!(align, Align::Left | Align::Center) && tx + tw > x1 {
                let mut k = ci + 1;
                while k < clist.len() && empty(clist[k]) && xs[k] < tx + tw {
                    cx1 = xs[k + 1];
                    k += 1;
                }
            }
            if matches!(align, Align::Right | Align::Center) && tx < x0 {
                let mut k = ci;
                while k > 0 && empty(clist[k - 1]) && xs[k] > tx {
                    cx0 = xs[k - 1];
                    k -= 1;
                }
            }
        }
        page.ops.push(Op::Clip([cx0, y0, cx1 - cx0, y1 - y0]));
        page.ops.extend(ops.into_iter().map(|o| o.moved(tx, ty)));
        page.ops.push(Op::Pop);
    }
    // 5. Pictures whose top-left corner is on this page.
    for pic in &sheet.pics {
        let (Some(ci), Some(ri)) = (col_of(pic.from.0), row_of(pic.from.2)) else {
            continue;
        };
        let x0 = xs[ci] + pic.from.1;
        let y0 = ys[ri] + pic.from.3;
        let (x1, y1) = match pic.to {
            Some(t) => {
                let xe = col_of(t.0).map_or(bw, |i| xs[i] + t.1);
                let ye = row_of(t.2).map_or(bh, |i| ys[i] + t.3);
                (xe, ye)
            }
            None => (x0 + pic.ext.0, y0 + pic.ext.1),
        };
        let (w, h) = ((x1 - x0).max(1.0), (y1 - y0).max(1.0));
        match &pic.what {
            Drawn::Picture(media) => page.ops.push(Op::Image {
                media: media.clone(),
                x: x0,
                y: y0,
                w,
                h,
                crop: [0.0; 4],
            }),
            Drawn::Chart(c) => {
                let alt = match c.title.as_deref().map(str::trim) {
                    Some(t) if !t.is_empty() => format!("Chart: {t}"),
                    _ => "Chart".into(),
                };
                let fig = boxes.add_tag(Role::Figure(alt), None);
                if boxes.tagging() {
                    page.ops.push(Op::Mark { node: fig, y: y0 });
                }
                boxes.fix_tag(fig);
                let ops = crate::chart::draw(c, w, h, boxes);
                boxes.unfix_tag();
                page.ops.extend(ops.into_iter().map(|o| o.moved(x0, y0)));
            }
        }
    }
    page.ops.push(Op::Pop);
    // Links, in page space.
    for (range, url) in &sheet.links {
        let (Some(ri), Some(ci)) = (row_of(range[0]), col_of(range[1])) else {
            continue;
        };
        let [x0, y0, x1, y1] = rect(ri, ci, None);
        let k = p.scale;
        page.links.push((
            [ox + x0 * k, oy + y0 * k, ox + x1 * k, oy + y1 * k],
            Arc::new(Link::Url(url.clone())),
        ));
    }
    page
}

/// Header and footer: `&L`/`&C`/`&R` sections with `&P`, `&N`, `&A`, `&F`, `&D`, `&T`.
fn header_footer(
    p: &Plan<'_>,
    page: &mut PageOut,
    boxes: &mut Boxes<'_>,
    n: u32,
    total: usize,
    file: &str,
) {
    let m = p.sheet.setup.margins;
    for (code, top) in [(&p.sheet.header, true), (&p.sheet.footer, false)] {
        let Some(code) = code else { continue };
        let parts = hf_sections(code, n, total, &p.sheet.name, file);
        let w = p.paper.0 - m[0] - m[1];
        for (text, align, bold) in parts {
            if text.trim().is_empty() {
                continue;
            }
            let f = Font {
                b: bold,
                sz: 10.0,
                ..Font::default()
            };
            // Running headers and footers aren't the sheet's content.
            boxes.fix_tag(if top { HEADER } else { FOOTER });
            let (ops, th, _) = boxes.lay(&[para(&text, Arc::new(run_style(&f, None)), align)], w);
            boxes.unfix_tag();
            let y = if top { m[4] } else { p.paper.1 - m[5] - th };
            page.ops.extend(ops.into_iter().map(|o| o.moved(m[0], y)));
        }
    }
}

fn hf_sections(
    code: &str,
    page: u32,
    total: usize,
    sheet: &str,
    file: &str,
) -> Vec<(String, Align, bool)> {
    let mut out: Vec<(String, Align, bool)> = vec![
        (String::new(), Align::Left, false),
        (String::new(), Align::Center, false),
        (String::new(), Align::Right, false),
    ];
    let mut cur = 1;
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '&' {
            out[cur].0.push(c);
            i += 1;
            continue;
        }
        let Some(&n) = chars.get(i + 1) else { break };
        i += 2;
        match n {
            'L' => cur = 0,
            'C' => cur = 1,
            'R' => cur = 2,
            'P' => out[cur].0.push_str(&page.to_string()),
            'N' => out[cur].0.push_str(&total.to_string()),
            'A' => out[cur].0.push_str(sheet),
            'F' => out[cur].0.push_str(file),
            'D' => out[cur].0.push_str(&crate::xlsxlayout::today()),
            'T' => {}
            'B' => out[cur].2 = true,
            '&' => out[cur].0.push('&'),
            '"' => {
                // &"Font,Style": skip the name; note bold.
                let mut name = String::new();
                while i < chars.len() && chars[i] != '"' {
                    name.push(chars[i]);
                    i += 1;
                }
                i += 1;
                if name.to_ascii_lowercase().contains("bold") {
                    out[cur].2 = true;
                }
            }
            d if d.is_ascii_digit() => {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            _ => {}
        }
    }
    out
}

pub(crate) fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as f64;
    let serial = secs / 86_400.0 + 25_569.0;
    numfmt::format(serial, "m/d/yyyy", false).0
}

pub struct XlsxLayoutEngine;

impl Engine for XlsxLayoutEngine {
    fn id(&self) -> &'static str {
        "xlsx-layout"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("xlsx-layout", "xlsx", "pdf")
                .loss(2)
                .time(60, 120)
                .note("prints the sheets: page setup, styles, number formats; charts aren't drawn yet"),
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
