//! Page layout: sections, columns, headers and footers, paragraphs, tables, floating
//! drawings and footnotes → positioned drawing operations per page.
//!
//! Paragraphs are broken into lines one at a time against the space they land in
//! (columns, indents, wrap-around floats), then paginated with Word's rules: space
//! before is dropped at the top of a page reached by overflow, keep-with-next chains
//! and keep-lines move whole paragraphs, widow/orphan control never leaves a single
//! line behind or ahead. Tables repeat their header rows and split tall rows between
//! lines. Everything that isn't the page flow (cells, headers, text boxes, footnotes)
//! is laid out unpaged into a [`Stack`].

use std::collections::HashMap;
use std::sync::Arc;

use super::fonts::Fonts;
use super::model::*;
use super::numbering::{NumFmt, format_number};
use super::props::{Align, Border, BorderStyle, Leader, Rgb, VAlign, VMerge, Width};
use super::tags::{ARTIFACT, FOOTER, HEADER, Numbering, Role, Tags};
use super::text::{self, Breaker, End, Env, Line, Mark, PGlyph, Placed, Seg};

#[derive(Debug, Clone)]
pub enum Op {
    Text {
        x: f32,
        y: f32,
        seg: Seg,
        color: Rgb,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Rgb,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: Rgb,
        style: BorderStyle,
    },
    Image {
        media: Arc<Media>,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        crop: [f32; 4],
    },
    Shape {
        geom: Geom,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<Rgb>,
        line: Option<(Rgb, f32)>,
    },
    /// Any outline with a solid or gradient fill and transparency (slide shapes).
    Figure(Arc<Figure>),
    /// Concatenate a transform (`[a b c d e f]`, page space, y down) until the matching
    /// [`Op::Pop`]: rotated and flipped shapes.
    Push([f32; 6]),
    /// Clip to `[x, y, w, h]` until the matching [`Op::Pop`] (spreadsheet cells).
    Clip([f32; 4]),
    /// Clip to a figure's outline (its fill rule) until the matching [`Op::Pop`].
    ClipPath(Arc<Figure>),
    Pop,
    /// What follows (until the next mark) belongs to structure node `node` (see
    /// `tags`); `y` is the top of that content, so splitting a stack keeps a mark with its
    /// line. Only emitted when the document is laid out for a tagged PDF.
    Mark {
        node: u32,
        y: f32,
    },
}

/// A path drawn at (`x`, `y`); its commands are relative to that point, y down.
#[derive(Debug, Clone)]
pub struct Figure {
    pub x: f32,
    pub y: f32,
    pub path: Vec<PathCmd>,
    pub fill: Option<Paint>,
    pub stroke: Option<Pen>,
    /// Even-odd fill rule (holes in self-overlapping outlines) instead of non-zero.
    pub even_odd: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCmd {
    Move(f32, f32),
    Line(f32, f32),
    Quad(f32, f32, f32, f32),
    Cubic(f32, f32, f32, f32, f32, f32),
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    /// Colour and opacity (0–1).
    Solid(Rgb, f32),
    /// From one point to another (relative to the figure), with (offset, colour, opacity)
    /// stops.
    Linear {
        from: (f32, f32),
        to: (f32, f32),
        stops: Vec<(f32, Rgb, f32)>,
    },
    /// Circles around `center` out to `radius`.
    Radial {
        center: (f32, f32),
        radius: f32,
        stops: Vec<(f32, Rgb, f32)>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pen {
    pub color: Rgb,
    pub alpha: f32,
    pub width: f32,
    /// Dash lengths as multiples of the width; empty = solid.
    pub dash: Vec<f32>,
    pub round: bool,
}

impl Op {
    pub(crate) fn moved(mut self, dx: f32, dy: f32) -> Op {
        match &mut self {
            Op::Text { x, y, .. }
            | Op::Rect { x, y, .. }
            | Op::Image { x, y, .. }
            | Op::Shape { x, y, .. } => {
                *x += dx;
                *y += dy;
            }
            Op::Line { x1, y1, x2, y2, .. } => {
                *x1 += dx;
                *x2 += dx;
                *y1 += dy;
                *y2 += dy;
            }
            Op::Figure(f) | Op::ClipPath(f) => {
                let f = Arc::make_mut(f);
                f.x += dx;
                f.y += dy;
            }
            // Moving what a transform applies to: T(d) · M · T(−d).
            Op::Push(m) => {
                m[4] += dx - (m[0] * dx + m[2] * dy);
                m[5] += dy - (m[1] * dx + m[3] * dy);
            }
            Op::Clip(r) => {
                r[0] += dx;
                r[1] += dy;
            }
            Op::Mark { y, .. } => *y += dy,
            Op::Pop => {}
        }
        self
    }

    fn top(&self) -> f32 {
        match self {
            Op::Text { y, seg, .. } => y - seg.asc,
            Op::Rect { y, .. } | Op::Image { y, .. } | Op::Shape { y, .. } => *y,
            Op::Line { y1, y2, .. } => y1.min(*y2),
            Op::Figure(f) | Op::ClipPath(f) => f.y,
            Op::Push(_) | Op::Pop => 0.0,
            Op::Clip(r) => r[1],
            Op::Mark { y, .. } => *y,
        }
    }
}

pub type LinkBox = ([f32; 4], Arc<Link>);

#[derive(Debug, Default)]
pub struct PageOut {
    pub w: f32,
    pub h: f32,
    pub ops: Vec<Op>,
    pub links: Vec<LinkBox>,
    /// Bookmark name and its y.
    pub dests: Vec<(String, f32)>,
    /// Heading outline: level, text, y.
    pub heads: Vec<(u8, String, f32)>,
}

/// Content laid out without pages, positioned from (0, 0).
#[derive(Debug, Default, Clone)]
struct Stack {
    h: f32,
    ops: Vec<Op>,
    links: Vec<LinkBox>,
    /// y positions between lines where the stack may be split.
    breaks: Vec<f32>,
}

impl Stack {
    /// Cut at the last break at or above `at`: (head, tail shifted to the top).
    fn split(&self, at: f32) -> Option<(Stack, Stack)> {
        let cut = self
            .breaks
            .iter()
            .copied()
            .filter(|b| *b <= at + 0.01 && *b > 0.5)
            .fold(None, |m: Option<f32>, b| Some(m.map_or(b, |m| m.max(b))))?;
        let mut head = Stack::default();
        let mut tail = Stack::default();
        for op in &self.ops {
            if op.top() < cut - 0.01 {
                head.ops.push(op.clone());
            } else {
                tail.ops.push(op.clone().moved(0.0, -cut));
            }
        }
        for (r, l) in &self.links {
            if r[1] < cut {
                head.links.push((*r, l.clone()));
            } else {
                tail.links
                    .push(([r[0], r[1] - cut, r[2], r[3] - cut], l.clone()));
            }
        }
        head.h = cut;
        tail.h = self.h - cut;
        tail.breaks = self
            .breaks
            .iter()
            .filter(|b| **b > cut)
            .map(|b| b - cut)
            .collect();
        head.breaks = self.breaks.iter().copied().filter(|b| *b < cut).collect();
        Some((head, tail))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Top {
    /// The page or column began because the previous one was full.
    Overflow,
    /// A page break, section start or column break.
    Hard,
}

#[derive(Debug, Clone, Copy)]
struct FloatBox {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
    wrap: Wrap,
}

#[derive(Clone)]
struct PageState {
    /// A lower bottom while balancing columns.
    limit: Option<f32>,
    sect: usize,
    number: u32,
    sect_first: bool,
    top: f32,
    bottom: f32,
    cols: Vec<(f32, f32)>,
    col: usize,
    col_top: f32,
    y: f32,
    max_y: f32,
    fresh: bool,
    reason: Top,
    floats: Vec<FloatBox>,
    notes: Vec<Stack>,
    notes_h: f32,
    front: Vec<Op>,
    /// Ops before this index are the header and footer: behind-text floats go here.
    body_start: usize,
    pending_after: f32,
    prev: Option<(String, bool, [Option<Border>; 5])>,
}

/// Where a stack will land, for drawings anchored to the page.
#[derive(Clone, Copy)]
struct Place {
    x: f32,
    y: f32,
    sect: usize,
}

pub struct Layout<'d> {
    doc: &'d Document,
    pub fonts: Fonts,
    notes: HashMap<String, u32>,
    total: u32,
    sect_pages: HashMap<usize, u32>,
    pages: Vec<PageOut>,
    st: PageState,
    endnote_order: Vec<String>,
    note_mark: Option<String>,
    /// Structure for tagged PDF; None when not wanted (no marks are emitted then).
    tags: Option<Tags>,
    /// Open containers (table cells, notes): new nodes go inside the last one.
    tag_parent: Vec<u32>,
    /// The node the lines being emitted belong to.
    tag_cur: u32,
    /// Everything goes into this node (headers, footers, charts) instead of new ones.
    tag_fixed: Option<u32>,
    /// Open lists per container: (left indent, L node, body of its last item).
    lists: HashMap<Option<u32>, Vec<(f32, u32, u32)>>,
}

fn luminance(c: Rgb) -> f32 {
    (0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32) / 255.0
}

fn mirror(a: Align) -> Align {
    match a {
        Align::Left => Align::Right,
        Align::Right => Align::Left,
        o => o,
    }
}

/// A drawing's description for readers who can't see it, or `fallback`.
fn alt_text(d: &Drawing, fallback: &str) -> String {
    d.alt
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn para_text(p: &Para) -> String {
    let mut s = String::new();
    for it in &p.items {
        if let Item::Text { text, .. } = it {
            s.push_str(text);
            if s.len() > 200 {
                break;
            }
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Lay out a document. `NUMPAGES` fields need the page count, so documents that use them
/// are laid out twice.
pub fn layout(doc: &Document, fonts: Fonts) -> (Vec<PageOut>, Fonts) {
    let (pages, fonts, _) = layout_with(doc, fonts, false);
    (pages, fonts)
}

/// [`layout`], also recording the document's structure (for a tagged PDF) when `tagged`.
pub fn layout_with(
    doc: &Document,
    fonts: Fonts,
    tagged: bool,
) -> (Vec<PageOut>, Fonts, Option<Tags>) {
    let mut l = Layout::new(doc, fonts);
    if tagged && !doc.uses_numpages {
        l.tags = Some(Tags::default());
    }
    l.run();
    if doc.uses_numpages {
        let total = l.pages.len() as u32;
        let sect_pages = std::mem::take(&mut l.sect_pages);
        let fonts = l.fonts;
        let mut l2 = Layout::new(doc, fonts);
        l2.total = total;
        l2.sect_pages = sect_pages;
        if tagged {
            l2.tags = Some(Tags::default());
        }
        l2.run();
        return (l2.pages, l2.fonts, l2.tags);
    }
    (l.pages, l.fonts, l.tags)
}

impl<'d> Layout<'d> {
    fn new(doc: &'d Document, fonts: Fonts) -> Layout<'d> {
        let mut notes = HashMap::new();
        let mut endnote_order = Vec::new();
        let mut n = 0;
        let mut seen_end = std::collections::HashSet::new();
        fn walk(blocks: &[Block], f: &mut dyn FnMut(&str, bool)) {
            for b in blocks {
                match b {
                    Block::Para(p) => {
                        for it in &p.items {
                            if let Item::NoteRef { id, endnote, .. } = it {
                                f(id, *endnote);
                            }
                        }
                    }
                    Block::Table(t) => {
                        for r in &t.rows {
                            for c in &r.cells {
                                walk(&c.blocks, f);
                            }
                        }
                    }
                }
            }
        }
        for s in &doc.sections {
            walk(&s.blocks, &mut |id, end| {
                if end {
                    if seen_end.insert(id.to_string()) {
                        endnote_order.push(id.to_string());
                    }
                } else if !notes.contains_key(id) {
                    n += 1;
                    notes.insert(id.to_string(), n);
                }
            });
        }
        for (i, id) in endnote_order.iter().enumerate() {
            notes.insert(format!("e{id}"), i as u32 + 1);
        }
        Layout {
            doc,
            fonts,
            notes,
            total: 0,
            sect_pages: HashMap::new(),
            pages: Vec::new(),
            st: PageState {
                limit: None,
                sect: 0,
                number: 0,
                sect_first: true,
                top: 72.0,
                bottom: 720.0,
                cols: vec![(72.0, 468.0)],
                col: 0,
                col_top: 72.0,
                y: 72.0,
                max_y: 72.0,
                fresh: true,
                reason: Top::Hard,
                floats: Vec::new(),
                notes: Vec::new(),
                notes_h: 0.0,
                front: Vec::new(),
                body_start: 0,
                pending_after: 0.0,
                prev: None,
            },
            endnote_order,
            note_mark: None,
            tags: None,
            tag_parent: Vec::new(),
            tag_cur: ARTIFACT,
            tag_fixed: None,
            lists: HashMap::new(),
        }
    }

    // ------------------------------------------------------------ structure (tagged PDF)

    fn tag_add(&mut self, role: Role, parent: Option<u32>) -> u32 {
        match &mut self.tags {
            Some(t) => t.add(role, parent),
            None => ARTIFACT,
        }
    }

    /// A mark for the current node at `y`, when tagging.
    fn mark(&self, y: f32) -> Option<Op> {
        self.tags.as_ref().map(|_| Op::Mark {
            node: self.tag_cur,
            y,
        })
    }

    /// The node for a paragraph about to be laid out: a heading, a list item's body (lists
    /// open, nest and close by indent), or a plain paragraph.
    fn para_tag(&mut self, p: &Para) -> u32 {
        if self.tags.is_none() {
            return ARTIFACT;
        }
        if let Some(f) = self.tag_fixed {
            return f;
        }
        let parent = self.tag_parent.last().copied();
        let Some(label) = &p.label else {
            self.lists.remove(&parent);
            if let Some(level) = p.outline {
                let text = para_text(p);
                if !text.is_empty() {
                    return self.tag_add(Role::H((level + 1).min(6), text), parent);
                }
            }
            return self.tag_add(Role::P, parent);
        };
        let indent = p.p.left;
        let mut open = self.lists.remove(&parent).unwrap_or_default();
        while open.last().is_some_and(|&(i, _, _)| i > indent + 0.5) {
            open.pop();
        }
        let l = match open.last() {
            Some(&(i, l, _)) if (i - indent).abs() <= 0.5 => l,
            last => {
                // Deeper than the open list: a nested list inside its last item.
                let under = last.map_or(parent, |&(_, _, body)| Some(body));
                let l = self.tag_add(Role::L(Numbering::of_label(&label.text)), under);
                open.push((indent, l, l));
                l
            }
        };
        let li = self.tag_add(Role::LI, Some(l));
        let body = self.tag_add(Role::LBody, Some(li));
        if let Some(top) = open.last_mut() {
            top.2 = body;
        }
        self.lists.insert(parent, open);
        body
    }

    fn sect(&self, i: usize) -> &'d SectPr {
        &self.doc.sections[i.min(self.doc.sections.len() - 1)].sect
    }

    fn run(&mut self) {
        if self.doc.sections.is_empty() {
            return;
        }
        for (si, sec) in self.doc.sections.iter().enumerate() {
            let s = &sec.sect;
            if si == 0 {
                self.new_page(0, Top::Hard, true);
            } else {
                let prev = self.sect(si - 1);
                let same_size = (prev.w - s.w).abs() < 1.0 && (prev.h - s.h).abs() < 1.0;
                match s.kind {
                    SectKind::Continuous if same_size => self.continuous(si),
                    SectKind::NextColumn if same_size && self.st.col + 1 < self.st.cols.len() => {
                        self.st.sect = si;
                        self.next_column(Top::Hard);
                    }
                    SectKind::EvenPage | SectKind::OddPage => {
                        self.new_page(si, Top::Hard, true);
                        let even = self.st.number.is_multiple_of(2);
                        if even != (s.kind == SectKind::EvenPage) {
                            self.new_page(si, Top::Hard, false);
                        }
                    }
                    _ => self.new_page(si, Top::Hard, true),
                }
            }
            let balance = self.st.cols.len() > 1
                && self
                    .doc
                    .sections
                    .get(si + 1)
                    .is_some_and(|n| n.sect.kind == SectKind::Continuous);
            if balance {
                self.flow_balanced(&sec.blocks);
            } else {
                self.flow_blocks(&sec.blocks);
            }
        }
        if !self.endnote_order.is_empty() {
            self.endnotes();
        }
        self.finish_page();
    }

    // ------------------------------------------------------------ balanced columns

    fn snapshot(&self) -> Snapshot {
        let last = self.pages.last();
        Snapshot {
            pages: self.pages.len(),
            lens: last.map_or((0, 0, 0, 0), |p| {
                (p.ops.len(), p.links.len(), p.dests.len(), p.heads.len())
            }),
            st: self.st.clone(),
            sect_pages: self.sect_pages.clone(),
        }
    }

    fn restore(&mut self, s: &Snapshot) {
        self.pages.truncate(s.pages);
        if let Some(p) = self.pages.last_mut() {
            p.ops.truncate(s.lens.0);
            p.links.truncate(s.lens.1);
            p.dests.truncate(s.lens.2);
            p.heads.truncate(s.lens.3);
        }
        self.st = s.st.clone();
        self.sect_pages = s.sect_pages.clone();
    }

    /// Lay out a multi-column section that a continuous break ends, with its columns
    /// balanced (as Word does): the smallest column height that still fits on the page.
    fn flow_balanced(&mut self, blocks: &[Block]) {
        let snap = self.snapshot();
        self.flow_blocks(blocks);
        if self.pages.len() != snap.pages {
            return; // Spills onto another page: Word balances only the last page; keep it simple.
        }
        let top = self.st.col_top;
        let full = self.bottom() - top;
        let (mut lo, mut hi) = (0.0_f32, full);
        for _ in 0..14 {
            if hi - lo < 1.0 {
                break;
            }
            let mid = (lo + hi) / 2.0;
            self.restore(&snap);
            self.st.limit = Some(top + mid);
            self.flow_blocks(blocks);
            if self.pages.len() == snap.pages {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        self.restore(&snap);
        self.st.limit = Some(top + hi + 0.5);
        self.flow_blocks(blocks);
        self.st.limit = None;
        self.st.y = self.st.max_y.max(self.st.y);
    }

    // ------------------------------------------------------------ pages

    fn finish_page(&mut self) {
        let Some(page) = self.pages.last_mut() else {
            return;
        };
        let s = &self.doc.sections[self.st.sect.min(self.doc.sections.len() - 1)].sect;
        // Column separators.
        if s.cols.sep && self.st.cols.len() > 1 {
            for w in self.st.cols.windows(2) {
                let x = (w[0].0 + w[0].1 + w[1].0) / 2.0;
                page.ops.push(Op::Line {
                    x1: x,
                    y1: self.st.col_top,
                    x2: x,
                    y2: self.st.max_y.max(self.st.y),
                    width: 0.5,
                    color: Rgb(0, 0, 0),
                    style: BorderStyle::Single,
                });
            }
        }
        // Footnotes at the bottom of the body.
        if !self.st.notes.is_empty() {
            let x = self.st.cols[0].0;
            let mut y = self.st.bottom - self.st.notes_h;
            page.ops.push(Op::Line {
                x1: x,
                y1: y + 4.0,
                x2: x + 144.0,
                y2: y + 4.0,
                width: 0.5,
                color: Rgb(0, 0, 0),
                style: BorderStyle::Single,
            });
            y += NOTE_GAP;
            for n in std::mem::take(&mut self.st.notes) {
                let h = n.h;
                for op in n.ops {
                    page.ops.push(op.moved(x, y));
                }
                for (r, l) in n.links {
                    page.links
                        .push(([r[0] + x, r[1] + y, r[2] + x, r[3] + y], l));
                }
                y += h;
            }
        }
        page.ops.append(&mut self.st.front);
        *self.sect_pages.entry(self.st.sect).or_insert(0) += 1;
    }

    fn new_page(&mut self, si: usize, reason: Top, sect_start: bool) {
        self.finish_page();
        let s = self.sect(si);
        let number = if self.pages.is_empty() {
            s.pg_start.unwrap_or(1)
        } else if sect_start && let Some(st) = s.pg_start {
            st
        } else {
            self.st.number + 1
        };
        self.pages.push(PageOut {
            w: s.w,
            h: s.h,
            ..Default::default()
        });
        self.st.sect = si;
        self.st.number = number;
        self.st.sect_first = sect_start;
        self.st.limit = None;
        self.st.floats.clear();
        self.st.notes.clear();
        self.st.notes_h = 0.0;
        self.st.front.clear();

        // Header and footer for this page.
        let slot = if sect_start && s.title_pg {
            1
        } else if self.doc.even_odd && number % 2 == 0 {
            2
        } else {
            0
        };
        let width = s.w - s.left - s.right;
        let mut top = s.top;
        let mut bottom = s.h - s.bottom;
        if let Some(blocks) = s.headers[slot]
            .as_ref()
            .and_then(|id| self.doc.parts.get(id))
        {
            let place = Place {
                x: s.left,
                y: s.header,
                sect: si,
            };
            let fixed = self.tag_fixed.replace(HEADER);
            let st = self.stack(blocks, width, Some(place));
            self.tag_fixed = fixed;
            top = top.max(s.header + st.h);
            self.page_append(st, s.left, s.header);
        }
        if let Some(blocks) = s.footers[slot]
            .as_ref()
            .and_then(|id| self.doc.parts.get(id))
        {
            let fixed = self.tag_fixed.replace(FOOTER);
            let probe = self.stack(blocks, width, None);
            let y = s.h - s.footer - probe.h;
            let place = Place {
                x: s.left,
                y,
                sect: si,
            };
            let st = self.stack(blocks, width, Some(place));
            self.tag_fixed = fixed;
            bottom = bottom.min(y);
            self.page_append(st, s.left, y);
        }
        if let Some(p) = self.pages.last() {
            self.st.body_start = p.ops.len();
        }
        self.st.top = top;
        self.st.bottom = bottom.max(top + 36.0);
        self.set_columns(si, top);
        self.st.y = top;
        self.st.max_y = top;
        self.st.fresh = true;
        self.st.reason = reason;
    }

    fn page_append(&mut self, st: Stack, dx: f32, dy: f32) {
        if let Some(p) = self.pages.last_mut() {
            p.ops.extend(st.ops.into_iter().map(|o| o.moved(dx, dy)));
            p.links.extend(
                st.links
                    .into_iter()
                    .map(|(r, l)| ([r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy], l)),
            );
        }
    }

    fn set_columns(&mut self, si: usize, top: f32) {
        let s = self.sect(si);
        let mut x = s.left;
        self.st.cols = s
            .cols
            .cols
            .iter()
            .map(|&(w, gap)| {
                let c = (x, w);
                x += w + gap;
                c
            })
            .collect();
        if self.st.cols.is_empty() {
            self.st.cols = vec![(s.left, s.w - s.left - s.right)];
        }
        self.st.col = 0;
        self.st.col_top = top;
    }

    fn continuous(&mut self, si: usize) {
        let y = self.st.max_y.max(self.st.y);
        self.st.sect = si;
        self.set_columns(si, y);
        self.st.y = y;
    }

    fn next_column(&mut self, reason: Top) {
        self.st.max_y = self.st.max_y.max(self.st.y);
        if self.st.col + 1 < self.st.cols.len() {
            self.st.col += 1;
            self.st.y = self.st.col_top;
            self.st.fresh = true;
            self.st.reason = reason;
        } else {
            let si = self.st.sect;
            self.new_page(si, reason, false);
        }
    }

    fn col(&self) -> (f32, f32) {
        self.st.cols[self.st.col.min(self.st.cols.len() - 1)]
    }

    fn bottom(&self) -> f32 {
        let gap = if self.st.notes.is_empty() {
            0.0
        } else {
            NOTE_GAP
        };
        let b = self.st.bottom - self.st.notes_h - gap;
        self.st.limit.map_or(b, |l| l.min(b))
    }

    fn page_ops(&mut self) -> &mut Vec<Op> {
        &mut self
            .pages
            .last_mut()
            .expect("a page exists while flowing")
            .ops
    }

    // ------------------------------------------------------------ blocks

    fn flow_blocks(&mut self, blocks: &[Block]) {
        for (i, b) in blocks.iter().enumerate() {
            let next = blocks.get(i + 1);
            match b {
                Block::Para(p) => {
                    let next_same = match next {
                        Some(Block::Para(n)) => same_borders(&p.p, &n.p),
                        _ => false,
                    };
                    self.flow_para(p, next, next_same);
                }
                Block::Table(t) => {
                    self.flow_table(t);
                    self.st.prev = None;
                }
            }
        }
    }

    fn env_field(&self) -> impl Fn(FieldKind, Option<NumFmt>) -> String + 'static {
        let num = self.st.number;
        let sect = self.st.sect;
        let total = if self.total > 0 {
            self.total
        } else {
            self.pages.len().max(1) as u32
        };
        let sp = self.sect_pages.get(&sect).copied().unwrap_or(total);
        let default_fmt = self.sect(sect).pg_fmt;
        move |k, f| {
            let f = f.unwrap_or(default_fmt);
            match k {
                FieldKind::Page => format_number(num, f),
                FieldKind::NumPages => format_number(total, f),
                FieldKind::SectionPages => format_number(sp.max(1), f),
            }
        }
    }

    fn atoms(&mut self, p: &Para, number: Option<(u32, usize)>) -> Vec<text::Atom> {
        let field_fn: Box<dyn Fn(FieldKind, Option<NumFmt>) -> String> = match number {
            Some((n, si)) => {
                let total = if self.total > 0 {
                    self.total
                } else {
                    self.pages.len().max(1) as u32
                };
                let sp = self.sect_pages.get(&si).copied().unwrap_or(total);
                let fmt = self.sect(si).pg_fmt;
                Box::new(move |k, f| {
                    let f = f.unwrap_or(fmt);
                    match k {
                        FieldKind::Page => format_number(n, f),
                        FieldKind::NumPages => format_number(total, f),
                        FieldKind::SectionPages => format_number(sp.max(1), f),
                    }
                })
            }
            None => Box::new(self.env_field()),
        };
        let notes = self.notes.clone();
        let note_fn = move |id: &str| notes.get(id).map_or_else(|| id.to_string(), u32::to_string);
        let mut env = Env {
            fonts: &mut self.fonts,
            field: &*field_fn,
            note_number: &note_fn,
            note_mark: self.note_mark.clone(),
        };
        text::atoms(p, &mut env)
    }

    /// Height of a paragraph laid out in `width` (lines only), and its first line.
    fn para_height(&mut self, p: &Para, width: f32) -> (f32, f32) {
        let atoms = self.atoms(p, None);
        let mut br = Breaker::new(atoms, &p.p, &p.mark, &mut self.fonts, self.doc.default_tab);
        let mut h = 0.0;
        let mut first_h = 0.0;
        let mut first = true;
        loop {
            let (x0, lim) = indents(&p.p, first, width);
            let l = br.next_line(x0, lim, first);
            if first {
                first_h = l.height;
            }
            h += l.height;
            first = false;
            if br.done() {
                break;
            }
        }
        (h, first_h)
    }

    fn flow_para(&mut self, p: &Para, next: Option<&Block>, next_same: bool) {
        let ps = &p.p;
        if ps.page_break_before && !self.st.fresh {
            let si = self.st.sect;
            self.new_page(si, Top::Hard, false);
        }
        let contextual = ps.contextual
            && self
                .st
                .prev
                .as_ref()
                .is_some_and(|(s, _, _)| *s == ps.style);
        let prev_same = self
            .st
            .prev
            .as_ref()
            .is_some_and(|(_, _, b)| borders_equal(b, &ps.borders));
        let mut before = if contextual { 0.0 } else { ps.before };
        if self.st.fresh {
            if self.st.reason == Top::Overflow {
                before = 0.0;
            }
        } else if !contextual {
            self.st.y += self.st.pending_after;
        }
        let top_border = ps.borders[0].filter(|b| b.visible() && !prev_same);
        let bottom_border = ps.borders[1].filter(|b| b.visible() && !next_same);

        // Keep with next / keep lines together.
        let (_, cw) = self.col();
        if (ps.keep_next || ps.keep_lines) && !self.st.fresh {
            let (h, _) = self.para_height(p, cw);
            let mut need = before + h;
            if ps.keep_next {
                need += self.chain_height(next, cw);
            }
            let room = self.bottom() - self.st.y;
            let full = self.st.bottom - self.st.top;
            if need > room + 0.01 && need < full {
                self.next_column(Top::Overflow);
                before = 0.0;
            }
        }
        self.tag_cur = self.para_tag(p);
        // Drawings anchored to the paragraph measure from its top, space before included.
        let anchor_top = self.st.y;
        self.st.y += before;
        if let Some(b) = top_border {
            self.st.y += b.space + b.width;
        }
        self.place_anchors(p, anchor_top);

        let atoms = self.atoms(p, None);
        let mut br = Breaker::new(atoms, ps, &p.mark, &mut self.fonts, self.doc.default_tab);
        let bg = ps.shd;
        let mut first = true;
        let mut portion_start = self.page_ops().len();
        let mut portion_top = self.st.y - top_border.map_or(0.0, |b| b.space + b.width);
        let mut first_portion = true;
        // Placed lines of this portion: (ops, links, dests, y, breaker position, notes).
        let mut placed: Vec<(usize, usize, usize, f32, usize, usize)> = Vec::new();
        let mut heading_done = false;
        // Did this portion's first line start at the top of a fresh column? Then moving
        // the paragraph on can't make it fit any better.
        let mut portion_fresh = false;
        loop {
            if br.done() && !(first && br.is_empty()) {
                break;
            }
            let (cx, cw) = self.col();
            let est = p.mark.sz * 1.2;
            let (x0, lim) = self.extent(ps, first, cx, cw, est);
            let pos_before = br.pos;
            let line = br.next_line(x0, lim, first);
            // Footnotes referenced on this line must fit on the page too.
            let new_notes = self.line_notes(&line, cw);
            let notes_h: f32 = new_notes.iter().map(|s| s.h).sum::<f32>()
                + if self.st.notes.is_empty() && !new_notes.is_empty() {
                    NOTE_GAP
                } else {
                    0.0
                };
            let fits = self.st.y + line.height + notes_h <= self.bottom() + 0.01;
            if !fits && !self.st.fresh {
                br.pos = pos_before;
                // Widow/orphan control.
                if ps.widow && !placed.is_empty() {
                    let rest = 1 + br_rest(
                        &br,
                        ps,
                        &p.mark,
                        &mut self.fonts,
                        self.doc.default_tab,
                        cw,
                        first,
                    );
                    let mut k = placed.len();
                    let total = k + rest;
                    let mut remaining = rest;
                    if remaining == 1 && k >= 2 {
                        k -= 1;
                        remaining += 1;
                    }
                    if k == 1 && total >= 2 {
                        k = 0;
                    }
                    if k == 0 && portion_fresh {
                        // A line taller than the page (a big picture) followed by more:
                        // pushing the whole paragraph on would repeat forever.
                        k = placed.len();
                    }
                    let _ = remaining;
                    if k < placed.len() {
                        let (ops, links, dests, y, pos, _) = placed[k];
                        let page = self.pages.last_mut().expect("page");
                        page.ops.truncate(ops);
                        page.links.truncate(links);
                        page.dests.truncate(dests);
                        self.st.y = y;
                        br.pos = pos;
                        placed.truncate(k);
                        if k == 0 {
                            first = true;
                        }
                    }
                }
                if !placed.is_empty() {
                    self.portion(
                        ps,
                        portion_start,
                        portion_top,
                        first_portion,
                        false,
                        None,
                        bg,
                    );
                    first_portion = false;
                }
                self.next_column(Top::Overflow);
                placed.clear();
                portion_start = self.page_ops().len();
                portion_top = self.st.y;
                continue;
            }
            if placed.is_empty() {
                portion_fresh = self.st.fresh;
            }
            let page = self.pages.last().expect("page");
            placed.push((
                page.ops.len(),
                page.links.len(),
                page.dests.len(),
                self.st.y,
                pos_before,
                0,
            ));
            if !heading_done && let Some(level) = p.outline {
                let text = para_text(p);
                if !text.is_empty() {
                    let y = self.st.y;
                    self.pages
                        .last_mut()
                        .expect("page")
                        .heads
                        .push((level, text, y));
                }
                heading_done = true;
            }
            let y = self.st.y;
            let mut ops = Vec::new();
            let mut links = Vec::new();
            let mut dests = Vec::new();
            self.emit_line(
                p, &line, cx, y, x0, lim, bg, &mut ops, &mut links, &mut dests,
            );
            let page = self.pages.last_mut().expect("page");
            page.ops.extend(ops);
            page.links.extend(links);
            page.dests.extend(dests);
            for n in new_notes {
                self.st.notes_h += n.h;
                self.st.notes.push(n);
            }
            self.st.y += line.height;
            self.st.max_y = self.st.max_y.max(self.st.y);
            self.st.fresh = false;
            first = false;
            match line.end {
                End::Forced(BreakKind::Page) => {
                    self.portion(
                        ps,
                        portion_start,
                        portion_top,
                        first_portion,
                        false,
                        None,
                        bg,
                    );
                    first_portion = false;
                    let si = self.st.sect;
                    self.new_page(si, Top::Hard, false);
                    placed.clear();
                    portion_start = self.page_ops().len();
                    portion_top = self.st.y;
                }
                End::Forced(BreakKind::Column) => {
                    self.portion(
                        ps,
                        portion_start,
                        portion_top,
                        first_portion,
                        false,
                        None,
                        bg,
                    );
                    first_portion = false;
                    self.next_column(Top::Hard);
                    placed.clear();
                    portion_start = self.page_ops().len();
                    portion_top = self.st.y;
                }
                _ => {}
            }
        }
        if let Some(b) = bottom_border {
            self.st.y += b.space + b.width;
        }
        let bottom_y = self.st.y;
        self.portion(
            ps,
            portion_start,
            portion_top,
            first_portion,
            true,
            Some(bottom_y),
            bg,
        );
        self.st.pending_after = ps.after;
        self.st.prev = Some((ps.style.clone(), ps.contextual, ps.borders));
    }

    /// Height that a keep-with-next chain needs after this paragraph.
    fn chain_height(&mut self, next: Option<&Block>, cw: f32) -> f32 {
        match next {
            Some(Block::Para(n)) => {
                let (h, first) = self.para_height(n, cw);
                n.p.before
                    + if n.p.keep_next || n.p.keep_lines {
                        h
                    } else {
                        first.max(h.min(first * 2.0))
                    }
            }
            Some(Block::Table(_)) => 20.0,
            None => 0.0,
        }
    }

    /// Line start and limit in column coordinates, narrowed by wrap-around floats.
    fn extent(&mut self, ps: &ParaStyle, first: bool, cx: f32, cw: f32, est: f32) -> (f32, f32) {
        let (mut x0, mut lim) = indents(ps, first, cw);
        let y0 = self.st.y;
        // Top-and-bottom floats push the line below them.
        loop {
            let y = self.st.y;
            let hit = self.st.floats.iter().find(|f| {
                f.wrap == Wrap::TopBottom
                    && y < f.y1
                    && y + est > f.y0
                    && f.x1 > cx
                    && f.x0 < cx + cw
            });
            match hit {
                Some(f) => self.st.y = f.y1,
                None => break,
            }
        }
        let _ = y0;
        let y = self.st.y;
        for f in &self.st.floats {
            if f.wrap != Wrap::Square || y >= f.y1 || y + est <= f.y0 {
                continue;
            }
            let (fx0, fx1) = (f.x0 - cx, f.x1 - cx);
            if fx1 <= x0 || fx0 >= lim {
                continue;
            }
            let left_room = fx0 - x0;
            let right_room = lim - fx1;
            if right_room >= left_room {
                x0 = x0.max(fx1);
            } else {
                lim = lim.min(fx0);
            }
        }
        if lim - x0 < 18.0 {
            // No usable room beside the float: go below it.
            if let Some(f) = self
                .st
                .floats
                .iter()
                .filter(|f| f.wrap == Wrap::Square && y < f.y1 && y + est > f.y0)
                .min_by(|a, b| a.y1.total_cmp(&b.y1))
            {
                self.st.y = f.y1;
                return indents(ps, first, cw);
            }
        }
        (x0, lim)
    }

    /// Lay out the footnotes a line references that aren't on this page yet.
    fn line_notes(&mut self, line: &Line, width: f32) -> Vec<Stack> {
        let mut out = Vec::new();
        for m in &line.marks {
            if let Mark::Note(id) = m
                && let Some(blocks) = self.doc.footnotes.get(id)
            {
                self.note_mark = self.notes.get(id).map(u32::to_string);
                let cur = self.tag_cur;
                let note = self.tag_add(Role::Note, (cur < FOOTER).then_some(cur));
                self.tag_parent.push(note);
                let s = self.stack(blocks, width, None);
                self.tag_parent.pop();
                self.note_mark = None;
                out.push(s);
            }
        }
        out
    }

    /// Shading and borders for the part of a paragraph on the current page, drawn under
    /// its text.
    #[allow(clippy::too_many_arguments)]
    fn portion(
        &mut self,
        ps: &ParaStyle,
        start: usize,
        top: f32,
        first: bool,
        last: bool,
        bottom: Option<f32>,
        bg: Option<Rgb>,
    ) {
        let (cx, cw) = self.col();
        let bottom = bottom.unwrap_or(self.st.y);
        if bottom <= top + 0.01 {
            return;
        }
        let lb = ps.borders[2].filter(Border::visible);
        let rb = ps.borders[3].filter(Border::visible);
        let x0 = cx + ps.left.min(ps.left + ps.first.min(0.0))
            - lb.map_or(0.0, |b| b.space + b.width / 2.0);
        let x1 = cx + cw - ps.right + rb.map_or(0.0, |b| b.space + b.width / 2.0);
        let mut under = Vec::new();
        if let Some(fill) = bg {
            under.push(Op::Rect {
                x: x0,
                y: top,
                w: x1 - x0,
                h: bottom - top,
                fill,
            });
        }
        let prev_same = self
            .st
            .prev
            .as_ref()
            .is_some_and(|(_, _, b)| borders_equal(b, &ps.borders));
        let mut lines = Vec::new();
        let hline = |y: f32, b: Border| Op::Line {
            x1: x0,
            y1: y,
            x2: x1,
            y2: y,
            width: b.width,
            color: b.color,
            style: b.style,
        };
        if first {
            if let Some(b) = ps.borders[0].filter(Border::visible)
                && !prev_same
            {
                lines.push(hline(top + b.width / 2.0, b));
            } else if let Some(b) = ps.borders[4].filter(Border::visible)
                && prev_same
            {
                lines.push(hline(top, b));
            }
        }
        if last
            && let Some(b) = ps.borders[1].filter(Border::visible)
            && self.st.y >= bottom - 0.01
        {
            lines.push(hline(bottom - b.width / 2.0, b));
        }
        for (b, x) in [(lb, x0), (rb, x1)] {
            if let Some(b) = b {
                lines.push(Op::Line {
                    x1: x,
                    y1: top,
                    x2: x,
                    y2: bottom,
                    width: b.width,
                    color: b.color,
                    style: b.style,
                });
            }
        }
        let ops = self.page_ops();
        let start = start.min(ops.len());
        ops.splice(start..start, under);
        ops.extend(lines);
    }

    // ------------------------------------------------------------ lines → ops

    #[allow(clippy::too_many_arguments)]
    fn emit_line(
        &mut self,
        p: &Para,
        line: &Line,
        cx: f32,
        y: f32,
        x0: f32,
        lim: f32,
        bg: Option<Rgb>,
        ops: &mut Vec<Op>,
        links: &mut Vec<LinkBox>,
        dests: &mut Vec<(String, f32)>,
    ) {
        let ps = &p.p;
        ops.extend(self.mark(y));
        let slack = (lim - line.right).max(0.0);
        let align = if ps.bidi { mirror(ps.align) } else { ps.align };
        let justify = match align {
            Align::Both => {
                line.end == End::Wrap
                    || (line.end == End::Forced(BreakKind::Line) && self.doc.expand_shift_return)
            }
            Align::Distribute => true,
            _ => false,
        };
        let mut dx = match align {
            Align::Center => slack / 2.0,
            Align::Right => slack,
            _ => 0.0,
        };
        let mut stretch = 0.0;
        if justify && slack > 0.01 {
            let n: usize = line.items[line.just_from.min(line.items.len())..]
                .iter()
                .map(|it| match it {
                    Placed::Seg { x, seg } => {
                        let mut gx = *x;
                        let mut c = 0;
                        for g in &seg.glyphs {
                            if g.space && gx + g.adv <= line.right + 0.01 {
                                c += 1;
                            }
                            gx += g.adv;
                        }
                        c
                    }
                    _ => 0,
                })
                .sum();
            if n > 0 {
                stretch = slack / n as f32;
            } else if align == Align::Distribute {
                dx = slack / 2.0;
            }
        }
        let base = y + line.baseline;
        let mut extra = 0.0;
        // Character borders: (border, x0, x1, top, bottom) of the current boxed run.
        let mut boxed: Option<(Border, f32, f32, f32, f32)> = None;
        let flush_box = |b: &mut Option<(Border, f32, f32, f32, f32)>, ops: &mut Vec<Op>| {
            if let Some((bd, x0, x1, t, bt)) = b.take() {
                let pad = 1.5;
                let (x0, x1, t, bt) = (x0 - pad, x1 + pad, t - pad * 0.5, bt + pad * 0.5);
                for (a, b, c, d) in [
                    (x0, t, x1, t),
                    (x0, bt, x1, bt),
                    (x0, t, x0, bt),
                    (x1, t, x1, bt),
                ] {
                    ops.push(Op::Line {
                        x1: a,
                        y1: b,
                        x2: c,
                        y2: d,
                        width: bd.width,
                        color: bd.color,
                        style: bd.style,
                    });
                }
            }
        };
        for (i, it) in line.items.iter().enumerate() {
            let stretchy = stretch > 0.0 && i >= line.just_from;
            match it {
                Placed::Seg { x, seg } => {
                    let mut seg = seg.clone();
                    let sx = cx + *x + dx + extra;
                    let trailing_seg = *x >= line.right - 0.01;
                    match (seg.rs.bdr, &mut boxed) {
                        (Some(b), Some(cur)) if cur.0 == b && !trailing_seg => {
                            cur.2 = sx + seg.width;
                            cur.3 = cur.3.min(base - seg.asc);
                            cur.4 = cur.4.max(base + seg.desc);
                        }
                        (Some(b), _) if !trailing_seg => {
                            flush_box(&mut boxed, ops);
                            boxed = Some((b, sx, sx + seg.width, base - seg.asc, base + seg.desc));
                        }
                        _ => flush_box(&mut boxed, ops),
                    }
                    if stretchy {
                        let mut gx = *x;
                        for g in &mut seg.glyphs {
                            let inside = gx + g.adv <= line.right + 0.01;
                            gx += g.adv;
                            if g.space && inside {
                                g.adv += stretch;
                                extra += stretch;
                            }
                        }
                        seg.width = seg.glyphs.iter().map(|g| g.adv).sum();
                    }
                    let trailing = *x >= line.right - 0.01;
                    self.seg_ops(seg, sx, base, bg, trailing, ops, links);
                }
                Placed::Leader { x0, x1, kind, rs } => {
                    if let Some(op) = self.leader(
                        *x0 + cx + dx + extra,
                        *x1 + cx + dx + extra,
                        base,
                        *kind,
                        rs,
                    ) {
                        ops.push(op);
                    }
                }
                Placed::Object { x, item, w, h } => {
                    if let Some(Item::Drawing(d)) = p.items.get(*item) {
                        let ox = cx + *x + dx + extra;
                        let oy = base - h;
                        self.drawing_ops(d, ox, oy, *w, *h, ops);
                    }
                }
            }
        }
        flush_box(&mut boxed, ops);
        for m in &line.marks {
            if let Mark::Bookmark(b) = m {
                dests.push((b.clone(), y));
            }
        }
        let _ = x0;
    }

    #[allow(clippy::too_many_arguments)]
    fn seg_ops(
        &self,
        seg: Seg,
        x: f32,
        base: f32,
        bg: Option<Rgb>,
        trailing: bool,
        ops: &mut Vec<Op>,
        links: &mut Vec<LinkBox>,
    ) {
        let rs = seg.rs.clone();
        let face = self.fonts.get(seg.face);
        let y = base - seg.rise;
        let w = seg.width;
        let local_bg = rs.highlight.or(rs.shd).or(bg);
        if let Some(fill) = rs.highlight.or(rs.shd)
            && !trailing
        {
            ops.push(Op::Rect {
                x,
                y: y - seg.asc,
                w,
                h: seg.asc + seg.desc,
                fill,
            });
        }
        let color = if rs.auto_color && local_bg.is_some_and(|c| luminance(c) < 0.45) {
            Rgb(255, 255, 255)
        } else {
            rs.color
        };
        let is_space = seg.text.chars().all(char::is_whitespace);
        if let Some(l) = &seg.link
            && !trailing
        {
            links.push(([x, y - seg.asc, x + w, y + seg.desc], l.clone()));
        }
        let size = seg.size;
        if !is_space {
            ops.push(Op::Text { x, y, seg, color });
        }
        let draw_ul = rs.u != super::props::Underline::None
            && !trailing
            && !(rs.u == super::props::Underline::Words && is_space);
        if draw_ul {
            let th = (face.ul_th * size).max(0.4)
                * if rs.u == super::props::Underline::Thick {
                    2.0
                } else {
                    1.0
                };
            let uy = y - face.ul_pos * size + th / 2.0;
            let c = rs.u_color.unwrap_or(color);
            let style = match rs.u {
                super::props::Underline::Dotted => BorderStyle::Dotted,
                super::props::Underline::Dash => BorderStyle::Dashed,
                _ => BorderStyle::Single,
            };
            ops.push(Op::Line {
                x1: x,
                y1: uy,
                x2: x + w,
                y2: uy,
                width: th,
                color: c,
                style,
            });
            if rs.u == super::props::Underline::Double {
                ops.push(Op::Line {
                    x1: x,
                    y1: uy + th * 2.0,
                    x2: x + w,
                    y2: uy + th * 2.0,
                    width: th,
                    color: c,
                    style,
                });
            }
        }
        if (rs.strike || rs.dstrike) && !trailing {
            let th = (face.st_th * size).max(0.4);
            let sy = y - face.st_pos * size;
            let mut ys = vec![sy];
            if rs.dstrike {
                ys = vec![sy - th, sy + th];
            }
            for yy in ys {
                ops.push(Op::Line {
                    x1: x,
                    y1: yy,
                    x2: x + w,
                    y2: yy,
                    width: th,
                    color,
                    style: BorderStyle::Single,
                });
            }
        }
    }

    fn leader(
        &mut self,
        x0: f32,
        x1: f32,
        base: f32,
        kind: Leader,
        rs: &Arc<RunStyle>,
    ) -> Option<Op> {
        let ch = match kind {
            Leader::Dot => ".",
            Leader::Hyphen => "-",
            Leader::Underscore | Leader::Heavy => "_",
            Leader::Middle => "·",
            Leader::None => return None,
        };
        let face = self.fonts.face(&rs.fonts[0], rs.b, rs.i)?;
        let g = self.fonts.shape(face, ch, false, false);
        let one = g.first()?;
        let adv = one.adv * rs.sz;
        if adv <= 0.1 {
            return None;
        }
        // Word leaves a little room before the stop and aligns dots to a grid.
        let span = x1 - x0 - adv * 0.5;
        let n = (span / adv).floor().max(0.0) as usize;
        if n == 0 {
            return None;
        }
        let start = x1 - adv * 0.5 - n as f32 * adv;
        let mut text = String::new();
        let mut glyphs = Vec::with_capacity(n);
        for i in 0..n {
            let s = text.len() as u32;
            text.push_str(ch);
            glyphs.push(PGlyph {
                id: one.id,
                start: s,
                end: s + ch.len() as u32,
                adv,
                dx: 0.0,
                dy: 0.0,
                space: false,
            });
            let _ = i;
        }
        let f = self.fonts.get(face);
        Some(Op::Text {
            x: start,
            y: base,
            color: rs.color,
            seg: Seg {
                face,
                size: rs.sz,
                rise: 0.0,
                width: adv * n as f32,
                asc: f.asc * rs.sz,
                desc: f.desc * rs.sz,
                glyphs,
                text,
                rs: rs.clone(),
                link: None,
            },
        })
    }

    fn drawing_ops(&mut self, d: &Drawing, x: f32, y: f32, w: f32, h: f32, ops: &mut Vec<Op>) {
        let cur = self.tag_cur;
        match &d.kind {
            DrawKind::Picture {
                media: Some(m),
                crop,
            } => {
                if let Some(node) = self.figure_tag(cur, alt_text(d, "Picture")) {
                    ops.push(Op::Mark { node, y });
                }
                ops.push(Op::Image {
                    media: m.clone(),
                    x,
                    y,
                    w,
                    h,
                    crop: *crop,
                });
            }
            DrawKind::Picture { media: None, .. } | DrawKind::Placeholder => {}
            DrawKind::Chart(c) => {
                let alt = match c.title.as_deref().map(str::trim) {
                    Some(t) if !t.is_empty() => format!("Chart: {t}"),
                    _ => alt_text(d, "Chart"),
                };
                let node = self.figure_tag(cur, alt);
                if let Some(node) = node {
                    ops.push(Op::Mark { node, y });
                }
                // The chart's labels are part of the figure.
                let fixed = self.tag_fixed;
                self.tag_fixed = node.or(fixed);
                let drawn = crate::chart::draw(c, w, h, self);
                self.tag_fixed = fixed;
                ops.extend(drawn.into_iter().map(|o| o.moved(x, y)));
            }
            DrawKind::Shape {
                geom,
                fill,
                line,
                text,
                ins,
                valign,
            } => {
                ops.push(Op::Shape {
                    geom: *geom,
                    x,
                    y,
                    w,
                    h,
                    fill: *fill,
                    line: *line,
                });
                if !text.is_empty() {
                    let inner_w = (w - ins[0] - ins[2]).max(4.0);
                    let st = self.stack(text, inner_w, None);
                    let room = h - ins[1] - ins[3];
                    let off = match valign {
                        VAlign::Center => ((room - st.h) / 2.0).max(0.0),
                        VAlign::Bottom => (room - st.h).max(0.0),
                        VAlign::Top => 0.0,
                    };
                    let (dx, dy) = (x + ins[0], y + ins[1] + off);
                    ops.extend(st.ops.into_iter().map(|o| o.moved(dx, dy)));
                }
            }
        }
        // Back to the paragraph for what follows on the line.
        self.tag_cur = cur;
        ops.extend(self.mark(y));
    }

    /// The node for a picture or chart in paragraph node `cur`: a new figure, or the
    /// header, footer or chart everything is going into.
    fn figure_tag(&mut self, cur: u32, alt: String) -> Option<u32> {
        self.tags.as_ref()?;
        Some(match self.tag_fixed {
            Some(f) => f,
            None if cur >= FOOTER => cur,
            None => self.tag_add(Role::Figure(alt), Some(cur)),
        })
    }

    /// Position the floating drawings of a paragraph whose top is at `para_top`.
    fn place_anchors(&mut self, p: &Para, para_top: f32) {
        let (cx, cw) = self.col();
        let s = self.sect(self.st.sect);
        for it in &p.items {
            let Item::Drawing(d) = it else { continue };
            let Some(a) = d.anchor else { continue };
            let (x, y) = anchor_pos(&a, d.w, d.h, s, cx, cw, para_top, cx + p.p.left);
            let mut ops = Vec::new();
            self.drawing_ops(d, x, y, d.w, d.h, &mut ops);
            if a.behind {
                let at = self.st.body_start.min(self.page_ops().len());
                self.page_ops().splice(at..at, ops);
            } else {
                self.st.front.extend(ops);
            }
            if a.wrap != Wrap::None {
                self.st.floats.push(FloatBox {
                    x0: x - a.dist[2],
                    x1: x + d.w + a.dist[3],
                    y0: y - a.dist[0],
                    y1: y + d.h + a.dist[1],
                    wrap: a.wrap,
                });
            }
        }
    }

    // ------------------------------------------------------------ unpaged stacks

    fn stack(&mut self, blocks: &[Block], width: f32, place: Option<Place>) -> Stack {
        let saved_tag = self.tag_cur;
        let st = self.stack_blocks(blocks, width, place);
        self.tag_cur = saved_tag;
        st
    }

    fn stack_blocks(&mut self, blocks: &[Block], width: f32, place: Option<Place>) -> Stack {
        let mut st = Stack::default();
        let mut y = 0.0;
        let mut pending_after = 0.0;
        let mut prev: Option<(String, [Option<Border>; 5])> = None;
        for (bi, b) in blocks.iter().enumerate() {
            match b {
                Block::Para(p) => {
                    let ps = &p.p;
                    let contextual =
                        ps.contextual && prev.as_ref().is_some_and(|(s, _)| *s == ps.style);
                    let prev_same = prev
                        .as_ref()
                        .is_some_and(|(_, b)| borders_equal(b, &ps.borders));
                    let next_same = matches!(blocks.get(bi + 1), Some(Block::Para(n)) if same_borders(ps, &n.p));
                    if !contextual {
                        y += pending_after;
                    }
                    let anchor_y = y;
                    if !contextual {
                        y += ps.before;
                    }
                    let tb = ps.borders[0].filter(|b| b.visible() && !prev_same);
                    if let Some(b) = tb {
                        y += b.space + b.width;
                    }
                    let top = y - tb.map_or(0.0, |b| b.space + b.width);
                    let start = st.ops.len();
                    self.tag_cur = self.para_tag(p);
                    // Anchored drawings in headers and text boxes.
                    for it in &p.items {
                        if let Item::Drawing(d) = it
                            && let Some(a) = d.anchor
                        {
                            let (ox, oy) = place.map_or((0.0, 0.0), |pl| (pl.x, pl.y));
                            let s = self.sect(place.map_or(self.st.sect, |pl| pl.sect));
                            let (x, yy) =
                                anchor_pos(&a, d.w, d.h, s, ox, width, oy + anchor_y, ox + ps.left);
                            let mut ops = Vec::new();
                            self.drawing_ops(d, x - ox, yy - oy, d.w, d.h, &mut ops);
                            st.ops.extend(ops);
                        }
                    }
                    let atoms = match place {
                        Some(pl) => self.atoms(p, Some((self.st.number, pl.sect))),
                        None => self.atoms(p, None),
                    };
                    let mut br =
                        Breaker::new(atoms, ps, &p.mark, &mut self.fonts, self.doc.default_tab);
                    let mut first = true;
                    loop {
                        if br.done() && !(first && br.is_empty()) {
                            break;
                        }
                        let (x0, lim) = indents(ps, first, width);
                        let line = br.next_line(x0, lim, first);
                        let mut ops = Vec::new();
                        let mut links = Vec::new();
                        let mut dests = Vec::new();
                        self.emit_line(
                            p, &line, 0.0, y, x0, lim, ps.shd, &mut ops, &mut links, &mut dests,
                        );
                        st.ops.extend(ops);
                        st.links.extend(links);
                        y += line.height;
                        st.breaks.push(y);
                        first = false;
                    }
                    let bb = ps.borders[1].filter(|b| b.visible() && !next_same);
                    if let Some(b) = bb {
                        y += b.space + b.width;
                    }
                    // Shading and borders under this paragraph.
                    let lb = ps.borders[2].filter(Border::visible);
                    let rb = ps.borders[3].filter(Border::visible);
                    let x0 = ps.left - lb.map_or(0.0, |b| b.space);
                    let x1 = width - ps.right + rb.map_or(0.0, |b| b.space);
                    let mut under = Vec::new();
                    if let Some(fill) = ps.shd {
                        under.push(Op::Rect {
                            x: x0,
                            y: top,
                            w: x1 - x0,
                            h: y - top,
                            fill,
                        });
                    }
                    st.ops.splice(start..start, under);
                    for (b, yy) in [(tb, top), (bb, y)] {
                        if let Some(b) = b {
                            st.ops.push(Op::Line {
                                x1: x0,
                                y1: yy,
                                x2: x1,
                                y2: yy,
                                width: b.width,
                                color: b.color,
                                style: b.style,
                            });
                        }
                    }
                    for (b, xx) in [(lb, x0), (rb, x1)] {
                        if let Some(b) = b {
                            st.ops.push(Op::Line {
                                x1: xx,
                                y1: top,
                                x2: xx,
                                y2: y,
                                width: b.width,
                                color: b.color,
                                style: b.style,
                            });
                        }
                    }
                    pending_after = ps.after;
                    prev = Some((ps.style.clone(), ps.borders));
                }
                Block::Table(t) => {
                    y += pending_after;
                    pending_after = 0.0;
                    let tb = self.table_box(t, width);
                    for r in 0..tb.rows.len() {
                        let h = tb.rows[r].h;
                        let mut ops = Vec::new();
                        let mut links = Vec::new();
                        self.draw_row(t, &tb, r, None, tb.x, y, h, &mut ops, &mut links);
                        st.ops.extend(ops);
                        st.links.extend(links);
                        y += h;
                        st.breaks.push(y);
                    }
                    prev = None;
                }
            }
        }
        // The last paragraph's space after counts inside cells and text boxes.
        st.h = y + pending_after;
        st
    }

    // ------------------------------------------------------------ tables

    fn table_widths(&self, t: &Table, avail: f32) -> Vec<f32> {
        let n = t
            .rows
            .iter()
            .map(|r| r.cells.last().map_or(0, |c| c.col + c.span))
            .max()
            .unwrap_or(0)
            .max(t.grid.len())
            .max(1);
        let mut w: Vec<f32> = t.grid.clone();
        w.resize(n, 0.0);
        let sum: f32 = w.iter().sum();
        if sum < 1.0 {
            // No grid: cell widths of the first row, else equal columns.
            let first = t.rows.first();
            let mut ok = false;
            if let Some(r) = first {
                for c in &r.cells {
                    if let Some(Width::Pt(cw)) = c.width {
                        let each = cw / c.span as f32;
                        for v in w.iter_mut().take((c.col + c.span).min(n)).skip(c.col) {
                            *v = each;
                        }
                        ok = true;
                    }
                }
            }
            if !ok || w.iter().sum::<f32>() < 1.0 {
                w = vec![avail / n as f32; n];
            }
        }
        for v in &mut w {
            if *v <= 0.0 {
                *v = 18.0;
            }
        }
        let sum: f32 = w.iter().sum();
        let target = match t.width {
            Some(Width::Pct(f)) => Some(avail * f),
            Some(Width::Pt(p)) if (p - sum).abs() > 1.0 && sum < 1.0 => Some(p),
            _ => None,
        };
        if let Some(tw) = target
            && sum > 0.0
        {
            let k = tw / sum;
            for v in &mut w {
                *v *= k;
            }
        }
        w
    }

    fn table_box(&mut self, t: &Table, avail: f32) -> TableBox {
        let widths = self.table_widths(t, avail);
        let total: f32 = widths.iter().sum();
        let mut x = t.indent;
        match t.align {
            Align::Center => x = ((avail - total) / 2.0).max(0.0),
            Align::Right => x = (avail - total).max(0.0),
            _ => {
                if !self.doc.compat15
                    && let Some(c) = t.rows.first().and_then(|r| r.cells.first())
                {
                    x -= c.mar[1];
                }
            }
        }
        let mut xs = vec![0.0; widths.len() + 1];
        for i in 0..widths.len() {
            xs[i + 1] = xs[i] + widths[i];
        }
        let nrows = t.rows.len();
        let mut rows: Vec<RowBox> = Vec::with_capacity(nrows);
        let plain = self.tag_fixed.is_some() || self.tags.is_none();
        let parent = self.tag_parent.last().copied();
        let table = if plain {
            ARTIFACT
        } else {
            self.tag_add(Role::Table, parent)
        };
        for r in &t.rows {
            let mut cells = Vec::new();
            let tr = if plain {
                ARTIFACT
            } else {
                self.tag_add(Role::TR, Some(table))
            };
            for c in &r.cells {
                let c0 = c.col.min(widths.len());
                let c1 = (c.col + c.span).min(widths.len());
                let cw = (xs[c1] - xs[c0]).max(1.0);
                let inner = (cw - c.mar[1] - c.mar[3]).max(1.0);
                let stack = if c.vmerge == Some(VMerge::Continue) {
                    Stack::default()
                } else if plain {
                    self.stack(&c.blocks, inner, None)
                } else {
                    let cell = self.tag_add(if r.header { Role::TH } else { Role::TD }, Some(tr));
                    self.tag_parent.push(cell);
                    let s = self.stack(&c.blocks, inner, None);
                    self.tag_parent.pop();
                    s
                };
                cells.push(CellBox {
                    x: xs[c0],
                    w: cw,
                    stack,
                    span_rows: 1,
                    continued: c.vmerge == Some(VMerge::Continue),
                });
            }
            let content = r
                .cells
                .iter()
                .zip(&cells)
                .filter(|(c, _)| c.vmerge.is_none())
                .map(|(c, b)| b.stack.h + c.mar[0] + c.mar[2])
                .fold(0.0, f32::max);
            let h = match r.height {
                Some((v, true)) => v,
                Some((v, false)) => content.max(v),
                None => content,
            };
            rows.push(RowBox {
                h: h.max(1.0),
                cells,
                header: r.header,
                cant_split: r.cant_split,
            });
        }
        // Vertical merges: the restart cell spans the rows below that continue it.
        for ri in 0..nrows {
            for ci in 0..t.rows[ri].cells.len() {
                let c = &t.rows[ri].cells[ci];
                if c.vmerge != Some(VMerge::Restart) {
                    continue;
                }
                let mut span = 1;
                while ri + span < nrows
                    && t.rows[ri + span]
                        .cells
                        .iter()
                        .any(|o| o.col == c.col && o.vmerge == Some(VMerge::Continue))
                {
                    span += 1;
                }
                rows[ri].cells[ci].span_rows = span;
                let need = rows[ri].cells[ci].stack.h + c.mar[0] + c.mar[2];
                let have: f32 = rows[ri..ri + span].iter().map(|r| r.h).sum();
                if need > have {
                    rows[ri + span - 1].h += need - have;
                }
            }
        }
        TableBox { x, rows }
    }

    /// Draw row `r` of a table at (`x`, `y`) with height `h`. `part` replaces the cells'
    /// stacks when a row is split across pages.
    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &self,
        t: &Table,
        tb: &TableBox,
        r: usize,
        part: Option<&[Stack]>,
        x: f32,
        y: f32,
        h: f32,
        ops: &mut Vec<Op>,
        links: &mut Vec<LinkBox>,
    ) {
        let row = &tb.rows[r];
        let mut border_ops = Vec::new();
        for (ci, cb) in row.cells.iter().enumerate() {
            let c = &t.rows[r].cells[ci];
            let cx = x + cb.x;
            // A merged cell covers the rows it spans.
            let ch = if cb.span_rows > 1 && part.is_none() {
                tb.rows[r..(r + cb.span_rows).min(tb.rows.len())]
                    .iter()
                    .map(|rr| rr.h)
                    .sum()
            } else {
                h
            };
            if let Some(fill) = c.shd.filter(|_| !cb.continued) {
                ops.push(Op::Rect {
                    x: cx,
                    y,
                    w: cb.w,
                    h: ch,
                    fill,
                });
            }
            let stack = part.map_or(&cb.stack, |p| &p[ci]);
            if !cb.continued {
                let room = ch - c.mar[0] - c.mar[2];
                let off = match c.valign {
                    VAlign::Center => ((room - stack.h) / 2.0).max(0.0),
                    VAlign::Bottom => (room - stack.h).max(0.0),
                    VAlign::Top => 0.0,
                };
                let (dx, dy) = (cx + c.mar[1], y + c.mar[0] + off);
                for op in &stack.ops {
                    let op = match op.clone() {
                        Op::Text { x, y, seg, color }
                            if seg.rs.auto_color
                                && c.shd.is_some_and(|f| luminance(f) < 0.45)
                                && color == Rgb(0, 0, 0) =>
                        {
                            Op::Text {
                                x,
                                y,
                                seg,
                                color: Rgb(255, 255, 255),
                            }
                        }
                        o => o,
                    };
                    ops.push(op.moved(dx, dy));
                }
                for (rr, l) in &stack.links {
                    links.push(([rr[0] + dx, rr[1] + dy, rr[2] + dx, rr[3] + dy], l.clone()));
                }
            }
            // Borders: top (not inside a merge), left, right, bottom (not inside a merge).
            let [top, left, bottom, right] = c.borders;
            let next_continues = t.rows.get(r + 1).is_some_and(|n| {
                n.cells
                    .iter()
                    .any(|o| o.col == c.col && o.vmerge == Some(VMerge::Continue))
            });
            let edges = [
                (top.filter(|_| !cb.continued), cx, y, cx + cb.w, y),
                (left, cx, y, cx, y + h),
                (right, cx + cb.w, y, cx + cb.w, y + h),
                (
                    bottom.filter(|_| !next_continues || part.is_some()),
                    cx,
                    y + h,
                    cx + cb.w,
                    y + h,
                ),
            ];
            for (b, x1, y1, x2, y2) in edges {
                if let Some(b) = b.filter(Border::visible) {
                    border_ops.push(Op::Line {
                        x1,
                        y1,
                        x2,
                        y2,
                        width: b.width,
                        color: b.color,
                        style: b.style,
                    });
                }
            }
        }
        ops.extend(border_ops);
    }

    fn flow_table(&mut self, t: &Table) {
        if t.rows.is_empty() {
            return;
        }
        if !self.st.fresh {
            self.st.y += self.st.pending_after;
        }
        self.st.pending_after = 0.0;
        let (_, cw) = self.col();
        let tb = self.table_box(t, cw);
        let headers: Vec<usize> = tb
            .rows
            .iter()
            .take_while(|r| r.header)
            .enumerate()
            .map(|(i, _)| i)
            .collect();
        let mut r = 0;
        let mut parts: Option<Vec<Stack>> = None;
        let mut on_page_rows = 0usize;
        while r < tb.rows.len() {
            let (cx, _) = self.col();
            let x = cx + tb.x;
            let row = &tb.rows[r];
            let stacks: Vec<Stack> = match &parts {
                Some(p) => p.clone(),
                None => row.cells.iter().map(|c| c.stack.clone()).collect(),
            };
            let h = if parts.is_some() {
                stacks
                    .iter()
                    .zip(&t.rows[r].cells)
                    .map(|(s, c)| s.h + c.mar[0] + c.mar[2])
                    .fold(0.0, f32::max)
                    .max(1.0)
            } else {
                row.h
            };
            let room = self.bottom() - self.st.y;
            if h > room + 0.01 && !(self.st.fresh && on_page_rows == 0) {
                let full = self.st.bottom - self.st.top;
                let can_split = !row.cant_split && !row.header && room > 6.0 && h <= full * 4.0;
                if can_split {
                    // Split each cell at its last line break that fits.
                    let mut head = Vec::new();
                    let mut tail = Vec::new();
                    let mut any = false;
                    for (s, c) in stacks.iter().zip(&t.rows[r].cells) {
                        let avail = room - c.mar[0] - c.mar[2];
                        match s.split(avail) {
                            Some((a, b)) if s.h > avail => {
                                any = true;
                                head.push(a);
                                tail.push(b);
                            }
                            _ if s.h <= avail => {
                                head.push(s.clone());
                                tail.push(Stack::default());
                            }
                            _ => {
                                head.push(Stack::default());
                                tail.push(s.clone());
                            }
                        }
                    }
                    if any {
                        let mut ops = Vec::new();
                        let mut links = Vec::new();
                        self.draw_row(
                            t,
                            &tb,
                            r,
                            Some(&head),
                            x,
                            self.st.y,
                            room,
                            &mut ops,
                            &mut links,
                        );
                        let page = self.pages.last_mut().expect("page");
                        page.ops.extend(ops);
                        page.links.extend(links);
                        self.st.y += room;
                        self.next_column(Top::Overflow);
                        self.repeat_headers(t, &tb, &headers, r);
                        parts = Some(tail);
                        on_page_rows = 0;
                        continue;
                    }
                }
                self.next_column(Top::Overflow);
                self.repeat_headers(t, &tb, &headers, r);
                on_page_rows = 0;
                continue;
            }
            let mut ops = Vec::new();
            let mut links = Vec::new();
            let part_ref = parts.as_deref();
            self.draw_row(t, &tb, r, part_ref, x, self.st.y, h, &mut ops, &mut links);
            let page = self.pages.last_mut().expect("page");
            page.ops.extend(ops);
            page.links.extend(links);
            self.st.y += h;
            self.st.max_y = self.st.max_y.max(self.st.y);
            self.st.fresh = false;
            on_page_rows += 1;
            parts = None;
            r += 1;
        }
    }

    fn repeat_headers(&mut self, t: &Table, tb: &TableBox, headers: &[usize], at: usize) {
        if headers.is_empty() || headers.contains(&at) {
            return;
        }
        let (cx, _) = self.col();
        for &h in headers {
            let mut ops = Vec::new();
            let mut links = Vec::new();
            let hh = tb.rows[h].h;
            self.draw_row(
                t,
                tb,
                h,
                None,
                cx + tb.x,
                self.st.y,
                hh,
                &mut ops,
                &mut links,
            );
            // The repeated copy is decoration; the first one is the table's header.
            for op in &mut ops {
                if let Op::Mark { node, .. } = op {
                    *node = ARTIFACT;
                }
            }
            let page = self.pages.last_mut().expect("page");
            page.ops.extend(ops);
            page.links.extend(links);
            self.st.y += hh;
        }
        self.st.fresh = false;
    }

    // ------------------------------------------------------------ endnotes

    fn endnotes(&mut self) {
        let order = self.endnote_order.clone();
        let (cx, cw) = self.col();
        self.st.y += self.st.pending_after + 12.0;
        let y = self.st.y;
        self.page_ops().push(Op::Line {
            x1: cx,
            y1: y,
            x2: cx + 144.0,
            y2: y,
            width: 0.5,
            color: Rgb(0, 0, 0),
            style: BorderStyle::Single,
        });
        self.st.y += 6.0;
        for id in order {
            let Some(blocks) = self.doc.endnotes.get(&id) else {
                continue;
            };
            self.note_mark = self.notes.get(&format!("e{id}")).map(u32::to_string);
            let parent = self.tag_parent.last().copied();
            let note = self.tag_add(Role::Note, parent);
            self.tag_parent.push(note);
            let st = self.stack(blocks, cw, None);
            self.tag_parent.pop();
            self.note_mark = None;
            if self.st.y + st.h > self.bottom() && !self.st.fresh {
                self.next_column(Top::Overflow);
            }
            let (cx, _) = self.col();
            let y = self.st.y;
            self.page_append(st.clone(), cx, y);
            self.st.y += st.h;
            self.st.fresh = false;
        }
    }
}

const NOTE_GAP: f32 = 10.0;

struct Snapshot {
    pages: usize,
    lens: (usize, usize, usize, usize),
    st: PageState,
    sect_pages: HashMap<usize, u32>,
}

struct CellBox {
    x: f32,
    w: f32,
    stack: Stack,
    span_rows: usize,
    continued: bool,
}

struct RowBox {
    h: f32,
    cells: Vec<CellBox>,
    header: bool,
    cant_split: bool,
}

struct TableBox {
    x: f32,
    rows: Vec<RowBox>,
}

fn indents(ps: &ParaStyle, first: bool, width: f32) -> (f32, f32) {
    let x0 = ps.left + if first { ps.first } else { 0.0 };
    let lim = (width - ps.right).max(x0 + 1.0);
    (x0.max(-ps.left.abs() - 72.0), lim)
}

fn borders_equal(a: &[Option<Border>; 5], b: &[Option<Border>; 5]) -> bool {
    let vis = |x: &Option<Border>| x.filter(Border::visible);
    a.iter()
        .take(4)
        .zip(b.iter().take(4))
        .all(|(x, y)| vis(x) == vis(y))
        && a.iter().take(4).any(|x| vis(x).is_some())
}

fn same_borders(a: &ParaStyle, b: &ParaStyle) -> bool {
    borders_equal(&a.borders, &b.borders)
}

/// Remaining line count of a breaker, without moving it.
fn br_rest(
    br: &Breaker<'_>,
    ps: &ParaStyle,
    _mark: &RunStyle,
    _fonts: &mut Fonts,
    _tab: f32,
    width: f32,
    first: bool,
) -> usize {
    let mut b = br.clone();
    let mut n = 0;
    let mut f = first;
    while !b.done() && n < 10_000 {
        let (x0, lim) = indents(ps, f, width);
        b.next_line(x0, lim, f);
        f = false;
        n += 1;
    }
    n
}

#[allow(clippy::too_many_arguments)]
fn anchor_pos(
    a: &Anchor,
    w: f32,
    h: f32,
    s: &SectPr,
    cx: f32,
    cw: f32,
    para_top: f32,
    char_x: f32,
) -> (f32, f32) {
    let (ox, span) = match a.h_rel {
        HRel::Page => (0.0, s.w),
        HRel::Margin => (s.left, s.w - s.left - s.right),
        HRel::Column => (cx, cw),
        HRel::Character => (char_x, cw),
        HRel::LeftMargin => (0.0, s.left),
        HRel::RightMargin => (s.w - s.right, s.right),
    };
    let x = match a.h {
        Pos::Offset(o) => ox + o,
        Pos::Align(0) => ox,
        Pos::Align(1) => ox + (span - w) / 2.0,
        Pos::Align(_) => ox + span - w,
    };
    let (oy, vspan) = match a.v_rel {
        VRel::Page => (0.0, s.h),
        VRel::Margin => (s.top, s.h - s.top - s.bottom),
        VRel::Paragraph | VRel::Line => (para_top, 0.0),
        VRel::TopMargin => (0.0, s.top),
        VRel::BottomMargin => (s.h - s.bottom, s.bottom),
    };
    let y = match a.v {
        Pos::Offset(o) => oy + o,
        Pos::Align(0) => oy,
        Pos::Align(1) => oy + (vspan - h) / 2.0,
        Pos::Align(_) => oy + vspan - h,
    };
    (x, y)
}

/// Lays out text blocks off the page flow, from (0, 0): what charts use for their labels,
/// on slides and sheets ([`Boxes`]) and inside Word documents (the page layout itself).
pub(crate) trait LayText {
    fn lay_text(&mut self, blocks: &[Block], width: f32) -> (Vec<Op>, f32);
}

impl LayText for Boxes<'_> {
    fn lay_text(&mut self, blocks: &[Block], width: f32) -> (Vec<Op>, f32) {
        let s = self.l.stack(blocks, width, None);
        (s.ops, s.h)
    }
}

impl LayText for Layout<'_> {
    fn lay_text(&mut self, blocks: &[Block], width: f32) -> (Vec<Op>, f32) {
        let s = self.stack(blocks, width, None);
        (s.ops, s.h)
    }
}

/// Text boxes laid out outside the page flow (slides, sheets): each call lays `blocks`
/// out `width` wide from (0, 0) with the document's fonts and rules.
pub struct Boxes<'d> {
    l: Layout<'d>,
}

impl<'d> Boxes<'d> {
    /// `doc` needs at least one section (its size is used for anchored drawings).
    pub fn new(doc: &'d Document, fonts: Fonts) -> Boxes<'d> {
        Boxes {
            l: Layout::new(doc, fonts),
        }
    }

    /// The drawing operations, the height, and the link areas.
    pub fn lay(&mut self, blocks: &[Block], width: f32) -> (Vec<Op>, f32, Vec<LinkBox>) {
        let s = self.l.stack(blocks, width, None);
        (s.ops, s.h, s.links)
    }

    /// Mark what's laid out from now on with its structure, for a tagged PDF.
    pub fn enable_tags(&mut self) {
        self.l.tags.get_or_insert_with(Tags::default);
    }

    pub fn tagging(&self) -> bool {
        self.l.tags.is_some()
    }

    /// Put everything laid out until [`Self::unfix_tag`] into `node` instead of a node
    /// per paragraph.
    pub fn fix_tag(&mut self, node: u32) {
        if self.l.tags.is_some() {
            self.l.tag_fixed = Some(node);
        }
    }

    pub fn unfix_tag(&mut self) {
        self.l.tag_fixed = None;
    }

    /// A new structure node (ARTIFACT when not tagging).
    pub fn add_tag(&mut self, role: Role, parent: Option<u32>) -> u32 {
        self.l.tag_add(role, parent)
    }

    pub fn into_parts(self) -> (Fonts, Option<Tags>) {
        (self.l.fonts, self.l.tags)
    }
}
