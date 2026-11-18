//! The Board's live preview for PDF tools (docs/12 §3.5): while a tool's options are
//! open, the main area shows a page of the first PDF as the tool would leave it, and
//! tools about places on the page are set by pointing at it: drag the crop box (for
//! every page or just this one), click where page numbers go.
//!
//! The tool runs on a background thread, on a copy of the page in a temporary folder
//! (`pdf_ops::preview`), a quarter second after the options stop changing. A newer
//! request cancels the one running; nothing is written next to the PDF.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};

use swirvo_core::CancelToken;
use swirvo_jobs::Toolkit;
use swirvo_jobs::args::ToolArgs;
use swirvo_jobs::pdf_ops;
use swirvo_jobs::tools::Tool;
use swirvo_ui::theme::size;
use swirvo_ui::widgets::{self, ButtonKind};
use swirvo_ui::{Align, Icon, Id, Pixmap, Point, Rect, Rgba, Ui, Waker, Weight};

use crate::board::Form;
use crate::thumbs::to_pixmap;

/// Renders are fitted into this many pixels.
const RENDER_PX: (u32, u32) = (1400, 1400);
/// Options settle for this long before the tool runs again (seconds).
const DEBOUNCE: f64 = 0.25;
/// Points per millimetre.
const MM: f32 = 72.0 / 25.4;
/// The smallest crop box (points).
const MIN_BOX: f32 = 24.0;
/// How close to a crop edge a press grabs it (logical px).
const GRAB: f32 = 10.0;

struct RunReq {
    seq: u64,
    tool: Tool,
    args: ToolArgs,
    page: usize,
}

#[derive(Default)]
struct Want {
    page: Option<usize>,
    run: Option<RunReq>,
    quit: bool,
}

enum Msg {
    Info(Vec<(f32, f32)>),
    Before(usize, image::RgbaImage),
    After {
        seq: u64,
        page: usize,
        pages: usize,
        img: image::RgbaImage,
    },
    Failed(u64, String),
    OpenFailed(String),
}

struct Shared {
    want: Mutex<Want>,
    cv: Condvar,
    /// The run in progress, cancelled when a newer one is asked for.
    running: Mutex<Option<Arc<CancelToken>>>,
}

struct Worker {
    shared: Arc<Shared>,
    rx: Receiver<Msg>,
}

impl Worker {
    fn start(tk: Arc<Toolkit>, file: PathBuf, waker: Waker) -> Worker {
        let shared = Arc::new(Shared {
            want: Mutex::new(Want::default()),
            cv: Condvar::new(),
            running: Mutex::new(None),
        });
        let (tx, rx) = channel();
        let sh = shared.clone();
        std::thread::spawn(move || work(&tk, &file, &sh, &tx, &waker));
        Worker { shared, rx }
    }

    fn page(&self, i: usize) {
        let mut g = self.shared.want.lock().unwrap_or_else(|p| p.into_inner());
        g.page = Some(i);
        drop(g);
        self.shared.cv.notify_one();
    }

    fn run(&self, r: RunReq) {
        if let Some(c) = self
            .shared
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            c.cancel();
        }
        let mut g = self.shared.want.lock().unwrap_or_else(|p| p.into_inner());
        g.run = Some(r);
        drop(g);
        self.shared.cv.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if let Ok(mut g) = self.shared.want.lock() {
            g.quit = true;
        }
        if let Ok(g) = self.shared.running.lock()
            && let Some(c) = g.as_ref()
        {
            c.cancel();
        }
        self.shared.cv.notify_all();
    }
}

fn render(path: &Path, page: usize) -> Result<image::RgbaImage, String> {
    let r = swirvo_engines::pdf_tools::PageRenderer::open(path).map_err(|e| e.to_string())?;
    let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        r.session(|s| s.render_fit(page, RENDER_PX.0, RENDER_PX.1))
    }));
    match img {
        Ok(Ok(i)) => Ok(i),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err("couldn't draw this page".into()),
    }
}

fn work(tk: &Toolkit, file: &Path, sh: &Shared, tx: &Sender<Msg>, waker: &Waker) {
    let send = |m: Msg| {
        let ok = tx.send(m).is_ok();
        waker();
        ok
    };
    let r = match swirvo_engines::pdf_tools::PageRenderer::open(file) {
        Ok(r) => r,
        Err(e) => {
            send(Msg::OpenFailed(e.to_string()));
            return;
        }
    };
    let sizes: Vec<(f32, f32)> = (0..r.count()).filter_map(|i| r.size(i)).collect();
    if !send(Msg::Info(sizes)) {
        return;
    }
    r.session(|s| {
        loop {
            let (page, run) = {
                let mut g = sh.want.lock().unwrap_or_else(|p| p.into_inner());
                loop {
                    if g.quit {
                        return;
                    }
                    if g.page.is_some() || g.run.is_some() {
                        break (g.page.take(), g.run.take());
                    }
                    g = sh.cv.wait(g).unwrap_or_else(|p| p.into_inner());
                }
            };
            if let Some(i) = page {
                let img = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    s.render_fit(i, RENDER_PX.0, RENDER_PX.1)
                }));
                if let Ok(Ok(img)) = img
                    && !send(Msg::Before(i, img))
                {
                    return;
                }
            }
            if let Some(req) = run {
                let cancel = Arc::new(CancelToken::new());
                *sh.running.lock().unwrap_or_else(|p| p.into_inner()) = Some(cancel.clone());
                let out = pdf_ops::preview(tk, req.tool, file, &req.args, req.page, &cancel)
                    .map_err(|e| e.to_string())
                    .and_then(|p| {
                        let img = render(&p.pdf, p.page)?;
                        Ok((p.page, p.pages, img))
                    });
                *sh.running.lock().unwrap_or_else(|p| p.into_inner()) = None;
                if cancel.is_cancelled() {
                    continue;
                }
                let m = match out {
                    Ok((page, pages, img)) => Msg::After {
                        seq: req.seq,
                        page,
                        pages,
                        img,
                    },
                    Err(e) => Msg::Failed(req.seq, e),
                };
                if !send(m) {
                    return;
                }
            }
        }
    });
}

/// Which part of the crop box a drag moves.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Grab {
    /// Edges: left, top, right, bottom (any may be set for a corner).
    Edges([bool; 4]),
    Move,
    /// A new box from this corner (page points).
    New([f32; 2]),
}

struct Drag {
    grab: Grab,
    /// The box when the drag started, and the pointer then (page points).
    from: [f32; 4],
    at: [f32; 2],
}

pub(crate) struct Preview {
    pub file: PathBuf,
    pub tool: Tool,
    name: String,
    worker: Worker,
    sizes: Vec<(f32, f32)>,
    open_error: Option<String>,
    page: usize,
    before: Option<(usize, Pixmap)>,
    /// The latest result: request number, page in the output, its page count.
    after: Option<(u64, usize, usize, Pixmap)>,
    failed: Option<(u64, String)>,
    seq: u64,
    /// What was last sent (args and page), and when the options last changed.
    sent: Option<(ToolArgs, usize)>,
    changed_at: f64,
    /// 0: the tool's result, 1: the page as it is (Crop: 0 edits the box).
    view: usize,
    /// Crop: 0 the same box on every page, 1 a box for this page only.
    scope: usize,
    drag: Option<Drag>,
    /// Sign: the page the box was last on, to follow it when the options move it.
    sign_page: Option<usize>,
}

impl Preview {
    pub fn new(tk: Arc<Toolkit>, waker: Waker, file: PathBuf, tool: Tool) -> Preview {
        Preview {
            name: crate::file_name(&file),
            worker: Worker::start(tk, file.clone(), waker),
            file,
            tool,
            sizes: Vec::new(),
            open_error: None,
            page: 0,
            before: None,
            after: None,
            failed: None,
            seq: 0,
            sent: None,
            changed_at: 0.0,
            view: 0,
            scope: 0,
            drag: None,
            sign_page: None,
        }
    }

    fn poll(&mut self) {
        while let Ok(m) = self.worker.rx.try_recv() {
            match m {
                Msg::Info(sizes) => {
                    self.sizes = sizes;
                    self.worker.page(self.page);
                }
                Msg::Before(i, img) => {
                    if let Some(pm) = to_pixmap(&img) {
                        self.before = Some((i, pm));
                    }
                }
                Msg::After {
                    seq,
                    page,
                    pages,
                    img,
                } => {
                    if let Some(pm) = to_pixmap(&img) {
                        self.after = Some((seq, page, pages, pm));
                        self.failed = None;
                    }
                }
                Msg::Failed(seq, e) => self.failed = Some((seq, e)),
                Msg::OpenFailed(e) => self.open_error = Some(e),
            }
        }
    }

    fn relayout(&self) -> bool {
        pdf_ops::relayouts(self.tool)
    }

    /// Pages the pager steps through: the output's sheets for N-up and booklets.
    fn pages(&self) -> usize {
        if self.relayout() {
            self.after.as_ref().map_or(1, |a| a.2.max(1))
        } else {
            self.sizes.len()
        }
    }

    /// Tools shown on the page as it is, with something to place (nothing runs).
    fn placing(&self) -> bool {
        self.tool == Tool::PdfSign
    }

    fn crop(&self) -> bool {
        self.tool == Tool::PdfCrop
    }

    fn busy(&self) -> bool {
        self.sent.is_some()
            && self.after.as_ref().map(|a| a.0) != Some(self.seq)
            && self.failed.as_ref().map(|f| f.0) != Some(self.seq)
    }

    /// Ask for the tool's result when the options (or the page) changed and have settled.
    fn request(&mut self, ui: &mut Ui, args: Option<ToolArgs>) {
        let Some(args) = args else { return };
        let page = self.page;
        if self
            .sent
            .as_ref()
            .is_some_and(|(a, p)| *a == args && *p == page)
        {
            return;
        }
        let now = ui.input.time;
        if self.changed_at == 0.0 {
            self.changed_at = now;
        }
        // While a crop box is being dragged the overlay is the preview.
        if now - self.changed_at < DEBOUNCE || self.drag.is_some() {
            ui.request_repaint(DEBOUNCE as f32 / 2.0);
            return;
        }
        self.changed_at = 0.0;
        self.seq += 1;
        self.worker.run(RunReq {
            seq: self.seq,
            tool: self.tool,
            args: args.clone(),
            page,
        });
        self.sent = Some((args, page));
    }

    fn go(&mut self, page: usize) {
        let n = self.pages();
        if n == 0 {
            return;
        }
        let p = page.min(n - 1);
        if p != self.page {
            self.page = p;
            if !self.relayout() {
                self.worker.page(p);
            }
        }
    }

    /// Draw the preview in `area`. Returns false when the user closed it.
    pub fn draw(&mut self, ui: &mut Ui, area: Rect, form: &mut Form) -> bool {
        self.poll();
        let t = ui.theme;
        ui.glass(area, size::R_PANEL, t.panel, 18.0);
        let mut r = area.inset(14.0);
        // Header: name, the After / Before switch, the pager and close.
        let mut head = r.cut_top_gap(32.0, 10.0);
        let close = head.cut_right(32.0);
        let mut keep = true;
        if widgets::icon_button(ui, Id::new("pv-close"), close, Icon::Close, "Hide preview").clicked
        {
            keep = false;
        }
        head.cut_right(8.0);
        let n = self.pages();
        let next = head.cut_right(32.0);
        let label_w = 120.0;
        let label = head.cut_right(label_w);
        let prev = head.cut_right(32.0);
        let what = if self.relayout() { "Sheet" } else { "Page" };
        ui.text_in(
            label,
            &format!("{what} {} of {}", self.page + 1, n.max(1)),
            size::BODY,
            Weight::Regular,
            t.text2,
            Align::Center,
        );
        if widgets::icon_button(ui, Id::new("pv-prev"), prev, Icon::ChevronLeft, "Previous").clicked
            && self.page > 0
        {
            self.go(self.page - 1);
        }
        if widgets::icon_button(ui, Id::new("pv-next"), next, Icon::ChevronRight, "Next").clicked {
            self.go(self.page + 1);
        }
        head.cut_right(12.0);
        if !self.relayout() && !self.placing() {
            let seg = head.cut_right(170.0);
            let items: &[&str] = if self.crop() {
                &["Edit", "Result"]
            } else {
                &["After", "Before"]
            };
            widgets::segmented(ui, Id::new("pv-view"), seg, items, &mut self.view);
            head.cut_right(12.0);
        }
        ui.text_in(
            head,
            &format!("Preview · {}", self.name),
            size::BODY_L,
            Weight::Semibold,
            t.text,
            Align::Left,
        );

        // Footer: what can be done on the page.
        let foot = r.cut_bottom(36.0);
        r.cut_bottom(8.0);
        let editing_crop = (self.crop() && self.view == 0) || self.placing();
        self.footer(ui, foot, form, editing_crop);

        // The page.
        if let Some(e) = &self.open_error {
            let e = format!("Couldn't open this PDF: {e}");
            ui.text_in(r, &e, size::BODY, Weight::Regular, t.error, Align::Center);
            return keep;
        }
        if self.sizes.is_empty() {
            widgets::spinner(ui, r.center(), 14.0, t.accent);
            return keep;
        }
        let args = form.args().ok();
        if !editing_crop || self.relayout() {
            self.request(ui, args);
        }
        let show_after = !self.placing() && (self.relayout() || (self.view == 0) != self.crop());
        let (pm, w, h) = if show_after {
            match &self.after {
                Some((_, _, _, pm)) => {
                    let (w, h) = (pm.width() as f32, pm.height() as f32);
                    (Some(pm.clone()), w, h)
                }
                None => (None, 1.0, std::f32::consts::SQRT_2),
            }
        } else {
            let (w, h) = self.sizes[self.page.min(self.sizes.len() - 1)];
            let pm = self
                .before
                .as_ref()
                .filter(|(i, _)| *i == self.page)
                .map(|(_, pm)| pm.clone());
            (pm, w, h)
        };
        let fit = r.inset(12.0);
        let k = (fit.w / w.max(1.0)).min(fit.h / h.max(1.0));
        let pr = fit.centered(w * k, h * k);
        ui.shadow(pr, 2.0, 10.0, 3.0, t.shadow);
        ui.fill_rect(pr, Rgba::WHITE);
        match pm {
            Some(pm) => ui.image(pr, &pm, 0.0, false),
            None => widgets::spinner(ui, pr.center(), 12.0, t.accent),
        }
        if show_after && self.busy() {
            let dot = Point::new(pr.right() - 18.0, pr.y + 18.0);
            widgets::spinner(ui, dot, 8.0, t.accent);
        }
        if show_after
            && let Some((g, e)) = &self.failed
            && *g == self.seq
        {
            let band = Rect::new(pr.x, pr.bottom() - 44.0, pr.w, 44.0);
            ui.fill_rect(band, t.panel);
            let e = e.clone();
            ui.text_in(
                band.inset_xy(12.0, 0.0),
                &e,
                size::BODY,
                Weight::Regular,
                t.error,
                Align::Center,
            );
        }
        if self.placing() {
            self.sign_editor(ui, pr, (w, h), form);
        } else if editing_crop {
            self.crop_editor(ui, pr, (w, h), form);
        } else if self.tool == Tool::PdfWatermark && self.view == 0 {
            self.watermark_editor(ui, pr, form);
        } else if !self.relayout() && self.view == 0 {
            self.spots(ui, pr, form);
        }
        let waiting = self.before.as_ref().is_none_or(|b| b.0 != self.page);
        if self.busy() || (self.sent.is_none() && !editing_crop) || waiting {
            // Results arrive through the waker; this is only a safety net.
            ui.request_repaint(0.25);
        }
        keep
    }

    fn footer(&mut self, ui: &mut Ui, r: Rect, form: &mut Form, editing_crop: bool) {
        let t = ui.theme;
        let mut row = r;
        let hint = match self.tool {
            Tool::PdfCrop if editing_crop => {
                let seg = row.cut_left_gap(230.0, 10.0).centered(230.0, 32.0);
                widgets::segmented(
                    ui,
                    Id::new("pv-scope"),
                    seg,
                    &["Every page", "Just this page"],
                    &mut self.scope,
                );
                let fit = row.cut_left_gap(132.0, 8.0).centered(132.0, 32.0);
                if widgets::button(
                    ui,
                    Id::new("pv-fit"),
                    fit,
                    "Fit to content",
                    Some(Icon::Crop),
                    ButtonKind::Secondary,
                    self.before.as_ref().is_some_and(|b| b.0 == self.page),
                )
                .clicked
                {
                    self.fit_to_content(form);
                }
                let has_own = self.own_crop(form).is_some();
                if has_own {
                    let reset = row.cut_left_gap(150.0, 8.0).centered(150.0, 32.0);
                    if widgets::button(
                        ui,
                        Id::new("pv-reset"),
                        reset,
                        "Use the shared box",
                        None,
                        ButtonKind::Ghost,
                        true,
                    )
                    .clicked
                    {
                        self.set_own_crop(form, None);
                        self.scope = 0;
                    }
                }
                "Drag the edges or corners, or draw a new box"
            }
            Tool::PdfSign if form.value("box") == "none" => {
                "Invisible signature: choose a box in the options to place one"
            }
            Tool::PdfSign => {
                if !form.value("place").is_empty() {
                    let reset = row.cut_left_gap(150.0, 8.0).centered(150.0, 32.0);
                    if widgets::button(
                        ui,
                        Id::new("pv-sign-reset"),
                        reset,
                        "Bottom right again",
                        None,
                        ButtonKind::Ghost,
                        true,
                    )
                    .clicked
                    {
                        form.set("place", "");
                    }
                }
                "Drag the box where the signature goes, on any page"
            }
            Tool::PdfWatermark => "Drag to move it · scroll over the page to resize",
            Tool::PdfNumbers | Tool::PdfBates => "Click a dot to put the number there",
            Tool::PdfHeaderFooter => "Click a dot to align the header and footer",
            Tool::PdfNup | Tool::PdfBooklet => "How the printed sheets will look",
            Tool::PdfRedact => "Black boxes show what will be removed",
            Tool::PdfCompress => "Zoom in on pictures to judge the quality",
            _ => "Updates as you change the options",
        };
        ui.text_in(
            row.inset_xy(6.0, 0.0),
            hint,
            size::CAPTION,
            Weight::Regular,
            t.text3,
            Align::Left,
        );
    }

    /// Clickable spots for page numbers, Bates numbers and header alignment.
    fn spots(&mut self, ui: &mut Ui, pr: Rect, form: &mut Form) {
        let t = ui.theme;
        let (key, spots): (&str, Vec<(&str, f32, f32)>) = match self.tool {
            Tool::PdfNumbers | Tool::PdfBates => (
                "position",
                vec![
                    ("top-left", 0.0, 0.0),
                    ("top-center", 0.5, 0.0),
                    ("top-right", 1.0, 0.0),
                    ("bottom-left", 0.0, 1.0),
                    ("bottom-center", 0.5, 1.0),
                    ("bottom-right", 1.0, 1.0),
                ],
            ),
            Tool::PdfHeaderFooter => (
                "align",
                vec![
                    ("left", 0.0, 0.0),
                    ("center", 0.5, 0.0),
                    ("right", 1.0, 0.0),
                    ("left", 0.0, 1.0),
                    ("center", 0.5, 1.0),
                    ("right", 1.0, 1.0),
                ],
            ),
            _ => return,
        };
        let current = form.value(key).to_string();
        let inset = 22.0;
        for (k, (value, fx, fy)) in spots.into_iter().enumerate() {
            let c = Point::new(
                pr.x + inset + fx * (pr.w - 2.0 * inset),
                pr.y + inset + fy * (pr.h - 2.0 * inset),
            );
            let hit = Rect::new(c.x - 14.0, c.y - 14.0, 28.0, 28.0);
            let resp = ui.interact(Id::new("pv-spot").with(k), hit);
            let on = current == value;
            let h = if resp.hovered { 1.0 } else { 0.0 };
            ui.fill_circle(
                c.x,
                c.y,
                9.0,
                if on {
                    t.accent
                } else {
                    t.text3.fade(0.35 + 0.4 * h)
                },
            );
            if on {
                ui.fill_circle(c.x, c.y, 3.5, t.on_accent);
            }
            if resp.clicked {
                form.set(key, value);
            }
        }
    }

    /// This page's own crop in mm [left, right, top, bottom], if it has one.
    fn own_crop(&self, form: &Form) -> Option<[f32; 4]> {
        let crops = pdf_ops::custom_crops(form.value("custom"), self.sizes.len()).ok()?;
        crops
            .into_iter()
            .rev()
            .find(|(p, _)| p.contains(&self.page))
            .map(|(_, m)| m)
    }

    /// Give this page its own crop (mm), or drop it (`None`).
    fn set_own_crop(&self, form: &mut Form, m: Option<[f32; 4]>) {
        let n = self.sizes.len();
        let mut per: std::collections::BTreeMap<usize, [f32; 4]> = Default::default();
        if let Ok(crops) = pdf_ops::custom_crops(form.value("custom"), n) {
            for (pages, c) in crops {
                for p in pages {
                    per.insert(p, c);
                }
            }
        }
        match m {
            Some(m) => per.insert(self.page, m),
            None => per.remove(&self.page),
        };
        let list: Vec<(usize, [f32; 4])> = per.into_iter().collect();
        form.set("custom", &pdf_ops::format_custom_crops(&list));
    }

    /// Does the shared box apply to the page shown?
    fn shared_applies(&self, form: &Form) -> bool {
        let spec = form.value("pages").trim();
        spec.is_empty()
            || swirvo_jobs::pages::parse(spec, self.sizes.len())
                .is_ok_and(|s| s.contains(&self.page))
    }

    /// The crop margins in effect on this page (mm: left, right, top, bottom).
    fn margins(&self, form: &Form) -> [f32; 4] {
        if let Some(m) = self.own_crop(form) {
            return m;
        }
        if self.scope == 0 && self.shared_applies(form) {
            let mm = |k: &str| form.value(k).trim().parse::<f32>().unwrap_or(10.0);
            return [mm("left"), mm("right"), mm("top"), mm("bottom")];
        }
        [0.0; 4]
    }

    fn set_margins(&self, form: &mut Form, m: [f32; 4]) {
        let m = m.map(|x| (x * 10.0).round() / 10.0);
        if self.scope == 1 || self.own_crop(form).is_some() {
            self.set_own_crop(form, Some(m));
        } else {
            for (k, x) in ["left", "right", "top", "bottom"].iter().zip(m) {
                form.set(k, &format_mm(x));
            }
        }
    }

    fn fit_to_content(&mut self, form: &mut Form) {
        let Some((_, pm)) = self.before.as_ref().filter(|b| b.0 == self.page) else {
            return;
        };
        let Some([x0, y0, x1, y1]) = ink_box(pm) else {
            return;
        };
        let (w, h) = self.sizes[self.page];
        let (kx, ky) = (w / pm.width() as f32, h / pm.height() as f32);
        let pad = 6.0;
        let m = [
            (x0 as f32 * kx - pad).max(0.0),
            (w - (x1 + 1) as f32 * kx - pad).max(0.0),
            (y0 as f32 * ky - pad).max(0.0),
            (h - (y1 + 1) as f32 * ky - pad).max(0.0),
        ]
        .map(|x| x / MM);
        self.set_margins(form, m);
    }

    /// The crop box over the page: dim outside it, drag its edges, corners or middle,
    /// or press outside it to draw a new one.
    fn crop_editor(&mut self, ui: &mut Ui, pr: Rect, (w, h): (f32, f32), form: &mut Form) {
        let t = ui.theme;
        let k = pr.w / w.max(1.0);
        let [l, r, tp, b] = self.margins(form).map(|x| x * MM);
        // The box in page points (y down).
        let bx = [l, tp, (w - r).max(l + 1.0), (h - b).max(tp + 1.0)];
        let screen = |q: [f32; 4]| {
            Rect::new(
                pr.x + q[0] * k,
                pr.y + q[1] * k,
                (q[2] - q[0]) * k,
                (q[3] - q[1]) * k,
            )
        };
        let cr = screen(bx);
        // Dim what's cut off.
        let dim = Rgba::BLACK.fade(0.45);
        ui.fill_rect(Rect::new(pr.x, pr.y, pr.w, cr.y - pr.y), dim);
        ui.fill_rect(
            Rect::new(pr.x, cr.bottom(), pr.w, pr.bottom() - cr.bottom()),
            dim,
        );
        ui.fill_rect(Rect::new(pr.x, cr.y, cr.x - pr.x, cr.h), dim);
        ui.fill_rect(
            Rect::new(cr.right(), cr.y, pr.right() - cr.right(), cr.h),
            dim,
        );
        let own = self.own_crop(form).is_some() || self.scope == 1;
        let color = if own { t.warning } else { t.accent };
        ui.stroke_rrect_w(cr, 0.0, color, 2.0);
        for (hx, hy) in [
            (0.0, 0.0),
            (0.5, 0.0),
            (1.0, 0.0),
            (0.0, 0.5),
            (1.0, 0.5),
            (0.0, 1.0),
            (0.5, 1.0),
            (1.0, 1.0),
        ] {
            let c = Point::new(cr.x + hx * cr.w, cr.y + hy * cr.h);
            ui.fill_rect(Rect::new(c.x - 4.0, c.y - 4.0, 8.0, 8.0), Rgba::WHITE);
            ui.stroke_rrect_w(Rect::new(c.x - 4.0, c.y - 4.0, 8.0, 8.0), 0.0, color, 1.5);
        }
        // The size of what's kept.
        let kept = format!(
            "{} × {} mm{}",
            format_mm((bx[2] - bx[0]) / MM),
            format_mm((bx[3] - bx[1]) / MM),
            if own { " · this page only" } else { "" }
        );
        let tw = ui.measure(&kept, size::CAPTION, Weight::Semibold) + 16.0;
        let tag = Rect::new(cr.x, (cr.y - 24.0).max(pr.y + 2.0), tw, 20.0);
        ui.fill_rrect(tag, 4.0, color);
        ui.text_in(
            tag,
            &kept,
            size::CAPTION,
            Weight::Semibold,
            t.on_accent,
            Align::Center,
        );

        // Pointer.
        let resp = ui.interact(Id::new("pv-crop"), pr.inset(-GRAB));
        let pointer = ui.pointer();
        let to_pt = |p: Point| {
            [
                ((p.x - pr.x) / k).clamp(0.0, w),
                ((p.y - pr.y) / k).clamp(0.0, h),
            ]
        };
        if resp.pressed
            && let Some(p) = pointer
        {
            let near = |a: f32, b: f32| (a - b).abs() <= GRAB;
            let inside_y = p.y >= cr.y - GRAB && p.y <= cr.bottom() + GRAB;
            let inside_x = p.x >= cr.x - GRAB && p.x <= cr.right() + GRAB;
            let edges = [
                near(p.x, cr.x) && inside_y,
                near(p.y, cr.y) && inside_x,
                near(p.x, cr.right()) && inside_y,
                near(p.y, cr.bottom()) && inside_x,
            ];
            let grab = if edges.iter().any(|&e| e) {
                Grab::Edges(edges)
            } else if cr.contains(p) {
                Grab::Move
            } else {
                Grab::New(to_pt(p))
            };
            self.drag = Some(Drag {
                grab,
                from: bx,
                at: to_pt(p),
            });
        }
        if let (Some(d), Some(p), true) = (&self.drag, pointer, resp.down) {
            let q = to_pt(p);
            let (dx, dy) = (q[0] - d.at[0], q[1] - d.at[1]);
            let f = d.from;
            let nb = match d.grab {
                Grab::Edges([el, et, er, eb]) => {
                    let mut n = f;
                    if el {
                        n[0] = (f[0] + dx).clamp(0.0, f[2] - MIN_BOX);
                    }
                    if et {
                        n[1] = (f[1] + dy).clamp(0.0, f[3] - MIN_BOX);
                    }
                    if er {
                        n[2] = (f[2] + dx).clamp(f[0] + MIN_BOX, w);
                    }
                    if eb {
                        n[3] = (f[3] + dy).clamp(f[1] + MIN_BOX, h);
                    }
                    n
                }
                Grab::Move => {
                    let dx = dx.clamp(-f[0], w - f[2]);
                    let dy = dy.clamp(-f[1], h - f[3]);
                    [f[0] + dx, f[1] + dy, f[2] + dx, f[3] + dy]
                }
                Grab::New(a) => {
                    let n = [
                        a[0].min(q[0]),
                        a[1].min(q[1]),
                        a[0].max(q[0]),
                        a[1].max(q[1]),
                    ];
                    if n[2] - n[0] < MIN_BOX || n[3] - n[1] < MIN_BOX {
                        f
                    } else {
                        n
                    }
                }
            };
            if nb != bx {
                let m = [nb[0], w - nb[2], nb[1], h - nb[3]].map(|x| x.max(0.0) / MM);
                self.set_margins(form, m);
            }
        }
        if ui.input.released {
            self.drag = None;
        }
    }

    /// The watermark's centre as a crosshair: drag to move it, scroll to resize. The
    /// result follows a moment after the drag ends.
    fn watermark_editor(&mut self, ui: &mut Ui, pr: Rect, form: &mut Form) {
        let t = ui.theme;
        let place = form.value("place").to_string();
        let mut c = [0.5_f32, 0.5];
        let nums: Vec<f32> = place
            .split([',', ' ', '%'])
            .filter_map(|x| x.parse().ok())
            .collect();
        if let [x, y] = nums[..] {
            c = [(x / 100.0).clamp(0.0, 1.0), (y / 100.0).clamp(0.0, 1.0)];
        }
        let resp = ui.interact(Id::new("pv-wm"), pr);
        let pointer = ui.pointer();
        if resp.pressed && pointer.is_some() {
            self.drag = Some(Drag {
                grab: Grab::Move,
                from: [c[0], c[1], 0.0, 0.0],
                at: [0.0, 0.0],
            });
        }
        if let (Some(_), Some(p), true) = (&self.drag, pointer, resp.down) {
            c = [
                ((p.x - pr.x) / pr.w).clamp(0.0, 1.0),
                ((p.y - pr.y) / pr.h).clamp(0.0, 1.0),
            ];
            let spec = format!("{}, {}", (c[0] * 100.0).round(), (c[1] * 100.0).round());
            if form.value("place") != spec {
                form.set("place", &spec);
            }
        }
        if ui.input.released {
            self.drag = None;
        }
        if resp.hovered && ui.input.wheel != 0.0 {
            let now: f32 = form.value("size").trim().parse().unwrap_or(60.0);
            let next = (now - ui.input.wheel.signum() * 5.0).clamp(5.0, 150.0);
            form.set("size", &format!("{next}"));
        }
        // The crosshair.
        let (x, y) = (pr.x + c[0] * pr.w, pr.y + c[1] * pr.h);
        let col = if self.drag.is_some() {
            t.accent
        } else {
            t.text3
        };
        ui.fill_rect(Rect::new(x - 12.0, y - 1.0, 24.0, 2.0), col);
        ui.fill_rect(Rect::new(x - 1.0, y - 12.0, 2.0, 24.0), col);
        ui.fill_circle(x, y, 4.0, col);
    }

    /// The signature box on the page shown, if it's on this page: [left, top] in points.
    fn sign_box(&self, form: &Form) -> Option<[f32; 2]> {
        let n = self.sizes.len();
        let (w, h) = *self.sizes.get(self.page)?;
        let [bw, bh] = pdf_ops::SIGN_BOX;
        match form.value("box") {
            "none" => None,
            _ if !form.value("place").is_empty() => {
                let (p, [x, y]) = pdf_ops::sign_place(form.value("place"), n).ok()?;
                (p == self.page).then(|| {
                    [
                        (x * MM).clamp(0.0, (w - bw).max(0.0)),
                        (y * MM).clamp(0.0, (h - bh).max(0.0)),
                    ]
                })
            }
            b => {
                let p = if b == "first" { 0 } else { n.saturating_sub(1) };
                (p == self.page).then(|| [(w - 256.0).max(0.0), h - 100.0])
            }
        }
    }

    /// The visible signature's box: drag it, or press anywhere on any page to put it there.
    fn sign_editor(&mut self, ui: &mut Ui, pr: Rect, (w, h): (f32, f32), form: &mut Form) {
        let t = ui.theme;
        if form.value("box") == "none" {
            return;
        }
        // Show the page the box is on whenever the options move it to another page.
        let n = self.sizes.len();
        let target = match pdf_ops::sign_place(form.value("place"), n) {
            Ok((p, _)) => p,
            Err(_) if form.value("box") == "first" => 0,
            Err(_) => n.saturating_sub(1),
        };
        if self.sign_page != Some(target) {
            self.sign_page = Some(target);
            self.go(target);
        }
        let k = pr.w / w.max(1.0);
        let [bw, bh] = pdf_ops::SIGN_BOX;
        let at = self.sign_box(form);
        if let Some([x, y]) = at {
            let r = Rect::new(pr.x + x * k, pr.y + y * k, bw * k, bh * k);
            ui.fill_rect(r, t.accent.fade(0.18));
            ui.stroke_rrect_w(r, 0.0, t.accent, 2.0);
            ui.text_in(
                r.inset(6.0),
                "Signature",
                size::BODY,
                Weight::Semibold,
                t.accent,
                Align::Center,
            );
        }
        let resp = ui.interact(Id::new("pv-sign"), pr);
        let pointer = ui.pointer();
        let to_pt = |p: Point| {
            [
                ((p.x - pr.x) / k).clamp(0.0, w),
                ((p.y - pr.y) / k).clamp(0.0, h),
            ]
        };
        if resp.pressed
            && let Some(p) = pointer
        {
            let q = to_pt(p);
            // Grab the box where it was pressed, or centre it under the pointer.
            let from = match at {
                Some([x, y]) if q[0] >= x && q[0] <= x + bw && q[1] >= y && q[1] <= y + bh => {
                    [x, y]
                }
                _ => [q[0] - bw / 2.0, q[1] - bh / 2.0],
            };
            self.drag = Some(Drag {
                grab: Grab::Move,
                from: [from[0], from[1], from[0] + bw, from[1] + bh],
                at: q,
            });
        }
        if let (Some(d), Some(p), true) = (&self.drag, pointer, resp.down) {
            let q = to_pt(p);
            let x = (d.from[0] + q[0] - d.at[0]).clamp(0.0, (w - bw).max(0.0));
            let y = (d.from[1] + q[1] - d.at[1]).clamp(0.0, (h - bh).max(0.0));
            let spec = format!(
                "{}: {}, {}",
                self.page + 1,
                format_mm(x / MM),
                format_mm(y / MM)
            );
            if form.value("place") != spec {
                form.set("place", &spec);
            }
        }
        if ui.input.released {
            self.drag = None;
        }
    }

    /// For tests: wait until the page sizes and a result for the current options arrive.
    pub fn settle(&mut self, form: &Form) -> Option<(usize, usize)> {
        for _ in 0..2000 {
            self.poll();
            if !self.sizes.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let args = form.args().ok()?;
        self.seq += 1;
        self.worker.run(RunReq {
            seq: self.seq,
            tool: self.tool,
            args: args.clone(),
            page: self.page,
        });
        self.sent = Some((args, self.page));
        for _ in 0..4000 {
            self.poll();
            if let Some((g, page, pages, _)) = &self.after
                && *g == self.seq
            {
                return Some((*page, *pages));
            }
            if self.failed.as_ref().is_some_and(|f| f.0 == self.seq) {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }
}

fn format_mm(x: f32) -> String {
    let r = (x * 10.0).round() / 10.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r:.1}")
    }
}

/// Bounding box of non-white pixels [x0, y0, x1, y1] (inclusive, y down).
fn ink_box(pm: &Pixmap) -> Option<[u32; 4]> {
    let (w, h) = (pm.width(), pm.height());
    let px = pm.data();
    let ink = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        let p = &px[i..i + 4];
        // Premultiplied: transparent counts as white paper.
        p[3] > 8 && (p[0] < 235 || p[1] < 235 || p[2] < 235)
    };
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            if ink(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 <= x1 && y0 <= y1).then_some([x0, y0, (x1 + 1).min(w - 1), (y1 + 1).min(h - 1)])
}

/// Does `tool` get a preview, and which PDF of `files` does it show?
pub(crate) fn wants(tk: &Toolkit, tool: Tool, files: &[PathBuf]) -> Option<PathBuf> {
    if !pdf_ops::previewable(tool) && tool != Tool::PdfSign {
        return None;
    }
    files
        .iter()
        .find(|f| {
            tk.registry
                .from_path(f)
                .is_some_and(|id| tk.registry.key(id) == "pdf")
        })
        .cloned()
}
