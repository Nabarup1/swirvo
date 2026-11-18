//! The page picker (docs/12 §4): choose pages of one PDF, then export, extract, split,
//! rotate or delete them, or organise them by hand (`organise`). Every operation writes
//! new files; the PDF is never changed.

mod organise;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};

use swirvo_core::CancelToken;
use swirvo_jobs::Toolkit;
use swirvo_jobs::args::ToolArgs;
use swirvo_jobs::pages;
use swirvo_jobs::tools::{PageOp, Tool, ToolResult};
use swirvo_jobs::worker::{run_page_op, run_tool_with};
use swirvo_ui::theme::size;
use swirvo_ui::widgets::{self, ButtonKind};
use swirvo_ui::{
    Align, App, HostEvent, Icon, Id, Key, Kind, Pixmap, Rect, Request, Rgba, Ui, Waker, Weight,
};

use crate::file_name;
use crate::thumbs::to_pixmap;

const TITLE_H: f32 = 48.0;
const TOOLBAR_H: f32 = 52.0;
const FOOTER_H: f32 = 76.0;
const CELL_W: f32 = 148.0;
const GAP: f32 = 14.0;
const THUMB_PX: (u32, u32) = (260, 360);
/// The page editor's render, fitted into this many pixels.
const BIG_PX: (u32, u32) = (1400, 1400);

const OPS: [&str; 7] = [
    "JPG", "PNG", "New PDF", "Split", "Rotate", "Delete", "Organise",
];
/// The segment that switches to Organise mode.
const ORGANISE: usize = 6;
/// The Split segment, which can split per page or into groups.
const SPLIT: usize = 3;

fn op_index(op: PageOp) -> usize {
    match op {
        PageOp::Images { png: false, .. } => 0,
        PageOp::Images { png: true, .. } => 1,
        PageOp::Extract => 2,
        PageOp::Split => 3,
        PageOp::Rotate => 4,
        PageOp::Delete => 5,
    }
}

enum Msg {
    Info(Vec<(f32, f32)>),
    Page(usize, image::RgbaImage),
    Big(usize, image::RgbaImage),
    Error(String),
}

struct Want {
    pages: VecDeque<usize>,
    /// One page for the editor, rendered large; served before thumbnails.
    big: Option<usize>,
    quit: bool,
}

/// Renders page thumbnails on one thread that parses the PDF once.
struct PageWorker {
    want: Arc<(Mutex<Want>, Condvar)>,
    rx: Receiver<Msg>,
}

impl PageWorker {
    fn start(path: PathBuf, waker: Waker) -> PageWorker {
        let want = Arc::new((
            Mutex::new(Want {
                pages: VecDeque::new(),
                big: None,
                quit: false,
            }),
            Condvar::new(),
        ));
        let (tx, rx) = channel();
        let w = want.clone();
        std::thread::spawn(move || {
            let r = match swirvo_engines::pdf_tools::PageRenderer::open(&path) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Msg::Error(e.to_string()));
                    waker();
                    return;
                }
            };
            let sizes: Vec<(f32, f32)> = (0..r.count()).filter_map(|i| r.size(i)).collect();
            let _ = tx.send(Msg::Info(sizes));
            waker();
            r.session(|s| {
                loop {
                    let (next, big) = {
                        let (m, cv) = &*w;
                        let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
                        loop {
                            if g.quit {
                                return;
                            }
                            if let Some(i) = g.big.take() {
                                break (i, true);
                            }
                            if let Some(i) = g.pages.pop_front() {
                                break (i, false);
                            }
                            g = cv.wait(g).unwrap_or_else(|p| p.into_inner());
                        }
                    };
                    let (pw, ph) = if big { BIG_PX } else { THUMB_PX };
                    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        s.render_fit(next, pw, ph)
                    }));
                    if let Ok(Ok(img)) = img {
                        let m = if big {
                            Msg::Big(next, img)
                        } else {
                            Msg::Page(next, img)
                        };
                        if tx.send(m).is_err() {
                            return;
                        }
                        waker();
                    }
                }
            });
        });
        PageWorker { want, rx }
    }

    /// Replace the queue: these pages, nearest the viewport center first.
    fn request(&self, pages: Vec<usize>) {
        let (m, cv) = &*self.want;
        let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
        g.pages = pages.into();
        drop(g);
        cv.notify_one();
    }

    /// Render page `i` large next, ahead of the thumbnails.
    fn request_big(&self, i: usize) {
        let (m, cv) = &*self.want;
        let mut g = m.lock().unwrap_or_else(|p| p.into_inner());
        g.big = Some(i);
        drop(g);
        cv.notify_one();
    }
}

/// One open PDF: its renderer thread, page sizes and the pictures made so far.
struct FileView {
    path: PathBuf,
    name: String,
    worker: PageWorker,
    sizes: Vec<(f32, f32)>,
    thumbs: HashMap<usize, Pixmap>,
    /// The latest large render (page, picture).
    big: Option<(usize, Pixmap)>,
    error: Option<String>,
}

impl FileView {
    fn start(path: PathBuf, waker: Waker) -> FileView {
        FileView {
            worker: PageWorker::start(path.clone(), waker),
            name: file_name(&path),
            path,
            sizes: Vec::new(),
            thumbs: HashMap::new(),
            big: None,
            error: None,
        }
    }

    /// Take what the worker sent. True when the page sizes arrived just now.
    fn poll(&mut self) -> bool {
        let mut info = false;
        while let Ok(m) = self.worker.rx.try_recv() {
            match m {
                Msg::Info(sizes) => {
                    self.sizes = sizes;
                    info = true;
                }
                Msg::Page(i, img) => {
                    if let Some(pm) = to_pixmap(&img) {
                        self.thumbs.insert(i, pm);
                    }
                }
                Msg::Big(i, img) => {
                    if let Some(pm) = to_pixmap(&img) {
                        self.big = Some((i, pm));
                    }
                }
                Msg::Error(e) => self.error = Some(e),
            }
        }
        info
    }
}

impl Drop for PageWorker {
    fn drop(&mut self) {
        let (m, cv) = &*self.want;
        if let Ok(mut g) = m.lock() {
            g.quit = true;
        }
        cv.notify_all();
    }
}

type Outcome = Result<ToolResult, String>;

pub struct PagePicker {
    tk: Arc<Toolkit>,
    waker: Waker,
    path: PathBuf,
    name: String,
    /// The PDF (0) and any inserted in Organise mode.
    files: Vec<FileView>,
    sizes: Vec<(f32, f32)>,
    sel: Vec<bool>,
    anchor: Option<usize>,
    range: String,
    range_err: Option<String>,
    op: usize,
    dpi: usize,
    /// Split: 0 a PDF per selected page, 1 a PDF per group.
    split_mode: usize,
    /// Split groups: which pages each output PDF gets (one flag per page).
    groups: Vec<Vec<bool>>,
    /// The group that clicks add pages to.
    active: usize,
    groups_text: String,
    groups_err: Option<String>,
    /// Groups typed before the page count was known (from the Board's form).
    pending_groups: Option<String>,
    /// A press-and-drag over pages: (page pressed, value it set, the flags before).
    swipe: Option<(usize, bool, Vec<bool>)>,
    org: Option<organise::Organiser>,
    running: Option<(Receiver<Outcome>, Arc<CancelToken>)>,
    result: Option<Result<ToolResult, String>>,
    requests: Vec<Request>,
    icon: Option<Pixmap>,
}

impl PagePicker {
    pub fn new(tk: Arc<Toolkit>, waker: Waker, path: PathBuf, op: PageOp) -> PagePicker {
        let main = FileView::start(path.clone(), waker.clone());
        let dpi = match op {
            PageOp::Images { dpi, .. } if dpi >= 300 => 1,
            _ => 0,
        };
        PagePicker {
            tk,
            waker,
            name: file_name(&path),
            path,
            files: vec![main],
            sizes: Vec::new(),
            sel: Vec::new(),
            anchor: None,
            range: "all".into(),
            range_err: None,
            op: op_index(op),
            dpi,
            split_mode: 0,
            groups: Vec::new(),
            active: 0,
            groups_text: String::new(),
            groups_err: None,
            pending_groups: None,
            swipe: None,
            org: None,
            running: None,
            result: None,
            requests: Vec::new(),
            icon: crate::app_icon(64),
        }
    }

    fn count(&self) -> usize {
        self.sizes.len()
    }

    fn selected(&self) -> Vec<usize> {
        (0..self.sel.len()).filter(|&i| self.sel[i]).collect()
    }

    fn page_op(&self) -> PageOp {
        let dpi = if self.dpi == 1 { 300 } else { 150 };
        match self.op {
            0 => PageOp::Images { png: false, dpi },
            1 => PageOp::Images { png: true, dpi },
            2 => PageOp::Extract,
            3 => PageOp::Split,
            4 => PageOp::Rotate,
            _ => PageOp::Delete,
        }
    }

    fn sync_text(&mut self) {
        let s = self.selected();
        self.range = if s.len() == self.count() && !s.is_empty() {
            "all".into()
        } else {
            pages::format(&s)
        };
        self.range_err = None;
    }

    fn poll(&mut self) {
        let mut arrived = Vec::new();
        for (k, f) in self.files.iter_mut().enumerate() {
            if f.poll() {
                arrived.push(k);
            }
        }
        if arrived.contains(&0) {
            self.sizes = self.files[0].sizes.clone();
            self.sel = vec![true; self.sizes.len()];
            self.sync_text();
            self.groups = vec![vec![false; self.sizes.len()]];
            self.active = 0;
            self.sync_groups();
            if let Some(spec) = self.pending_groups.take() {
                self.groups_text = spec;
                self.set_groups_text();
            }
        }
        if let Some(org) = &mut self.org {
            org.poll(&self.files, &arrived);
        }
        if let Some((rx, _)) = &self.running
            && let Ok(r) = rx.try_recv()
        {
            self.result = Some(r);
            self.running = None;
        }
    }

    /// The page flags clicks change: the active group's when grouping, else the selection.
    fn flags(&mut self) -> &mut Vec<bool> {
        if self.grouping() && !self.groups.is_empty() {
            let k = self.active.min(self.groups.len() - 1);
            &mut self.groups[k]
        } else {
            &mut self.sel
        }
    }

    /// After [`Self::flags`] changed: the text field follows.
    fn synced(&mut self) {
        if self.grouping() {
            self.sync_groups();
        } else {
            self.sync_text();
        }
    }

    /// Splitting into groups (the Split segment, Groups mode)?
    fn grouping(&self) -> bool {
        self.op == SPLIT && self.split_mode == 1
    }

    /// The non-empty groups, as 0-based page lists.
    fn group_pages(&self) -> Vec<Vec<usize>> {
        self.groups
            .iter()
            .map(|g| (0..g.len()).filter(|&i| g[i]).collect::<Vec<_>>())
            .filter(|g| !g.is_empty())
            .collect()
    }

    fn sync_groups(&mut self) {
        let parts: Vec<String> = self
            .groups
            .iter()
            .map(|g| pages::format(&(0..g.len()).filter(|&i| g[i]).collect::<Vec<_>>()))
            .collect();
        // Keep empty groups visible while they're being filled, except trailing ones.
        let mut text = parts.join("; ");
        while text.ends_with("; ") {
            text.truncate(text.len() - 2);
        }
        self.groups_text = text;
        self.groups_err = None;
    }

    fn set_groups_text(&mut self) {
        let n = self.count();
        match pages::parse_groups(&self.groups_text, n) {
            Ok(gs) => {
                self.groups = gs
                    .into_iter()
                    .map(|g| {
                        let mut v = vec![false; n];
                        for i in g {
                            v[i] = true;
                        }
                        v
                    })
                    .collect();
                self.active = self.active.min(self.groups.len().saturating_sub(1));
                self.groups_err = None;
            }
            Err(e) => self.groups_err = Some(e),
        }
    }

    fn add_group(&mut self) {
        if self.groups.len() < 1000 {
            self.groups.push(vec![false; self.count()]);
            self.active = self.groups.len() - 1;
        }
    }

    fn remove_group(&mut self, k: usize) {
        if k < self.groups.len() {
            self.groups.remove(k);
        }
        if self.groups.is_empty() {
            self.groups.push(vec![false; self.count()]);
        }
        self.active = self.active.min(self.groups.len() - 1);
        self.sync_groups();
    }

    fn run_groups(&mut self) {
        let groups = self.group_pages();
        if groups.is_empty() || self.running.is_some() {
            return;
        }
        let mut args = ToolArgs::new();
        args.insert("ranges".into(), pages::format_groups(&groups));
        let (tx, rx) = channel();
        let cancel = Arc::new(CancelToken::new());
        let (tk, path, waker, c) = (
            self.tk.clone(),
            self.path.clone(),
            self.waker.clone(),
            cancel.clone(),
        );
        std::thread::spawn(move || {
            let r = run_tool_with(&tk, Tool::PdfSplitRanges, &[path], &args, &c)
                .map_err(|e| e.to_string())
                .and_then(|r| match r.failures.first() {
                    Some((_, e)) => Err(e.clone()),
                    None => Ok(r),
                });
            let _ = tx.send(r);
            waker();
        });
        self.result = None;
        self.running = Some((rx, cancel));
    }

    fn run(&mut self) {
        if self.op == ORGANISE {
            self.save_organised();
            return;
        }
        if self.grouping() {
            self.run_groups();
            return;
        }
        let pages = self.selected();
        if pages.is_empty() || self.running.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(CancelToken::new());
        let (tk, path, op, waker, c) = (
            self.tk.clone(),
            self.path.clone(),
            self.page_op(),
            self.waker.clone(),
            cancel.clone(),
        );
        std::thread::spawn(move || {
            let r = run_page_op(&tk, &path, op, &pages, &c).map_err(|e| e.to_string());
            let _ = tx.send(r);
            waker();
        });
        self.result = None;
        self.running = Some((rx, cancel));
    }

    fn set_all(&mut self, f: impl Fn(usize, bool) -> bool) {
        for i in 0..self.sel.len() {
            self.sel[i] = f(i, self.sel[i]);
        }
        self.sync_text();
    }

    /// Split into groups: a chip per output PDF (click to fill it), "+ PDF", and the
    /// whole split as text.
    fn group_toolbar(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        let mut row = r.inset_xy(16.0, 10.0);
        let field = row.cut_right((row.w * 0.42).clamp(240.0, 420.0));
        row.cut_right(12.0);
        let mut remove = None;
        let mut shown = 0;
        for k in 0..self.groups.len() {
            let n = self.groups[k].iter().filter(|&&b| b).count();
            let label = format!("PDF {}  ·  {n}", k + 1);
            let w = ui.measure(&label, size::BODY, Weight::Regular) + 44.0;
            if row.w < w + 96.0 {
                break;
            }
            shown += 1;
            let c = row.cut_left_gap(w, 6.0);
            let on = k == self.active;
            let id = Id::new("group").with(k);
            if widgets::chip(ui, id, c, "", on).clicked {
                self.active = k;
            }
            let dot = Rect::new(c.x + 10.0, c.center().y - 5.0, 10.0, 10.0);
            ui.fill_circle(dot.center().x, dot.center().y, 5.0, t.series(k));
            let ink = if on { t.on_accent } else { t.text };
            ui.text_in(
                Rect::new(c.x + 26.0, c.y, c.w - 30.0, c.h),
                &label,
                size::BODY,
                Weight::Regular,
                ink,
                Align::Left,
            );
            if on && self.groups.len() > 1 {
                let x = Rect::new(c.right() + 2.0, c.y + 4.0, c.h - 8.0, c.h - 8.0);
                row.cut_left(x.w + 6.0);
                if widgets::button(
                    ui,
                    id.with("x"),
                    x,
                    "",
                    Some(Icon::Close),
                    ButtonKind::Ghost,
                    true,
                )
                .clicked
                {
                    remove = Some(k);
                }
            }
        }
        if shown < self.groups.len() {
            let more = format!("+{} more", self.groups.len() - shown);
            let c = row.cut_left_gap(70.0, 6.0);
            ui.text_in(
                c,
                &more,
                size::CAPTION,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
        }
        let add = row.cut_left_gap(72.0, 6.0);
        if widgets::chip(ui, Id::new("group-add"), add, "+ PDF", false).clicked {
            self.add_group();
            self.sync_groups();
        }
        if let Some(k) = remove {
            self.remove_group(k);
        }
        let resp = widgets::text_field(
            ui,
            Id::new("groups"),
            field,
            &mut self.groups_text,
            "1-15, 16, 18-20; 25-40, 42",
            self.groups_err.is_some(),
        );
        if resp.changed {
            self.set_groups_text();
        }
        if resp.submitted && self.groups_err.is_none() {
            self.run();
        }
        let tip = Rect::new(field.x, field.bottom() + 2.0, field.w, 18.0);
        match &self.groups_err {
            Some(e) if resp.focused => {
                let e = e.clone();
                ui.text_in(
                    tip,
                    &e,
                    size::CAPTION,
                    Weight::Regular,
                    t.error,
                    Align::Left,
                );
            }
            None if resp.focused => ui.text_in(
                tip,
                "A ; starts the next PDF",
                size::CAPTION,
                Weight::Regular,
                t.text3,
                Align::Left,
            ),
            _ => {}
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, r: Rect) {
        if self.grouping() {
            self.group_toolbar(ui, r);
            return;
        }
        let t = ui.theme;
        let mut row = r.inset_xy(16.0, 10.0);
        for (i, (label, w)) in [
            ("All", 52.0),
            ("None", 62.0),
            ("Odd", 54.0),
            ("Even", 60.0),
            ("Invert", 70.0),
        ]
        .into_iter()
        .enumerate()
        {
            let c = row.cut_left_gap(w, 6.0);
            if widgets::chip(ui, Id::new("pick").with(i), c, label, false).clicked {
                match i {
                    0 => self.set_all(|_, _| true),
                    1 => self.set_all(|_, _| false),
                    2 => self.set_all(|i, _| i % 2 == 0),
                    3 => self.set_all(|i, _| i % 2 == 1),
                    _ => self.set_all(|_, s| !s),
                }
            }
        }
        let field = row.cut_right(240.0);
        row.cut_right(10.0);
        let lab = row.cut_right(48.0);
        ui.text_in(
            lab,
            "Pages",
            size::BODY,
            Weight::Regular,
            t.text2,
            Align::Right,
        );
        let resp = widgets::text_field(
            ui,
            Id::new("range"),
            field,
            &mut self.range,
            "e.g. 1-3, 7, 10-",
            self.range_err.is_some(),
        );
        if resp.changed {
            match pages::parse(&self.range, self.count()) {
                Ok(sel) => {
                    self.sel = vec![false; self.count()];
                    for i in sel {
                        self.sel[i] = true;
                    }
                    self.range_err = None;
                }
                Err(e) => self.range_err = Some(e),
            }
        }
        if resp.submitted && self.range_err.is_none() {
            self.run();
        }
        if let Some(e) = &self.range_err
            && resp.focused
        {
            let e = e.clone();
            let tip = Rect::new(field.x, field.bottom() + 2.0, field.w, 18.0);
            ui.text_in(
                tip,
                &e,
                size::CAPTION,
                Weight::Regular,
                t.error,
                Align::Left,
            );
        }
    }

    fn grid(&mut self, ui: &mut Ui, view: Rect) {
        let t = ui.theme;
        let n = self.count();
        if n == 0 {
            match &self.files[0].error {
                Some(e) => {
                    let e = format!("Couldn't open this PDF: {e}");
                    ui.text_in(
                        view,
                        &e,
                        size::BODY,
                        Weight::Regular,
                        t.error,
                        Align::Center,
                    );
                }
                None => widgets::spinner(ui, view.center(), 14.0, t.accent),
            }
            return;
        }
        let (cols, cw) = swirvo_ui::geom::grid_columns(view.w - 12.0, CELL_W, GAP);
        // Cells share one aspect ratio (the first page's) so the grid stays regular.
        let (pw, ph) = self.sizes[0];
        let thumb_h = (cw - 16.0) * (ph / pw.max(1.0)).clamp(0.5, 1.8);
        let cell_h = thumb_h + 40.0;
        let rows = n.div_ceil(cols);
        let content = rows as f32 * (cell_h + GAP) + 8.0;
        let sid = Id::new("pages");
        let off = widgets::scroll_begin(ui, sid, view, content);
        let first_row = ((off / (cell_h + GAP)).floor() as usize).saturating_sub(1);
        let last_row = (((off + view.h) / (cell_h + GAP)).ceil() as usize + 1).min(rows);
        let mut want = Vec::new();
        let shift = ui.input.mods.shift;
        let grouping = self.grouping();
        let active = self.active.min(self.groups.len().saturating_sub(1));
        let accent = if grouping { t.series(active) } else { t.accent };
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
                let id = Id::new("page").with(i);
                let resp = ui.interact(id, c);
                let h = ui.anim(id.with("h"), if resp.hovered { 1.0 } else { 0.0 }, 22.0);
                let on = if grouping {
                    self.groups.get(active).is_some_and(|g| g[i])
                } else {
                    self.sel[i]
                };
                if on {
                    ui.fill_rrect(c, size::R_CARD, accent.fade(0.16));
                } else if h > 0.0 {
                    ui.fill_rrect(c, size::R_CARD, t.card_hover.fade(h));
                }
                let page = Rect::new(c.x + 8.0, c.y + 8.0, c.w - 16.0, thumb_h);
                // Fit the page's own ratio inside the cell.
                let (w, hh) = self.sizes[i];
                let k = (page.w / w.max(1.0)).min(page.h / hh.max(1.0));
                let pr = page.centered(w * k, hh * k);
                ui.shadow(pr, 2.0, 6.0, 2.0, t.shadow);
                ui.fill_rect(pr, Rgba::WHITE);
                match self.files[0].thumbs.get(&i) {
                    Some(pm) => {
                        let pm = pm.clone();
                        ui.image(pr, &pm, 0.0, false);
                    }
                    None => want.push(i),
                }
                if grouping {
                    // A numbered dot for every group the page is in.
                    let mut x = pr.x + 6.0;
                    for (k, g) in self.groups.iter().enumerate() {
                        if !g[i] || x > pr.right() - 22.0 {
                            continue;
                        }
                        let col = t.series(k);
                        ui.fill_circle(x + 10.0, pr.y + 16.0, 10.0, col);
                        ui.text_in(
                            Rect::new(x, pr.y + 6.0, 20.0, 20.0),
                            &(k + 1).to_string(),
                            size::CAPTION,
                            Weight::Semibold,
                            Rgba::WHITE,
                            Align::Center,
                        );
                        x += 24.0;
                    }
                }
                if on {
                    ui.stroke_rrect_w(pr.inset(-3.0), 4.0, accent, 2.0);
                    let b = Rect::new(pr.right() - 26.0, pr.y + 6.0, 20.0, 20.0);
                    ui.fill_circle(b.center().x, b.center().y, 10.0, accent);
                    ui.icon(b.inset(3.0), Icon::Check, t.on_accent);
                } else if h > 0.0 {
                    let b = Rect::new(pr.right() - 26.0, pr.y + 6.0, 20.0, 20.0);
                    ui.fill_circle(b.center().x, b.center().y, 10.0, Rgba::BLACK.fade(0.3 * h));
                    ui.stroke_logical(
                        |pb, s| {
                            if let Some(p) =
                                tiny_skia_circle(b.center().x * s, b.center().y * s, 9.0 * s)
                            {
                                pb.push_path(&p);
                            }
                        },
                        Rgba::WHITE.fade(h),
                        1.5,
                    );
                }
                let label = Rect::new(c.x, page.bottom() + 6.0, c.w, 22.0);
                let ink = if on { t.text } else { t.text2 };
                let w8 = if on {
                    Weight::Semibold
                } else {
                    Weight::Regular
                };
                ui.text_in(
                    label,
                    &(i + 1).to_string(),
                    size::BODY,
                    w8,
                    ink,
                    Align::Center,
                );
                if resp.pressed {
                    let anchor = self.anchor;
                    let flags = self.flags();
                    match (shift, anchor) {
                        (true, Some(a)) if a < flags.len() => {
                            let v = flags[a];
                            flags[a.min(i)..=a.max(i)].fill(v);
                        }
                        _ => {
                            // A press flips the page; dragging on carries that to every
                            // page swept over (like a range with Shift).
                            let before = flags.clone();
                            flags[i] = !flags[i];
                            let value = flags[i];
                            self.swipe = Some((i, value, before));
                            self.anchor = Some(i);
                        }
                    }
                    self.synced();
                }
            }
        }
        // Sweeping: the pages from where the press started to the one under the pointer.
        if let (Some((start, value, before)), Some(p)) = (self.swipe.clone(), ui.pointer())
            && ui.input.down
            && n > 0
        {
            let col = (((p.x - view.x) / (cw + GAP)).floor().max(0.0) as usize).min(cols - 1);
            let row = ((p.y - view.y - 4.0 + off) / (cell_h + GAP))
                .floor()
                .max(0.0) as usize;
            let j = (row * cols + col).min(n - 1);
            let flags = self.flags();
            if flags.len() == before.len() {
                flags.clone_from(&before);
                flags[start.min(j)..=start.max(j)].fill(value);
                self.synced();
            }
        }
        if ui.input.released || !ui.input.down {
            self.swipe = None;
        }
        widgets::scroll_end(ui, sid, view, content);
        // Nearest the middle of the viewport first.
        let mid = (first_row + last_row) / 2 * cols;
        want.sort_by_key(|&i| i.abs_diff(mid));
        self.files[0].worker.request(want);
    }

    fn footer(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        let panel = r.inset_xy(12.0, 10.0);
        ui.glass(panel, size::R_PANEL, t.panel, 14.0);
        let mut row = panel.inset_xy(16.0, 0.0);
        if let Some(res) = &self.result {
            let (ok, text, first) = match res {
                Ok(r) => (true, r.message.clone(), r.outputs.first().cloned()),
                Err(e) => (false, e.clone(), None),
            };
            let ic = row.cut_left_gap(20.0, 12.0);
            ui.icon(
                ic.centered(20.0, 20.0),
                if ok { Icon::Check } else { Icon::Warning },
                if ok { t.success } else { t.error },
            );
            let close = row.cut_right(96.0).centered(96.0, 36.0);
            row.cut_right(8.0);
            if widgets::button(
                ui,
                Id::new("done"),
                close,
                if ok { "Done" } else { "Back" },
                None,
                ButtonKind::Secondary,
                true,
            )
            .clicked
            {
                if ok {
                    self.requests.push(Request::Close);
                } else {
                    self.result = None;
                }
            }
            if let Some(p) = first {
                let show = row.cut_right(140.0).centered(140.0, 36.0);
                row.cut_right(8.0);
                if widgets::button(
                    ui,
                    Id::new("show"),
                    show,
                    "Show in folder",
                    Some(Icon::Folder),
                    ButtonKind::Primary,
                    true,
                )
                .clicked
                {
                    self.requests.push(Request::ShowInFolder(p));
                }
            }
            ui.text_in(
                row,
                &text,
                size::BODY,
                Weight::Semibold,
                t.text,
                Align::Left,
            );
            return;
        }
        let organising = self.op == ORGANISE;
        let grouping = self.grouping();
        let n = if organising {
            self.org.as_ref().map_or(0, |o| o.tiles_len())
        } else if grouping {
            self.group_pages().len()
        } else {
            self.selected().len()
        };
        let op = self.page_op();
        let label = if organising {
            "Save as a new PDF".to_string()
        } else if grouping {
            match n {
                1 => "Save 1 PDF".to_string(),
                n => format!("Split into {n} PDFs"),
            }
        } else {
            op.verb(n)
        };
        let bw = (ui.measure(&label, size::BODY, Weight::Semibold) + 60.0).max(180.0);
        let btn = row.cut_right(bw).centered(bw, 38.0);
        row.cut_right(16.0);
        let images = self.op <= 1;
        let splitting = self.op == SPLIT;
        let room = row.w
            - if images {
                116.0
            } else if splitting {
                204.0
            } else {
                0.0
            };
        let sw = room.clamp(300.0, 480.0);
        let seg = row.cut_left_gap(sw, 12.0).centered(sw, 34.0);
        widgets::segmented(ui, Id::new("op"), seg, &OPS, &mut self.op);
        if images {
            let dd = row.cut_left_gap(104.0, 12.0).centered(104.0, 34.0);
            let opts = vec!["150 dpi".to_string(), "300 dpi".to_string()];
            widgets::dropdown(ui, Id::new("dpi"), dd, &opts, &mut self.dpi);
        }
        if splitting {
            let sm = row.cut_left_gap(192.0, 12.0).centered(192.0, 34.0);
            if widgets::segmented(
                ui,
                Id::new("split-mode"),
                sm,
                &["Each page", "Groups"],
                &mut self.split_mode,
            ) && self.split_mode == 1
                && self.groups.iter().all(|g| !g.contains(&true))
                && let Some(g) = self.groups.first_mut()
            {
                // Start the first group from the current selection.
                g.clone_from(&self.sel);
                self.sync_groups();
            }
        }
        let can = if organising || grouping {
            n > 0
        } else {
            n > 0 && !(op == PageOp::Delete && n == self.count())
        };
        if let Some((_, cancel)) = &self.running {
            let cancel = cancel.clone();
            widgets::spinner(
                ui,
                swirvo_ui::Point::new(btn.x - 18.0, btn.center().y),
                8.0,
                t.accent,
            );
            if widgets::button(
                ui,
                Id::new("cancel"),
                btn,
                "Cancel",
                Some(Icon::Stop),
                ButtonKind::Secondary,
                true,
            )
            .clicked
            {
                cancel.cancel();
            }
        } else {
            let icon = match op {
                _ if organising => Icon::Export,
                PageOp::Delete => Icon::Trash,
                PageOp::Rotate => Icon::Rotate,
                _ => Icon::Export,
            };
            if widgets::button(
                ui,
                Id::new("go"),
                btn,
                &label,
                Some(icon),
                ButtonKind::Primary,
                can,
            )
            .clicked
            {
                self.run();
            }
            if !organising && op == PageOp::Delete && n == self.count() && n > 0 {
                let hint = Rect::new(btn.x - 220.0, btn.y, 210.0, btn.h);
                ui.text_in(
                    hint,
                    "Keep at least one page",
                    size::CAPTION,
                    Weight::Regular,
                    t.warning,
                    Align::Right,
                );
            }
        }
    }
}

fn tiny_skia_circle(x: f32, y: f32, r: f32) -> Option<swirvo_ui::Path> {
    swirvo_ui::PathBuilder::from_circle(x, y, r)
}

impl App for PagePicker {
    fn title(&self) -> String {
        format!("{} · Pages", self.name)
    }

    fn min_size(&self) -> (f32, f32) {
        (780.0, 500.0)
    }

    fn on_event(&mut self, ev: HostEvent) {
        if let HostEvent::FilesPicked(files) = ev {
            self.org_event(files);
        }
    }

    fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn frame(&mut self, ui: &mut Ui) {
        self.poll();
        let t = ui.theme;
        let organising = self.op == ORGANISE;
        if organising && self.org.is_none() && !self.sizes.is_empty() {
            self.org = Some(organise::Organiser::new(self.sizes.len()));
        }
        let used = !ui.has_any_focus() && organising && self.result.is_none() && self.org_keys(ui);
        if !ui.has_any_focus() && !used {
            if ui.input.key(Key::Escape) {
                self.requests.push(Request::Close);
            }
            if ui.input.key(Key::Enter) {
                if self.result.as_ref().is_some_and(|r| r.is_ok()) {
                    self.requests.push(Request::Close);
                } else {
                    self.run();
                }
            }
            if ui.input.ctrl_key(Key::A) && !organising && !self.grouping() {
                self.set_all(|_, _| true);
            }
        }
        let mut r = ui.bounds();
        let title = r.cut_top(TITLE_H);
        widgets::caption(ui, title);
        let mut tr = title;
        tr.cut_left(16.0);
        let ic = tr.cut_left_gap(22.0, 10.0);
        if let Some(icon) = &self.icon {
            ui.image(ic.centered(22.0, 22.0), icon, 0.0, false);
        }
        let b = tr.cut_left_gap(24.0, 10.0).centered(24.0, 24.0);
        ui.kind_badge(b, Kind::Document);
        let name = self.name.clone();
        let nw = ui
            .measure(&name, size::BODY_L, Weight::Semibold)
            .min(tr.w * 0.5);
        let nr = tr.cut_left_gap(nw, 8.0);
        ui.text_in(
            nr,
            &name,
            size::BODY_L,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let n = self.count();
        let sel = self.selected().len();
        let meta = match n {
            _ if organising => self.org_summary(),
            _ if self.grouping() => {
                let k = self.group_pages().len();
                format!(
                    "{n} pages · {k} PDF{} · click pages to add them to PDF {}",
                    if k == 1 { "" } else { "s" },
                    self.active + 1
                )
            }
            0 => String::new(),
            1 => "1 page".into(),
            n => format!("{n} pages · {sel} selected"),
        };
        ui.text_in(tr, &meta, size::BODY, Weight::Regular, t.text2, Align::Left);
        widgets::window_controls(ui, title, false);
        let tb = r.cut_top(TOOLBAR_H);
        let footer = r.cut_bottom(FOOTER_H);
        let view = r.inset_xy(16.0, 4.0);
        if organising {
            self.org_view(ui, view);
            self.org_toolbar(ui, tb);
        } else {
            self.grid(ui, view);
            self.toolbar(ui, tb);
        }
        self.footer(ui, footer);
    }
}

impl PagePicker {
    /// For tests: wait until the page count is known.
    pub fn settle(&mut self) {
        for _ in 0..400 {
            self.poll();
            if !self.sizes.is_empty() || self.files[0].error.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn set_range(&mut self, spec: &str) -> Result<(), String> {
        let sel = pages::parse(spec, self.count())?;
        self.sel = vec![false; self.count()];
        for i in sel {
            self.sel[i] = true;
        }
        self.sync_text();
        Ok(())
    }

    /// Open on Split → Groups, with `spec` ("1-15, 16; 25-40") once the pages are known.
    pub fn start_groups(&mut self, spec: &str) {
        self.op = SPLIT;
        self.split_mode = 1;
        if !spec.trim().is_empty() {
            if self.sizes.is_empty() {
                self.pending_groups = Some(spec.to_string());
            } else {
                self.groups_text = spec.to_string();
                self.set_groups_text();
            }
        }
    }

    /// For tests: split into groups typed as text ("1-2; 3").
    pub fn set_groups(&mut self, spec: &str) -> Result<(), String> {
        self.op = SPLIT;
        self.split_mode = 1;
        self.groups_text = spec.to_string();
        self.set_groups_text();
        match &self.groups_err {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    pub fn range_text(&self) -> &str {
        &self.range
    }

    pub fn run_and_wait(&mut self) -> Option<&Result<ToolResult, String>> {
        self.run();
        for _ in 0..2000 {
            self.poll();
            if self.running.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        self.result.as_ref()
    }
}
