//! Organise mode of the page picker (docs/12 §4, docs/17 §2): the pages as tiles to drag
//! into a new order, turn, duplicate, delete, add blank pages or other PDFs, and a page
//! editor to crop, black out, sign and comment. Nothing is written until *Save as a new
//! PDF*, which runs `swirvo_jobs::organise` on a background thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use swirvo_core::CancelToken;
use swirvo_engines::pdfedit::annotate::Comment;
use swirvo_engines::pdfedit::assemble::PageSrc;
use swirvo_engines::pdfedit::geometry::Box4;
use swirvo_engines::pdfedit::organise::{Mark, Sheet};
use swirvo_ui::theme::size;
use swirvo_ui::widgets::{self, ButtonKind};
use swirvo_ui::{Align, Icon, Id, Key, Pixmap, Point, Rect, Request, Rgba, Ui, Weight};

use super::{CELL_W, FileView, GAP, PagePicker};
use crate::thumbs::to_pixmap;

const UNDO_DEPTH: usize = 50;
const PANEL_W: f32 = 290.0;
/// A press turns into a drag after moving this far (logical px).
const DRAG_SLOP: f32 = 6.0;

/// Comment colours: yellow, green, blue, red, black.
const PALETTE: [[u8; 3]; 5] = [
    [255, 214, 0],
    [76, 175, 80],
    [33, 118, 255],
    [220, 38, 38],
    [20, 20, 20],
];

/// A mark as the editor holds it: what the job gets, plus what's needed to draw it.
#[derive(Clone)]
pub(super) enum UiMark {
    Redact(Box4),
    Picture(Box4, Arc<Vec<u8>>, Arc<Pixmap>),
    Comment(Comment),
}

impl UiMark {
    fn label(&self) -> &'static str {
        match self {
            UiMark::Redact(_) => "Blacked-out area",
            UiMark::Picture(..) => "Signature picture",
            UiMark::Comment(c) => match c {
                Comment::Highlight { .. } => "Highlight",
                Comment::Note { .. } => "Note",
                Comment::Ink { .. } => "Drawing",
                Comment::Box { .. } => "Box",
                Comment::Text { .. } => "Text",
            },
        }
    }

    /// The same mark after the page turns a quarter clockwise; `w` is the page's width
    /// before turning.
    fn turned(&self, w: f32) -> UiMark {
        let p = |q: [f32; 2]| [q[1], w - q[0]];
        let b = |r: Box4| {
            let (a, c) = (p([r[0], r[1]]), p([r[2], r[3]]));
            [
                a[0].min(c[0]),
                a[1].min(c[1]),
                a[0].max(c[0]),
                a[1].max(c[1]),
            ]
        };
        match self {
            UiMark::Redact(r) => UiMark::Redact(b(*r)),
            UiMark::Picture(r, bytes, pm) => UiMark::Picture(b(*r), bytes.clone(), pm.clone()),
            UiMark::Comment(c) => UiMark::Comment(match c {
                Comment::Highlight { rect, color } => Comment::Highlight {
                    rect: b(*rect),
                    color: *color,
                },
                Comment::Box { rect, color, width } => Comment::Box {
                    rect: b(*rect),
                    color: *color,
                    width: *width,
                },
                Comment::Text {
                    rect,
                    text,
                    size,
                    color,
                } => Comment::Text {
                    rect: b(*rect),
                    text: text.clone(),
                    size: *size,
                    color: *color,
                },
                Comment::Note { at, text, color } => Comment::Note {
                    at: p(*at),
                    text: text.clone(),
                    color: *color,
                },
                Comment::Ink {
                    strokes,
                    color,
                    width,
                } => Comment::Ink {
                    strokes: strokes
                        .iter()
                        .map(|s| s.iter().map(|&q| p(q)).collect())
                        .collect(),
                    color: *color,
                    width: *width,
                },
            }),
        }
    }
}

#[derive(Clone)]
pub(super) struct Tile {
    src: PageSrc,
    turn: i64,
    /// Display points of the turned page, y up.
    crop: Option<Box4>,
    marks: Vec<UiMark>,
    sel: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EditTool {
    Crop,
    Redact,
    Sign,
    Highlight,
    Note,
    Box,
    Ink,
    Text,
}

const TOOLS: [(EditTool, &str, Icon); 8] = [
    (EditTool::Highlight, "Highlight", Icon::Highlight),
    (EditTool::Note, "Note", Icon::Note),
    (EditTool::Text, "Text", Icon::TextBox),
    (EditTool::Ink, "Draw", Icon::Pen),
    (EditTool::Box, "Box", Icon::Square),
    (EditTool::Sign, "Sign", Icon::Pen),
    (EditTool::Redact, "Black out", Icon::Redact),
    (EditTool::Crop, "Crop", Icon::Crop),
];

enum Draft {
    /// Two corners in page points.
    Rect([f32; 2], [f32; 2]),
    Ink(Vec<[f32; 2]>),
}

struct Drag {
    from: usize,
    start: Point,
    active: bool,
}

enum Pick {
    Insert(usize),
    Signature,
}

type Loaded = Result<(Arc<Vec<u8>>, Pixmap), String>;

pub(super) struct Organiser {
    tiles: Vec<Tile>,
    undo: Vec<Vec<Tile>>,
    anchor: Option<usize>,
    drag: Option<Drag>,
    editing: Option<usize>,
    tool: EditTool,
    draft: Option<Draft>,
    text: String,
    color: usize,
    signature: Option<(Arc<Vec<u8>>, Arc<Pixmap>)>,
    loading: Option<Receiver<Loaded>>,
    picking: Option<Pick>,
    /// Inserted PDFs whose page count isn't known yet: (file, insert at).
    waiting: Vec<(usize, usize)>,
    /// Turned copies of page pictures: (file, page, turn, big).
    turned: HashMap<(usize, usize, i64, bool), Pixmap>,
    hint: Option<String>,
}

impl Organiser {
    pub(super) fn new(pages: usize) -> Organiser {
        Organiser {
            tiles: (0..pages)
                .map(|page| Tile {
                    src: PageSrc::Page { file: 0, page },
                    turn: 0,
                    crop: None,
                    marks: Vec::new(),
                    sel: false,
                })
                .collect(),
            undo: Vec::new(),
            anchor: None,
            drag: None,
            editing: None,
            tool: EditTool::Highlight,
            draft: None,
            text: String::new(),
            color: 0,
            signature: None,
            loading: None,
            picking: None,
            waiting: Vec::new(),
            turned: HashMap::new(),
            hint: None,
        }
    }

    pub(super) fn poll(&mut self, files: &[FileView], arrived: &[usize]) {
        for &k in arrived {
            let Some(pos) = self.waiting.iter().position(|w| w.0 == k) else {
                continue;
            };
            let (_, at) = self.waiting.remove(pos);
            let n = files[k].sizes.len();
            let at = at.min(self.tiles.len());
            self.snapshot();
            let new = (0..n).map(|page| Tile {
                src: PageSrc::Page { file: k, page },
                turn: 0,
                crop: None,
                marks: Vec::new(),
                sel: true,
            });
            for t in &mut self.tiles {
                t.sel = false;
            }
            self.tiles.splice(at..at, new);
            // Files waiting for the same gap that were picked later go after this one.
            for w in &mut self.waiting {
                if w.1 > at || (w.1 == at && w.0 > k) {
                    w.1 += n;
                }
            }
            self.hint = Some(format!("Added {} from {}", pages(n), files[k].name));
        }
        for (k, f) in files.iter().enumerate() {
            if let Some(e) = &f.error
                && let Some(pos) = self.waiting.iter().position(|w| w.0 == k)
            {
                self.waiting.remove(pos);
                self.hint = Some(format!("Couldn't open {}: {e}", f.name));
            }
        }
        if let Some(rx) = &self.loading
            && let Ok(r) = rx.try_recv()
        {
            self.loading = None;
            match r {
                Ok((bytes, pm)) => {
                    self.signature = Some((bytes, Arc::new(pm)));
                    self.tool = EditTool::Sign;
                    self.hint = Some("Drag a box on the page to place the signature".into());
                }
                Err(e) => self.hint = Some(e),
            }
        }
    }

    fn snapshot(&mut self) {
        self.undo.push(self.tiles.clone());
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
    }

    fn undo(&mut self) {
        if let Some(t) = self.undo.pop() {
            self.tiles = t;
            if self.editing.is_some_and(|e| e >= self.tiles.len()) {
                self.editing = None;
            }
        }
    }

    fn selected(&self) -> Vec<usize> {
        (0..self.tiles.len())
            .filter(|&i| self.tiles[i].sel)
            .collect()
    }

    /// Move the tiles `which` (in order) so they start at gap `to` (0..=len).
    fn move_to(&mut self, which: &[usize], to: usize) {
        if which.is_empty() {
            return;
        }
        let before = which.iter().filter(|&&i| i < to).count();
        let mut moving = Vec::with_capacity(which.len());
        let mut rest = Vec::with_capacity(self.tiles.len());
        for (i, t) in std::mem::take(&mut self.tiles).into_iter().enumerate() {
            if which.contains(&i) {
                moving.push(t);
            } else {
                rest.push(t);
            }
        }
        let at = (to - before).min(rest.len());
        rest.splice(at..at, moving);
        self.tiles = rest;
    }

    fn turn(&mut self, which: &[usize], files: &[FileView]) {
        for &i in which {
            let (w, _) = tile_size(&self.tiles[i], files);
            let t = &mut self.tiles[i];
            t.turn = (t.turn + 90).rem_euclid(360);
            t.marks = t.marks.iter().map(|m| m.turned(w)).collect();
            t.crop = t.crop.map(|c| {
                let (a, b) = ([c[1], w - c[0]], [c[3], w - c[2]]);
                [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[0].max(b[0]),
                    a[1].max(b[1]),
                ]
            });
        }
    }

    fn sheets_and_marks(&self) -> (Vec<Sheet>, Vec<Mark>) {
        let sheets = self
            .tiles
            .iter()
            .map(|t| Sheet {
                src: t.src,
                turn: t.turn,
                crop: t.crop,
            })
            .collect();
        let marks = self
            .tiles
            .iter()
            .enumerate()
            .flat_map(|(page, t)| {
                t.marks.iter().map(move |m| match m {
                    UiMark::Redact(rect) => Mark::Redact { page, rect: *rect },
                    UiMark::Picture(rect, image, _) => Mark::Picture {
                        page,
                        rect: *rect,
                        image: image.clone(),
                    },
                    UiMark::Comment(c) => Mark::Comment {
                        page,
                        comment: c.clone(),
                    },
                })
            })
            .collect();
        (sheets, marks)
    }

    pub(super) fn tiles_len(&self) -> usize {
        self.tiles.len()
    }

    fn mark_count(&self) -> usize {
        self.tiles
            .iter()
            .map(|t| t.marks.len() + usize::from(t.crop.is_some()))
            .sum()
    }
}

fn pages(n: usize) -> String {
    if n == 1 {
        "1 page".into()
    } else {
        format!("{n} pages")
    }
}

/// The tile's page as the reader will see it (turned), before cropping, in points.
fn tile_size(t: &Tile, files: &[FileView]) -> (f32, f32) {
    let (w, h) = match t.src {
        PageSrc::Page { file, page } => files
            .get(file)
            .and_then(|f| f.sizes.get(page).copied())
            .unwrap_or((595.0, 842.0)),
        PageSrc::Blank { w, h } => (w, h),
    };
    if t.turn % 180 == 0 { (w, h) } else { (h, w) }
}

/// `pm` turned clockwise by `turn` degrees.
fn turn_pixmap(pm: &Pixmap, turn: i64) -> Option<Pixmap> {
    let (w, h) = (pm.width(), pm.height());
    let quarter = turn.rem_euclid(360) / 90;
    let (nw, nh) = if quarter % 2 == 1 { (h, w) } else { (w, h) };
    let mut out = Pixmap::new(nw, nh)?;
    let src = pm.pixels();
    let dst = out.pixels_mut();
    for y in 0..h {
        for x in 0..w {
            let (dx, dy) = match quarter {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                3 => (y, w - 1 - x),
                _ => (x, y),
            };
            dst[(dy * nw + dx) as usize] = src[(y * w + x) as usize];
        }
    }
    Some(out)
}

fn rgba(c: [u8; 3]) -> Rgba {
    Rgba::rgb(((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32)
}

/// Maps page points (y up) to the screen rectangle `pr` the page is drawn in.
#[derive(Clone, Copy)]
struct View {
    pr: Rect,
    w: f32,
    h: f32,
}

impl View {
    fn k(self) -> f32 {
        self.pr.w / self.w.max(1.0)
    }
    fn screen(self, p: [f32; 2]) -> Point {
        Point::new(
            self.pr.x + p[0] * self.k(),
            self.pr.y + (self.h - p[1]) * self.k(),
        )
    }
    fn rect(self, b: Box4) -> Rect {
        let a = self.screen([b[0], b[3]]);
        let c = self.screen([b[2], b[1]]);
        Rect::new(a.x, a.y, c.x - a.x, c.y - a.y)
    }
    fn page_pt(self, p: Point) -> [f32; 2] {
        let k = self.k();
        [
            ((p.x - self.pr.x) / k).clamp(0.0, self.w),
            (self.h - (p.y - self.pr.y) / k).clamp(0.0, self.h),
        ]
    }
}

fn norm(a: [f32; 2], b: [f32; 2]) -> Box4 {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[0].max(b[0]),
        a[1].max(b[1]),
    ]
}

/// Draw `marks` (and the crop's dimmed outside) over a page drawn at `v`.
fn draw_marks(ui: &mut Ui, v: View, crop: Option<Box4>, marks: &[UiMark]) {
    let k = v.k();
    for m in marks {
        match m {
            UiMark::Redact(b) => ui.fill_rect(v.rect(*b), Rgba::BLACK),
            UiMark::Picture(b, _, pm) => ui.image(v.rect(*b), pm, 0.0, false),
            UiMark::Comment(c) => match c {
                Comment::Highlight { rect, color } => {
                    ui.fill_rect(v.rect(*rect), rgba(*color).fade(0.45));
                }
                Comment::Box { rect, color, width } => {
                    let r = v.rect(*rect);
                    ui.stroke_rrect_w(r, 0.0, rgba(*color), (width * k).max(1.0));
                }
                Comment::Ink {
                    strokes,
                    color,
                    width,
                } => {
                    let pts: Vec<Vec<Point>> = strokes
                        .iter()
                        .map(|s| s.iter().map(|&p| v.screen(p)).collect())
                        .collect();
                    ui.stroke_logical(
                        |pb, s| {
                            for st in &pts {
                                for (i, p) in st.iter().enumerate() {
                                    if i == 0 {
                                        pb.move_to(p.x * s, p.y * s);
                                    } else {
                                        pb.line_to(p.x * s, p.y * s);
                                    }
                                }
                            }
                        },
                        rgba(*color),
                        (width * k).max(1.0),
                    );
                }
                Comment::Note { at, color, .. } => {
                    let p = v.screen(*at);
                    let side = (20.0 * k).max(10.0);
                    let r = Rect::new(p.x, p.y, side, side);
                    ui.fill_rrect(r, 2.0, rgba(*color));
                    ui.icon(r.inset(side * 0.12), Icon::Note, Rgba::BLACK.fade(0.7));
                }
                Comment::Text {
                    rect,
                    text,
                    size: s,
                    color,
                } => {
                    let r = v.rect(*rect);
                    let fs = (s * k).max(4.0);
                    let line = text.lines().next().unwrap_or("");
                    ui.text_in(
                        Rect::new(r.x + 2.0 * k, r.y, r.w, fs * 1.4),
                        line,
                        fs,
                        Weight::Regular,
                        rgba(*color),
                        Align::Left,
                    );
                }
            },
        }
    }
    if let Some(c) = crop {
        let pr = v.pr;
        let inner = v.rect(c);
        let shade = Rgba::BLACK.fade(0.45);
        ui.fill_rect(Rect::new(pr.x, pr.y, pr.w, inner.y - pr.y), shade);
        ui.fill_rect(
            Rect::new(pr.x, inner.bottom(), pr.w, pr.bottom() - inner.bottom()),
            shade,
        );
        ui.fill_rect(Rect::new(pr.x, inner.y, inner.x - pr.x, inner.h), shade);
        ui.fill_rect(
            Rect::new(inner.right(), inner.y, pr.right() - inner.right(), inner.h),
            shade,
        );
        ui.stroke_rrect_w(inner, 0.0, Rgba::WHITE, 1.5);
    }
}

impl PagePicker {
    fn org_page_picture(&mut self, i: usize, big: bool) -> Option<Pixmap> {
        let org = self.org.as_mut()?;
        let t = &org.tiles[i];
        let PageSrc::Page { file, page } = t.src else {
            return None;
        };
        let f = self.files.get(file)?;
        let pm = if big {
            match &f.big {
                Some((p, pm)) if *p == page => pm,
                _ => f.thumbs.get(&page)?,
            }
        } else {
            f.thumbs.get(&page)?
        };
        if t.turn == 0 {
            return Some(pm.clone());
        }
        let real_big = big && f.big.as_ref().is_some_and(|(p, _)| *p == page);
        let key = (file, page, t.turn, real_big);
        if let Some(p) = org.turned.get(&key) {
            return Some(p.clone());
        }
        let turned = turn_pixmap(pm, t.turn)?;
        if real_big {
            org.turned.retain(|k, _| !k.3);
        }
        org.turned.insert(key, turned.clone());
        Some(turned)
    }

    /// Keys for Organise mode. True when the key was used.
    pub(super) fn org_keys(&mut self, ui: &mut Ui) -> bool {
        let Some(org) = self.org.as_mut() else {
            return false;
        };
        if ui.input.ctrl_key(Key::Z) {
            org.undo();
            return true;
        }
        if org.editing.is_some() {
            if ui.input.key(Key::Escape) {
                org.editing = None;
                org.draft = None;
                return true;
            }
            return false;
        }
        if ui.input.ctrl_key(Key::A) {
            for t in &mut org.tiles {
                t.sel = true;
            }
            return true;
        }
        if ui.input.ctrl_key(Key::D) {
            self.org_duplicate();
            return true;
        }
        if ui.input.key(Key::Delete) {
            self.org_delete();
            return true;
        }
        if ui.input.key(Key::Enter) {
            let first = org.selected().first().copied();
            if let Some(i) = first {
                org.editing = Some(i);
                return true;
            }
        }
        false
    }

    fn org_duplicate(&mut self) {
        let Some(org) = self.org.as_mut() else { return };
        let sel = org.selected();
        if sel.is_empty() {
            return;
        }
        org.snapshot();
        for &i in sel.iter().rev() {
            let mut t = org.tiles[i].clone();
            org.tiles[i].sel = false;
            t.sel = true;
            org.tiles.insert(i + 1, t);
        }
    }

    fn org_delete(&mut self) {
        let Some(org) = self.org.as_mut() else { return };
        let sel = org.selected();
        if sel.is_empty() {
            return;
        }
        if sel.len() == org.tiles.len() {
            org.hint = Some("Keep at least one page".into());
            return;
        }
        org.snapshot();
        for &i in sel.iter().rev() {
            org.tiles.remove(i);
        }
        org.editing = None;
    }

    fn org_blank(&mut self) {
        let files = &self.files;
        let Some(org) = self.org.as_mut() else { return };
        let at = org.selected().last().map_or(org.tiles.len(), |&i| i + 1);
        let near = at.checked_sub(1).or((!org.tiles.is_empty()).then_some(0));
        let (w, h) = near.map_or((595.0, 842.0), |i| tile_size(&org.tiles[i], files));
        org.snapshot();
        for t in &mut org.tiles {
            t.sel = false;
        }
        org.tiles.insert(
            at,
            Tile {
                src: PageSrc::Blank { w, h },
                turn: 0,
                crop: None,
                marks: Vec::new(),
                sel: true,
            },
        );
    }

    /// Open PDFs to insert after the selection (or at the end).
    fn org_insert(&mut self, paths: Vec<PathBuf>, at: usize) {
        // Each file is inserted at `at` when it opens, after the files picked before it.
        for p in paths {
            let is_pdf = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
            if !is_pdf {
                if let Some(org) = self.org.as_mut() {
                    org.hint = Some("Only PDFs can be inserted".into());
                }
                continue;
            }
            let k = self.files.len();
            self.files.push(FileView::start(p, self.waker.clone()));
            if let Some(org) = self.org.as_mut() {
                org.waiting.push((k, at));
            }
        }
    }

    fn org_load_signature(&mut self, path: PathBuf) {
        let Some(org) = self.org.as_mut() else { return };
        let (tx, rx) = channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let r = (|| {
                let bytes = std::fs::read(&path).map_err(|e| format!("Couldn't read it: {e}"))?;
                match image::guess_format(&bytes) {
                    Ok(image::ImageFormat::Png | image::ImageFormat::Jpeg) => {}
                    _ => return Err("The signature must be a PNG or JPEG picture".to_string()),
                }
                let img = image::load_from_memory(&bytes)
                    .map_err(|e| format!("Couldn't read the picture: {e}"))?
                    .to_rgba8();
                let pm = to_pixmap(&img).ok_or("The picture is empty")?;
                Ok((Arc::new(bytes), pm))
            })();
            let _ = tx.send(r);
            waker();
        });
        org.loading = Some(rx);
        org.hint = Some("Reading the picture…".into());
    }

    pub(super) fn org_event(&mut self, files: Vec<PathBuf>) {
        let Some(org) = self.org.as_mut() else { return };
        match org.picking.take() {
            Some(Pick::Insert(at)) => self.org_insert(files, at),
            Some(Pick::Signature) => {
                if let Some(p) = files.into_iter().next() {
                    self.org_load_signature(p);
                }
            }
            None => {}
        }
    }

    /// Files dropped on the window: PDFs are inserted, a picture becomes the signature.
    fn org_dropped(&mut self, dropped: Vec<PathBuf>) {
        let Some(org) = self.org.as_ref() else { return };
        let at = org.selected().last().map_or(org.tiles.len(), |&i| i + 1);
        let (pdfs, other): (Vec<PathBuf>, Vec<PathBuf>) = dropped
            .into_iter()
            .partition(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")));
        if !pdfs.is_empty() {
            self.org_insert(pdfs, at);
        }
        if let Some(p) = other.into_iter().next() {
            self.org_load_signature(p);
        }
    }

    pub(super) fn org_toolbar(&mut self, ui: &mut Ui, r: Rect) {
        let Some(org) = self.org.as_ref() else { return };
        let editing = org.editing.is_some();
        let n_sel = org.selected().len();
        let can_undo = !org.undo.is_empty();
        let mut row = r.inset_xy(16.0, 9.0);
        let btn = |ui: &mut Ui, row: &mut Rect, key: &str, label: &str, icon, on: bool| {
            let w = ui.measure(label, size::BODY, Weight::Semibold) + 44.0;
            let c = row.cut_left_gap(w, 6.0);
            widgets::button(
                ui,
                Id::new("org").with(key),
                c,
                label,
                Some(icon),
                ButtonKind::Secondary,
                on,
            )
            .clicked
        };
        if editing {
            if btn(ui, &mut row, "back", "All pages", Icon::ChevronLeft, true)
                && let Some(o) = self.org.as_mut()
            {
                o.editing = None;
                o.draft = None;
            }
            if btn(ui, &mut row, "undo", "Undo", Icon::Undo, can_undo)
                && let Some(o) = self.org.as_mut()
            {
                o.undo();
            }
            return;
        }
        if btn(ui, &mut row, "undo", "Undo", Icon::Undo, can_undo)
            && let Some(o) = self.org.as_mut()
        {
            o.undo();
        }
        if btn(ui, &mut row, "turn", "Rotate", Icon::Rotate, n_sel > 0) {
            let files = &self.files;
            if let Some(o) = self.org.as_mut() {
                let sel = o.selected();
                o.snapshot();
                o.turn(&sel, files);
            }
        }
        if btn(ui, &mut row, "dup", "Duplicate", Icon::Duplicate, n_sel > 0) {
            self.org_duplicate();
        }
        if btn(ui, &mut row, "del", "Delete", Icon::Trash, n_sel > 0) {
            self.org_delete();
        }
        if btn(ui, &mut row, "blank", "Blank page", Icon::BlankPage, true) {
            self.org_blank();
        }
        if btn(ui, &mut row, "ins", "Insert PDF…", Icon::Add, true)
            && let Some(o) = self.org.as_mut()
        {
            let at = o.selected().last().map_or(o.tiles.len(), |&i| i + 1);
            o.picking = Some(Pick::Insert(at));
            self.requests.push(Request::PickFiles);
        }
        if btn(ui, &mut row, "edit", "Edit page", Icon::Pen, n_sel > 0)
            && let Some(o) = self.org.as_mut()
        {
            o.editing = o.selected().first().copied();
        }
    }

    pub(super) fn org_view(&mut self, ui: &mut Ui, view: Rect) {
        let dropped = std::mem::take(&mut ui.input.dropped);
        if !dropped.is_empty() {
            self.org_dropped(dropped);
        }
        let Some(org) = self.org.as_ref() else {
            widgets::spinner(ui, view.center(), 14.0, ui.theme.accent);
            return;
        };
        if org.editing.is_some() {
            self.org_editor(ui, view);
        } else {
            self.org_grid(ui, view);
        }
    }

    fn org_grid(&mut self, ui: &mut Ui, view: Rect) {
        let t = ui.theme;
        let n = self.org.as_ref().map_or(0, |o| o.tiles.len());
        if n == 0 {
            return;
        }
        let (cols, cw) = swirvo_ui::geom::grid_columns(view.w - 12.0, CELL_W, GAP);
        let (pw, ph) = self
            .org
            .as_ref()
            .map_or((595.0, 842.0), |o| tile_size(&o.tiles[0], &self.files));
        let thumb_h = (cw - 16.0) * (ph / pw.max(1.0)).clamp(0.5, 1.8);
        let cell_h = thumb_h + 40.0;
        let rows = n.div_ceil(cols);
        let content = rows as f32 * (cell_h + GAP) + 8.0;
        let sid = Id::new("org-pages");
        let off = widgets::scroll_begin(ui, sid, view, content);
        let first_row = ((off / (cell_h + GAP)).floor() as usize).saturating_sub(1);
        let last_row = (((off + view.h) / (cell_h + GAP)).ceil() as usize + 1).min(rows);
        let pointer = ui.pointer();
        let (shift, ctrl) = (ui.input.mods.shift, ui.input.mods.ctrl);
        let mut want: Vec<Vec<usize>> = vec![Vec::new(); self.files.len()];
        // Where a drop would land: the gap nearest the pointer.
        let gap_at = |p: Point| -> usize {
            let row = ((p.y - view.y - 4.0 + off) / (cell_h + GAP))
                .floor()
                .max(0.0) as usize;
            let slot = ((p.x - view.x + GAP / 2.0) / (cw + GAP))
                .round()
                .clamp(0.0, cols as f32);
            (row * cols + slot as usize).min(n)
        };
        let dragging = self
            .org
            .as_ref()
            .and_then(|o| o.drag.as_ref())
            .is_some_and(|d| d.active);
        let drag_set: Vec<usize> = self.org.as_ref().map_or(Vec::new(), |o| match &o.drag {
            Some(d) if d.active => {
                if o.tiles[d.from].sel {
                    o.selected()
                } else {
                    vec![d.from]
                }
            }
            _ => Vec::new(),
        });
        let mut clicked: Option<usize> = None;
        let mut opened: Option<usize> = None;
        let mut pressed: Option<usize> = None;
        for row in first_row..last_row {
            for col in 0..cols {
                let i = row * cols + col;
                if i >= n {
                    break;
                }
                let c = Rect::new(
                    view.x + col as f32 * (cw + GAP),
                    view.y + 4.0 + row as f32 * (cell_h + GAP) - off,
                    cw,
                    cell_h,
                );
                let id = Id::new("tile").with(i);
                let resp = ui.interact(id, c);
                if resp.pressed {
                    pressed = Some(i);
                }
                if resp.clicked && !dragging {
                    clicked = Some(i);
                }
                if resp.double_clicked {
                    opened = Some(i);
                }
                let h = ui.anim(id.with("h"), if resp.hovered { 1.0 } else { 0.0 }, 22.0);
                let Some(org) = self.org.as_ref() else { return };
                let tile = &org.tiles[i];
                let on = tile.sel;
                let ghost = drag_set.contains(&i);
                if on {
                    ui.fill_rrect(c, size::R_CARD, t.accent_wash);
                } else if h > 0.0 {
                    ui.fill_rrect(c, size::R_CARD, t.card_hover.fade(h));
                }
                let page = Rect::new(c.x + 8.0, c.y + 8.0, c.w - 16.0, thumb_h);
                let (w, hh) = tile_size(tile, &self.files);
                let k = (page.w / w.max(1.0)).min(page.h / hh.max(1.0));
                let pr = page.centered(w * k, hh * k);
                ui.shadow(pr, 2.0, 6.0, 2.0, t.shadow);
                ui.fill_rect(pr, Rgba::WHITE);
                let label = match tile.src {
                    PageSrc::Page { file: 0, page } => (page + 1).to_string(),
                    PageSrc::Page { file, page } => {
                        let name = &self.files[file].name;
                        let short: String = name.chars().take(14).collect();
                        format!("{short} · {}", page + 1)
                    }
                    PageSrc::Blank { .. } => "Blank".into(),
                };
                let (crop, marks) = (tile.crop, tile.marks.clone());
                let src = tile.src;
                match self.org_page_picture(i, false) {
                    Some(pm) => ui.image(pr, &pm, 0.0, false),
                    None => {
                        if let PageSrc::Page { file, page } = src
                            && let Some(v) = want.get_mut(file)
                        {
                            v.push(page);
                        }
                    }
                }
                draw_marks(ui, View { pr, w, h: hh }, crop, &marks);
                if ghost {
                    ui.fill_rect(pr, t.accent_wash.fade(0.8));
                }
                if on {
                    ui.stroke_rrect_w(pr.inset(-3.0), 4.0, t.accent, 2.0);
                    let b = Rect::new(pr.right() - 26.0, pr.y + 6.0, 20.0, 20.0);
                    ui.fill_circle(b.center().x, b.center().y, 10.0, t.accent);
                    ui.icon(b.inset(3.0), Icon::Check, t.on_accent);
                }
                let lr = Rect::new(c.x, page.bottom() + 6.0, c.w, 22.0);
                let ink = if on { t.text } else { t.text2 };
                ui.text_in(lr, &label, size::BODY, Weight::Regular, ink, Align::Center);
                let count = marks.len() + usize::from(crop.is_some());
                if count > 0 {
                    let b = Rect::new(pr.x + 6.0, pr.y + 6.0, 22.0, 18.0);
                    ui.fill_rrect(b, 9.0, t.accent);
                    ui.text_in(
                        b,
                        &count.to_string(),
                        size::CAPTION,
                        Weight::Semibold,
                        t.on_accent,
                        Align::Center,
                    );
                }
            }
        }
        // The drop marker and the dragged pages under the pointer.
        if dragging && let Some(p) = pointer {
            let g = gap_at(p);
            let (row, col) = (g / cols, g % cols);
            let (row, col) = if col == 0 && g > 0 && g == n && n.is_multiple_of(cols) {
                (row - 1, cols)
            } else {
                (row, col)
            };
            let x = view.x + col as f32 * (cw + GAP) - GAP / 2.0;
            let y = view.y + 4.0 + row as f32 * (cell_h + GAP) - off;
            ui.fill_rrect(Rect::new(x - 1.5, y, 3.0, cell_h), 1.5, t.accent);
            let badge = Rect::new(p.x + 12.0, p.y + 8.0, 96.0, 24.0);
            ui.fill_rrect(badge, 12.0, t.accent);
            ui.text_in(
                badge,
                &pages(drag_set.len()),
                size::CAPTION,
                Weight::Semibold,
                t.on_accent,
                Align::Center,
            );
            // Scroll when held near the top or bottom edge.
            if p.y < view.y + 30.0 {
                widgets::scroll_to(ui, sid, (off - 14.0).max(0.0));
                ui.request_repaint(0.016);
            } else if p.y > view.bottom() - 30.0 {
                widgets::scroll_to(ui, sid, off + 14.0);
                ui.request_repaint(0.016);
            }
        }
        widgets::scroll_end(ui, sid, view, content);
        for (k, mut v) in want.into_iter().enumerate() {
            if !v.is_empty() {
                let mid = v[v.len() / 2];
                v.sort_by_key(|&p| p.abs_diff(mid));
                self.files[k].worker.request(v);
            }
        }

        // Pointer: press, drag, release.
        let (down, released) = (ui.input.down, ui.input.released);
        let Some(org) = self.org.as_mut() else { return };
        if let (Some(i), Some(p)) = (pressed, pointer) {
            org.drag = Some(Drag {
                from: i,
                start: p,
                active: false,
            });
        }
        if let (Some(d), Some(p)) = (org.drag.as_mut(), pointer)
            && down
            && !d.active
            && ((p.x - d.start.x).powi(2) + (p.y - d.start.y).powi(2)).sqrt() > DRAG_SLOP
        {
            d.active = true;
        }
        if released {
            if let Some(d) = org.drag.take()
                && d.active
                && let Some(p) = pointer
            {
                let to = gap_at(p);
                org.snapshot();
                org.move_to(&drag_set, to);
                return;
            }
            if let Some(i) = clicked {
                match (shift, org.anchor) {
                    (true, Some(a)) if a < org.tiles.len() => {
                        let v = org.tiles[a].sel;
                        for j in a.min(i)..=a.max(i) {
                            org.tiles[j].sel = v;
                        }
                    }
                    _ => {
                        if !ctrl && !org.tiles[i].sel {
                            // A plain click selects just this page; ctrl adds to it.
                            for t in &mut org.tiles {
                                t.sel = false;
                            }
                        }
                        org.tiles[i].sel = !org.tiles[i].sel || !ctrl && org.selected().len() > 1;
                        org.anchor = Some(i);
                    }
                }
            }
        }
        if let Some(i) = opened {
            org.editing = Some(i);
            org.draft = None;
        }
    }

    fn org_editor(&mut self, ui: &mut Ui, view: Rect) {
        let t = ui.theme;
        let Some(i) = self.org.as_ref().and_then(|o| o.editing) else {
            return;
        };
        let mut area = view;
        let panel = area.cut_right(PANEL_W);
        area.cut_right(12.0);
        let Some(org) = self.org.as_ref() else { return };
        let tile = org.tiles[i].clone();
        let (w, h) = tile_size(&tile, &self.files);
        let fit = area.inset(16.0);
        let k = (fit.w / w.max(1.0)).min(fit.h / h.max(1.0));
        let pr = fit.centered(w * k, h * k);
        let v = View { pr, w, h };
        ui.shadow(pr, 2.0, 10.0, 3.0, t.shadow);
        ui.fill_rect(pr, Rgba::WHITE);
        if let PageSrc::Page { file, page } = tile.src {
            let have_big = self.files[file]
                .big
                .as_ref()
                .is_some_and(|(p, _)| *p == page);
            if !have_big {
                self.files[file].worker.request_big(page);
            }
            if !self.files[file].thumbs.contains_key(&page) {
                self.files[file].worker.request(vec![page]);
            }
            if let Some(pm) = self.org_page_picture(i, true) {
                ui.image(pr, &pm, 0.0, false);
            }
        }
        let Some(org) = self.org.as_ref() else { return };
        draw_marks(ui, v, tile.crop, &tile.marks);
        // The shape being drawn.
        let color = PALETTE[org.color];
        match &org.draft {
            Some(Draft::Rect(a, b)) => {
                let r = v.rect(norm(*a, *b));
                let c = match org.tool {
                    EditTool::Redact => Rgba::BLACK.fade(0.6),
                    EditTool::Crop => Rgba::WHITE,
                    _ => rgba(color),
                };
                ui.stroke_rrect_w(r, 0.0, c, 1.5);
                if org.tool == EditTool::Highlight {
                    ui.fill_rect(r, rgba(color).fade(0.35));
                }
            }
            Some(Draft::Ink(pts)) => {
                let marks = [UiMark::Comment(Comment::Ink {
                    strokes: vec![pts.clone()],
                    color,
                    width: 2.0,
                })];
                draw_marks(ui, v, None, &marks);
            }
            None => {}
        }

        // Pointer on the page.
        let resp = ui.interact(Id::new("editor-page"), pr);
        if resp.hovered {
            ui.cursor = swirvo_ui::Cursor::Arrow;
        }
        let pointer = ui.pointer();
        let released = ui.input.released;
        let Some(org) = self.org.as_mut() else { return };
        if resp.pressed
            && let Some(p) = pointer
        {
            let q = v.page_pt(p);
            org.draft = Some(match org.tool {
                EditTool::Ink => Draft::Ink(vec![q]),
                _ => Draft::Rect(q, q),
            });
        }
        if resp.down
            && let Some(p) = pointer
        {
            let q = v.page_pt(p);
            match &mut org.draft {
                Some(Draft::Rect(_, b)) => *b = q,
                Some(Draft::Ink(pts)) => {
                    let last = pts.last().copied().unwrap_or(q);
                    if (last[0] - q[0]).abs() + (last[1] - q[1]).abs() > 1.0 / k.max(0.01) {
                        pts.push(q);
                    }
                }
                None => {}
            }
        }
        if released && let Some(d) = org.draft.take() {
            org_finish(org, i, d, (w, h));
        }
        self.org_panel(ui, panel, i, (w, h));
    }

    fn org_panel(&mut self, ui: &mut Ui, r: Rect, i: usize, (w, h): (f32, f32)) {
        let t = ui.theme;
        ui.glass(r, size::R_PANEL, t.panel, 14.0);
        let mut col = r.inset_xy(14.0, 12.0);
        let n = self.org.as_ref().map_or(0, |o| o.tiles.len());
        // Page N of M with previous and next.
        let mut head = col.cut_top_gap(32.0, 10.0);
        let next = head.cut_right(32.0);
        let prev = head.cut_right(32.0);
        ui.text_in(
            head,
            &format!("Page {} of {n}", i + 1),
            size::BODY_L,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let go_prev = widgets::icon_button(
            ui,
            Id::new("ed-prev"),
            prev,
            Icon::ChevronLeft,
            "Previous page",
        )
        .clicked;
        let go_next = widgets::icon_button(
            ui,
            Id::new("ed-next"),
            next,
            Icon::ChevronRight,
            "Next page",
        )
        .clicked;
        // Tools, two per row.
        let current = self.org.as_ref().map_or(EditTool::Highlight, |o| o.tool);
        let mut chosen = None;
        for pair in TOOLS.chunks(2) {
            let mut row = col.cut_top_gap(34.0, 6.0);
            let half = (row.w - 6.0) / 2.0;
            for (k, &(tool, label, icon)) in pair.iter().enumerate() {
                let c = if k == 0 {
                    row.cut_left_gap(half, 6.0)
                } else {
                    row
                };
                let kind = if tool == current {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Secondary
                };
                if widgets::button(
                    ui,
                    Id::new("ed-tool").with(label),
                    c,
                    label,
                    Some(icon),
                    kind,
                    true,
                )
                .clicked
                {
                    chosen = Some(tool);
                }
            }
        }
        col.cut_top(6.0);
        let mut pick_sig = false;
        let mut clear_crop = false;
        let turn;
        {
            let Some(org) = self.org.as_mut() else { return };
            if let Some(tool) = chosen {
                org.tool = tool;
                org.hint = None;
            }
            let tool = org.tool;
            let help = match tool {
                EditTool::Highlight => "Drag over text to highlight it.",
                EditTool::Note => "Type the note, then click where it goes.",
                EditTool::Text => "Type the text, then drag a box for it.",
                EditTool::Ink => "Draw on the page.",
                EditTool::Box => "Drag to draw a box.",
                EditTool::Sign => {
                    "Choose a picture of your signature, then drag where it goes. This adds a picture, not a digital signature."
                }
                EditTool::Redact => {
                    "Drag over what to remove. The page is rebuilt from pixels so nothing under the box survives."
                }
                EditTool::Crop => "Drag the part of the page to keep.",
            };
            let hr = col.cut_top_gap(40.0, 8.0);
            ui.text_in(
                hr,
                help,
                size::CAPTION,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
            if matches!(
                tool,
                EditTool::Highlight
                    | EditTool::Note
                    | EditTool::Text
                    | EditTool::Ink
                    | EditTool::Box
            ) {
                let mut row = col.cut_top_gap(26.0, 8.0);
                for (k, c) in PALETTE.iter().enumerate() {
                    let s = row.cut_left_gap(26.0, 8.0);
                    let resp = ui.interact(Id::new("swatch").with(k), s);
                    ui.fill_circle(s.center().x, s.center().y, 11.0, rgba(*c));
                    if k == org.color {
                        ui.stroke_rrect_w(s.inset(-2.0), 15.0, t.accent, 2.0);
                    }
                    if resp.clicked {
                        org.color = k;
                    }
                }
            }
            if matches!(tool, EditTool::Note | EditTool::Text) {
                let fr = col.cut_top_gap(34.0, 8.0);
                widgets::text_field(ui, Id::new("ed-text"), fr, &mut org.text, "Text", false);
            }
            if tool == EditTool::Sign {
                let br = col.cut_top_gap(34.0, 8.0);
                pick_sig = widgets::button(
                    ui,
                    Id::new("ed-sig"),
                    br,
                    if org.signature.is_some() {
                        "Choose another picture…"
                    } else {
                        "Choose picture…"
                    },
                    Some(Icon::Image),
                    ButtonKind::Secondary,
                    org.loading.is_none(),
                )
                .clicked;
                if let Some((_, pm)) = &org.signature {
                    let pv = col.cut_top_gap(60.0, 8.0);
                    ui.fill_rrect(pv, 6.0, Rgba::WHITE);
                    let pm = pm.clone();
                    ui.image(pv.inset(6.0), &pm, 0.0, false);
                }
            }
            if tool == EditTool::Crop && org.tiles[i].crop.is_some() {
                let br = col.cut_top_gap(34.0, 8.0);
                clear_crop = widgets::button(
                    ui,
                    Id::new("ed-uncrop"),
                    br,
                    "Remove the crop",
                    Some(Icon::Close),
                    ButtonKind::Secondary,
                    true,
                )
                .clicked;
            }
            let br = col.cut_top_gap(34.0, 14.0);
            turn = widgets::button(
                ui,
                Id::new("ed-turn"),
                br,
                "Rotate this page",
                Some(Icon::Rotate),
                ButtonKind::Secondary,
                true,
            )
            .clicked;
            // What's on this page.
            let marks: Vec<&'static str> = org.tiles[i].marks.iter().map(UiMark::label).collect();
            if !marks.is_empty() {
                let lr = col.cut_top_gap(20.0, 4.0);
                ui.text_in(
                    lr,
                    "On this page",
                    size::CAPTION,
                    Weight::Semibold,
                    t.text2,
                    Align::Left,
                );
            }
            let mut remove = None;
            for (k, label) in marks.iter().enumerate() {
                if col.h < 60.0 {
                    break;
                }
                let mut row = col.cut_top_gap(28.0, 2.0);
                let x = row.cut_right(28.0);
                ui.text_in(row, label, size::BODY, Weight::Regular, t.text, Align::Left);
                if widgets::icon_button(ui, Id::new("ed-rm").with(k), x, Icon::Trash, "Remove")
                    .clicked
                {
                    remove = Some(k);
                }
            }
            if let Some(k) = remove {
                org.snapshot();
                org.tiles[i].marks.remove(k);
            }
            if let Some(hint) = &org.hint {
                let hr = Rect::new(col.x, r.bottom() - 52.0, col.w, 40.0);
                ui.text_in(
                    hr,
                    hint,
                    size::CAPTION,
                    Weight::Regular,
                    t.warning,
                    Align::Left,
                );
            }
        }
        let files = &self.files;
        let Some(org) = self.org.as_mut() else { return };
        if clear_crop {
            org.snapshot();
            org.tiles[i].crop = None;
        }
        if turn {
            org.snapshot();
            org.turn(&[i], files);
        }
        if go_prev && i > 0 {
            org.editing = Some(i - 1);
            org.draft = None;
        }
        if go_next && i + 1 < n {
            org.editing = Some(i + 1);
            org.draft = None;
        }
        if pick_sig {
            org.picking = Some(Pick::Signature);
            self.requests.push(Request::PickFiles);
        }
        let _ = (w, h);
    }

    pub(super) fn save_organised(&mut self) {
        if self.running.is_some() {
            return;
        }
        let Some(org) = self.org.as_ref() else { return };
        let (sheets, marks) = org.sheets_and_marks();
        let inputs: Vec<PathBuf> = self.files.iter().map(|f| f.path.clone()).collect();
        let (tx, rx) = channel();
        let cancel = Arc::new(CancelToken::new());
        let (tk, waker, c) = (self.tk.clone(), self.waker.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = swirvo_jobs::worker::run_organise(&tk, &inputs, &sheets, &marks, &c)
                .map_err(|e| e.to_string());
            let _ = tx.send(r);
            waker();
        });
        self.result = None;
        self.running = Some((rx, cancel));
    }

    /// The footer's summary in Organise mode.
    pub(super) fn org_summary(&self) -> String {
        match &self.org {
            Some(o) => {
                let m = o.mark_count();
                if m == 0 {
                    pages(o.tiles.len())
                } else {
                    format!("{} · {m} changes", pages(o.tiles.len()))
                }
            }
            None => String::new(),
        }
    }
}

/// Turn a finished drag into a mark on tile `i` (page size `w`×`h` points).
fn org_finish(org: &mut Organiser, i: usize, d: Draft, (w, h): (f32, f32)) {
    let color = PALETTE[org.color];
    let mark = match d {
        Draft::Ink(pts) => {
            if pts.len() < 2 {
                return;
            }
            UiMark::Comment(Comment::Ink {
                strokes: vec![pts],
                color,
                width: 2.0,
            })
        }
        Draft::Rect(a, b) => {
            let r = norm(a, b);
            let small = r[2] - r[0] < 6.0 || r[3] - r[1] < 6.0;
            match org.tool {
                EditTool::Crop => {
                    if r[2] - r[0] < 20.0 || r[3] - r[1] < 20.0 {
                        return;
                    }
                    org.snapshot();
                    org.tiles[i].crop = Some(r);
                    return;
                }
                EditTool::Redact if !small => UiMark::Redact(r),
                EditTool::Highlight if !small => {
                    UiMark::Comment(Comment::Highlight { rect: r, color })
                }
                EditTool::Box if !small => UiMark::Comment(Comment::Box {
                    rect: r,
                    color,
                    width: 2.0,
                }),
                EditTool::Note => {
                    if org.text.trim().is_empty() {
                        org.hint = Some("Type the note first".into());
                        return;
                    }
                    UiMark::Comment(Comment::Note {
                        at: [b[0].min(w - 20.0), b[1].max(20.0)],
                        text: std::mem::take(&mut org.text),
                        color,
                    })
                }
                EditTool::Text => {
                    if org.text.trim().is_empty() {
                        org.hint = Some("Type the text first".into());
                        return;
                    }
                    let rect = if small {
                        [b[0], (b[1] - 30.0).max(0.0), (b[0] + 220.0).min(w), b[1]]
                    } else {
                        r
                    };
                    UiMark::Comment(Comment::Text {
                        rect,
                        text: std::mem::take(&mut org.text),
                        size: 12.0,
                        color,
                    })
                }
                EditTool::Sign => {
                    let Some((bytes, pm)) = org.signature.clone() else {
                        org.hint = Some("Choose a picture of your signature first".into());
                        return;
                    };
                    let aspect = pm.height() as f32 / pm.width().max(1) as f32;
                    let rect = if small {
                        let sw = 150.0f32.min(w * 0.5);
                        let sh = sw * aspect;
                        [
                            b[0] - sw / 2.0,
                            b[1] - sh / 2.0,
                            b[0] + sw / 2.0,
                            b[1] + sh / 2.0,
                        ]
                    } else {
                        // Fit the picture inside the box, as it will be drawn.
                        let (bw, bh) = (r[2] - r[0], r[3] - r[1]);
                        let (fw, fh) = if bh / bw > aspect {
                            (bw, bw * aspect)
                        } else {
                            (bh / aspect, bh)
                        };
                        let (cx, cy) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
                        [cx - fw / 2.0, cy - fh / 2.0, cx + fw / 2.0, cy + fh / 2.0]
                    };
                    let rect = [
                        rect[0].clamp(0.0, w),
                        rect[1].clamp(0.0, h),
                        rect[2].clamp(0.0, w),
                        rect[3].clamp(0.0, h),
                    ];
                    UiMark::Picture(rect, bytes, pm)
                }
                _ => return,
            }
        }
    };
    org.snapshot();
    org.hint = None;
    org.tiles[i].marks.push(mark);
}

/// For tests and snapshots: drive Organise mode without a pointer.
impl PagePicker {
    /// Switch to Organise mode (once the page count is known).
    pub fn organise(&mut self) {
        self.op = super::ORGANISE;
        if self.org.is_none() && !self.sizes.is_empty() {
            self.org = Some(Organiser::new(self.sizes.len()));
        }
    }

    pub fn org_len(&self) -> usize {
        self.org.as_ref().map_or(0, |o| o.tiles.len())
    }

    /// Move tiles `which` to gap `to`, as a drag does.
    pub fn org_move(&mut self, which: &[usize], to: usize) {
        if let Some(o) = self.org.as_mut() {
            o.snapshot();
            o.move_to(which, to);
        }
    }

    pub fn org_select(&mut self, which: &[usize]) {
        if let Some(o) = self.org.as_mut() {
            for (i, t) in o.tiles.iter_mut().enumerate() {
                t.sel = which.contains(&i);
            }
        }
    }

    pub fn org_rotate_selected(&mut self) {
        let files = &self.files;
        if let Some(o) = self.org.as_mut() {
            let sel = o.selected();
            o.snapshot();
            o.turn(&sel, files);
        }
    }

    pub fn org_add_blank(&mut self) {
        self.org_blank();
    }

    pub fn org_duplicate_selected(&mut self) {
        self.org_duplicate();
    }

    pub fn org_delete_selected(&mut self) {
        self.org_delete();
    }

    pub fn org_undo(&mut self) {
        if let Some(o) = self.org.as_mut() {
            o.undo();
        }
    }

    /// Black out `rect` (page points, y up) on tile `i`.
    pub fn org_redact(&mut self, i: usize, rect: Box4) {
        if let Some(o) = self.org.as_mut() {
            o.snapshot();
            o.tiles[i].marks.push(UiMark::Redact(rect));
        }
    }

    pub fn org_comment(&mut self, i: usize, c: Comment) {
        if let Some(o) = self.org.as_mut() {
            o.snapshot();
            o.tiles[i].marks.push(UiMark::Comment(c));
        }
    }

    pub fn org_crop(&mut self, i: usize, rect: Box4) {
        if let Some(o) = self.org.as_mut() {
            o.snapshot();
            o.tiles[i].crop = Some(rect);
        }
    }

    /// Open the page editor on tile `i` (None: back to the grid).
    pub fn org_edit(&mut self, i: Option<usize>) {
        if let Some(o) = self.org.as_mut() {
            o.editing = i;
        }
    }

    /// Insert `pdf` at gap `at`; its pages arrive once it has been opened.
    pub fn org_insert_pdf(&mut self, pdf: PathBuf, at: usize) {
        self.org_insert(vec![pdf], at);
    }

    /// Which page each tile shows: (file, page), or None for a blank page.
    pub fn org_order(&self) -> Vec<Option<(usize, usize)>> {
        self.org.as_ref().map_or(Vec::new(), |o| {
            o.tiles
                .iter()
                .map(|t| match t.src {
                    PageSrc::Page { file, page } => Some((file, page)),
                    PageSrc::Blank { .. } => None,
                })
                .collect()
        })
    }

    /// For tests: poll until inserted PDFs have arrived.
    pub fn org_settle(&mut self) {
        for _ in 0..400 {
            self.poll();
            if self.org.as_ref().is_none_or(|o| o.waiting.is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(page: usize) -> Tile {
        Tile {
            src: PageSrc::Page { file: 0, page },
            turn: 0,
            crop: None,
            marks: Vec::new(),
            sel: false,
        }
    }

    fn order(o: &Organiser) -> Vec<usize> {
        o.tiles
            .iter()
            .map(|t| match t.src {
                PageSrc::Page { page, .. } => page,
                PageSrc::Blank { .. } => 99,
            })
            .collect()
    }

    #[test]
    fn moving_pages_lands_them_in_the_gap() {
        let mut o = Organiser::new(0);
        o.tiles = (0..6).map(tile).collect();
        o.move_to(&[4], 1);
        assert_eq!(order(&o), [0, 4, 1, 2, 3, 5]);
        o.move_to(&[0, 1], 6);
        assert_eq!(order(&o), [1, 2, 3, 5, 0, 4]);
        o.move_to(&[2, 4], 0);
        assert_eq!(order(&o), [3, 0, 1, 2, 5, 4]);
        // Dropping a page in the gap it came from changes nothing.
        o.move_to(&[2], 2);
        assert_eq!(order(&o), [3, 0, 1, 2, 5, 4]);
        o.move_to(&[2], 3);
        assert_eq!(order(&o), [3, 0, 1, 2, 5, 4]);
    }

    #[test]
    fn marks_turn_with_the_page() {
        // A4 portrait: a box in the top-left corner ends in the top-right corner.
        let m = UiMark::Redact([10.0, 800.0, 60.0, 830.0]).turned(595.0);
        let UiMark::Redact(r) = m else { unreachable!() };
        // Turned page is 842 × 595: top-right is x near 842, y near 595.
        assert_eq!(r, [800.0, 535.0, 830.0, 585.0]);
        // Four quarter turns bring it back.
        let mut m = UiMark::Redact([10.0, 800.0, 60.0, 830.0]);
        let mut w = 595.0;
        for _ in 0..4 {
            m = m.turned(w);
            w = if w == 595.0 { 842.0 } else { 595.0 };
        }
        let UiMark::Redact(r) = m else { unreachable!() };
        assert_eq!(r, [10.0, 800.0, 60.0, 830.0]);
    }

    #[test]
    fn pixmaps_turn_clockwise() {
        let mut pm = Pixmap::new(3, 2).unwrap();
        // Mark the top-left pixel red (premultiplied RGBA bytes).
        pm.data_mut()[..4].copy_from_slice(&[255, 0, 0, 255]);
        let red = |p: &Pixmap, i: usize| p.data()[i * 4] == 255;
        let t = turn_pixmap(&pm, 90).unwrap();
        assert_eq!((t.width(), t.height()), (2, 3));
        // Clockwise: top-left goes to top-right.
        assert!(red(&t, 1));
        let t = turn_pixmap(&pm, 180).unwrap();
        assert!(red(&t, 5));
        let t = turn_pixmap(&pm, 270).unwrap();
        assert!(red(&t, 4));
    }
}
