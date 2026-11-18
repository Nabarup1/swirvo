//! The Board (docs/12 §3): drop many files, pick a target per source type, convert all.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use swirvo_core::{FormatId, Preset};
use swirvo_jobs::args::{FieldKind, ToolArgs, Values};
use swirvo_jobs::tools::{Tool, tools_for};
use swirvo_jobs::{Options, Toolkit, load_frecency, ranked_targets, save_frecency};
use swirvo_ui::theme::size;
use swirvo_ui::widgets::{self, ButtonKind};
use swirvo_ui::{
    Align, App, HostEvent, Icon, Id, Key, Kind, Pixmap, Rect, Request, Rgba, Ui, Waker, Weight,
};
use swirvo_wheel::Frecency;

use crate::runner::{self, Run, RunEvent};
use crate::thumbs::Thumbs;
use crate::{file_name, human, kind_of};

const TITLE_H: f32 = 48.0;
const STATUS_H: f32 = 30.0;
const SIDEBAR_W: f32 = 316.0;
const CARD_MIN_W: f32 = 176.0;
const CARD_H: f32 = 204.0;
const GAP: f32 = 12.0;
const MAX_FILES: usize = 5000;

#[derive(Debug, Clone, PartialEq)]
enum Status {
    Ready,
    Queued,
    Done {
        output: PathBuf,
        bytes: u64,
        note: Option<String>,
    },
    Failed(String),
    Skipped(String),
}

struct Item {
    id: u64,
    path: PathBuf,
    name: String,
    bytes: u64,
    format: Option<FormatId>,
    kind: Kind,
    selected: bool,
    status: Status,
}

struct NewItem {
    path: PathBuf,
    bytes: u64,
}

struct Toast {
    text: String,
    error: bool,
    at: f64,
    show: Option<PathBuf>,
}

/// Targets reachable from one source format, best first, with dropdown labels.
struct Targets {
    ids: Vec<FormatId>,
    labels: Vec<String>,
}

/// A tool's options, being filled in on the Tools tab.
pub(crate) struct Form {
    tool: Tool,
    /// One value per field (choices and flags hold their value string).
    values: Vec<String>,
    choice: Vec<usize>,
    /// Page-groups fields: one page selection per output PDF (empty for other fields).
    rows: Vec<Vec<String>>,
    error: Option<String>,
}

impl Form {
    fn new(tool: Tool) -> Form {
        let fields = tool.fields();
        Form {
            tool,
            values: fields.iter().map(|f| f.default.to_string()).collect(),
            choice: fields
                .iter()
                .map(|f| match f.kind {
                    FieldKind::Choice(opts) => {
                        opts.iter().position(|o| o.0 == f.default).unwrap_or(0)
                    }
                    _ => 0,
                })
                .collect(),
            rows: fields
                .iter()
                .map(|f| match f.kind {
                    FieldKind::PageGroups => vec![String::new(), String::new()],
                    _ => Vec::new(),
                })
                .collect(),
            error: None,
        }
    }

    /// Put a page-groups field's rows back into its value: `1-15, 16; 25-40`.
    fn sync_rows(&mut self, i: usize) {
        self.values[i] = self.rows[i]
            .iter()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        self.error = None;
    }

    /// The value of field `key` ("" when the tool has no such field).
    pub(crate) fn value(&self, key: &str) -> &str {
        self.tool
            .fields()
            .iter()
            .position(|f| f.key == key)
            .map_or("", |i| self.values[i].as_str())
    }

    /// Set field `key` (and, for a choice, which entry is shown).
    pub(crate) fn set(&mut self, key: &str, value: &str) {
        let fields = self.tool.fields();
        if let Some(i) = fields.iter().position(|f| f.key == key) {
            self.values[i] = value.to_string();
            if fields[i].kind == FieldKind::PageGroups {
                self.rows[i] = value
                    .split([';', '|', '\n'])
                    .map(|r| r.trim().to_string())
                    .filter(|r| !r.is_empty())
                    .collect();
                if self.rows[i].is_empty() {
                    self.rows[i].push(String::new());
                }
            }
            if let FieldKind::Choice(opts) = fields[i].kind
                && let Some(c) = opts.iter().position(|o| o.0 == value)
            {
                self.choice[i] = c;
            }
        }
    }

    /// The values that differ from the defaults, checked.
    pub(crate) fn args(&self) -> Result<ToolArgs, String> {
        let mut a = ToolArgs::new();
        for (f, v) in self.tool.fields().iter().zip(&self.values) {
            if !v.trim().is_empty() && v != f.default {
                a.insert(f.key.to_string(), v.clone());
            }
        }
        Values::new(self.tool.fields(), &a)?;
        Ok(a)
    }
}

pub struct Board {
    tk: Arc<Toolkit>,
    waker: Waker,
    items: Vec<Item>,
    next_id: u64,
    known: HashSet<PathBuf>,
    targets: HashMap<FormatId, Targets>,
    choice: HashMap<FormatId, usize>,
    thumbs: Thumbs,
    run: Option<Run>,
    run_done: usize,
    run_bytes: (u64, u64),
    summary: Option<String>,
    tab: usize,
    preset: usize,
    save_to: usize,
    out_dir: Option<PathBuf>,
    requests: Vec<Request>,
    frecency: Frecency,
    epoch: Instant,
    anchor: Option<usize>,
    toast: Option<Toast>,
    adding: Vec<Receiver<(Vec<NewItem>, usize)>>,
    icon: Option<Pixmap>,
    engines: String,
    form: Option<Form>,
    /// The form field waiting for a file from the picker.
    pick_for: Option<usize>,
    /// The open tool's live preview (PDF tools), and whether the user hid it.
    preview: Option<crate::preview::Preview>,
    preview_hidden: bool,
    /// Page counts of PDFs, for checking page numbers in forms (None: being counted).
    page_counts: HashMap<PathBuf, Option<usize>>,
    /// The title bar's search: filters files, formats and tools (all words must match).
    search: String,
    counts_tx: std::sync::mpsc::Sender<(PathBuf, Option<usize>)>,
    counts_rx: Receiver<(PathBuf, Option<usize>)>,
}

fn expand(p: PathBuf, out: &mut Vec<NewItem>, depth: u32, skipped: &mut usize) {
    if out.len() >= MAX_FILES {
        *skipped += 1;
        return;
    }
    match std::fs::metadata(&p) {
        Ok(m) if m.is_dir() => {
            if depth >= 4 {
                *skipped += 1;
                return;
            }
            let Ok(rd) = std::fs::read_dir(&p) else {
                *skipped += 1;
                return;
            };
            let mut kids: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|k| {
                    !k.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.'))
                })
                .collect();
            kids.sort();
            for k in kids {
                expand(k, out, depth + 1, skipped);
            }
        }
        Ok(m) => out.push(NewItem {
            path: p,
            bytes: m.len(),
        }),
        Err(_) => *skipped += 1,
    }
}

impl Board {
    pub fn new(tk: Arc<Toolkit>, waker: Waker, files: Vec<PathBuf>) -> Board {
        let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 3));
        let thumbs = Thumbs::new(crate::decoder(tk.clone()), waker.clone(), workers);
        let (counts_tx, counts_rx) = channel();
        let ids = tk.engine_ids();
        let mut found = Vec::new();
        // Exact ids: "office" is LibreOffice, "ms-office" is Microsoft Office.
        for (id, name) in [
            ("ffmpeg", "Media"),
            ("ms-office", "Microsoft Office"),
            ("office", "LibreOffice"),
            ("ocr", "OCR"),
            ("os-wic", "Windows codecs"),
            ("systar", "7z · RAR"),
        ] {
            if ids.contains(&id) && !found.contains(&name) {
                found.push(name);
            }
        }
        let engines = if found.is_empty() {
            "Built-in engines".to_string()
        } else {
            format!("Built-in + {}", found.join(" · "))
        };
        let mut b = Board {
            tk,
            waker,
            items: Vec::new(),
            next_id: 1,
            known: HashSet::new(),
            targets: HashMap::new(),
            choice: HashMap::new(),
            thumbs,
            run: None,
            run_done: 0,
            run_bytes: (0, 0),
            summary: None,
            tab: 0,
            // The defaults chosen in Settings; the sidebar can still change them per batch.
            preset: match swirvo_jobs::settings::current().preset {
                swirvo_jobs::settings::PresetChoice::Best => 0,
                swirvo_jobs::settings::PresetChoice::Balanced => 1,
                swirvo_jobs::settings::PresetChoice::Smallest => 2,
            },
            save_to: usize::from(swirvo_jobs::settings::current().out_dir.is_some()),
            out_dir: swirvo_jobs::settings::current().out_dir.clone(),
            requests: Vec::new(),
            frecency: load_frecency(),
            epoch: Instant::now(),
            anchor: None,
            toast: None,
            adding: Vec::new(),
            icon: crate::app_icon(64),
            engines,
            form: None,
            pick_for: None,
            preview: None,
            preview_hidden: false,
            page_counts: HashMap::new(),
            search: String::new(),
            counts_tx,
            counts_rx,
        };
        if !files.is_empty() {
            b.add_paths(files);
        }
        b
    }

    /// Show the Tools tab, with `id`'s options open when it has any (from the wheel).
    pub fn open_tool(&mut self, id: &str) {
        self.tab = 1;
        self.form = Tool::from_id(id)
            .filter(|t| !t.fields().is_empty())
            .map(Form::new);
    }

    /// Stat and expand folders on a background thread (no I/O on the UI thread).
    pub fn add_paths(&mut self, paths: Vec<PathBuf>) {
        let (tx, rx) = channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut skipped = 0;
            for p in paths {
                expand(p, &mut out, 0, &mut skipped);
            }
            let _ = tx.send((out, skipped));
            waker();
        });
        self.adding.push(rx);
    }

    fn take_added(&mut self, ui: &Ui) {
        let mut arrived = Vec::new();
        self.adding.retain(|rx| match rx.try_recv() {
            Ok(v) => {
                arrived.push(v);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
        for (list, skipped) in arrived {
            let reg = self.tk.registry;
            let mut added = 0;
            for n in list {
                if self.items.len() >= MAX_FILES || !self.known.insert(n.path.clone()) {
                    continue;
                }
                let format = reg.from_path(&n.path);
                let kind = format.map_or(Kind::Other, |f| kind_of(reg.get(f).category));
                self.items.push(Item {
                    id: self.next_id,
                    name: file_name(&n.path),
                    path: n.path,
                    bytes: n.bytes,
                    format,
                    kind,
                    selected: false,
                    status: Status::Ready,
                });
                self.next_id += 1;
                added += 1;
            }
            if skipped > 0 {
                self.say(
                    ui,
                    format!(
                        "Added {added} files · {skipped} skipped (unreadable, too deep, or over {MAX_FILES})"
                    ),
                    true,
                );
            }
        }
    }

    fn say(&mut self, ui: &Ui, text: String, error: bool) {
        self.toast = Some(Toast {
            text,
            error,
            at: ui.input.time,
            show: None,
        });
    }

    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn targets_for(&mut self, f: FormatId) -> &Targets {
        if !self.targets.contains_key(&f) {
            let items = ranked_targets(&self.tk, &[f], &self.frecency, self.now());
            let t = Targets {
                ids: items.iter().map(|i| i.target).collect(),
                labels: items
                    .iter()
                    .map(|i| format!("{} · {}", i.label, i.caption))
                    .collect(),
            };
            self.targets.insert(f, t);
        }
        &self.targets[&f]
    }

    fn any_selected(&self) -> bool {
        self.items.iter().any(|i| i.selected)
    }

    /// Items the Convert button acts on: the selection, else everything not yet done.
    fn scope(&self) -> Vec<usize> {
        if self.any_selected() {
            (0..self.items.len())
                .filter(|&i| self.items[i].selected)
                .collect()
        } else {
            (0..self.items.len())
                .filter(|&i| !matches!(self.items[i].status, Status::Done { .. }))
                .collect()
        }
    }

    /// Source formats in scope with their counts, in first-seen order.
    fn groups(&self, scope: &[usize]) -> Vec<(Option<FormatId>, usize, Kind)> {
        let mut out: Vec<(Option<FormatId>, usize, Kind)> = Vec::new();
        for &i in scope {
            let it = &self.items[i];
            match out.iter_mut().find(|g| g.0 == it.format) {
                Some(g) => g.1 += 1,
                None => out.push((it.format, 1, it.kind)),
            }
        }
        out
    }

    fn chosen(&mut self, f: FormatId) -> Option<FormatId> {
        let sel = self.choice.get(&f).copied().unwrap_or(0);
        self.targets_for(f).ids.get(sel).copied()
    }

    fn running(&self) -> bool {
        self.run.is_some()
    }

    fn start_convert(&mut self, ui: &Ui) {
        if self.running() {
            return;
        }
        let scope = self.scope();
        let mut groups: HashMap<FormatId, Vec<(u64, PathBuf)>> = HashMap::new();
        let mut order = Vec::new();
        let mut unconvertible = 0;
        for i in scope {
            let Some(f) = self.items[i].format else {
                unconvertible += 1;
                continue;
            };
            let Some(to) = self.chosen(f) else {
                unconvertible += 1;
                continue;
            };
            let it = &mut self.items[i];
            it.status = Status::Queued;
            if !groups.contains_key(&to) {
                order.push(to);
            }
            groups.entry(to).or_default().push((it.id, it.path.clone()));
            let now = self.epoch.elapsed().as_millis() as u64;
            self.frecency
                .bump(self.tk.registry.key(f), self.tk.registry.key(to), now);
        }
        if groups.is_empty() {
            self.say(ui, "Nothing here can be converted".into(), true);
            return;
        }
        if unconvertible > 0 {
            self.say(
                ui,
                format!("{unconvertible} files have no conversion and were left out"),
                false,
            );
        }
        let preset = [Preset::Best, Preset::Balanced, Preset::Smallest][self.preset.min(2)];
        let opts = Options {
            preset,
            out_dir: if self.save_to == 1 {
                self.out_dir.clone()
            } else {
                None
            },
            ..Options::from_settings()
        };
        let list = order
            .into_iter()
            .filter_map(|t| groups.remove(&t).map(|g| (t, g)))
            .collect();
        self.run_done = 0;
        self.run_bytes = (0, 0);
        self.summary = None;
        self.run = Some(runner::convert(
            self.tk.clone(),
            list,
            opts,
            self.waker.clone(),
        ));
    }

    fn start_tool(&mut self, tool: Tool, files: Vec<PathBuf>, args: ToolArgs) {
        if self.running() || files.is_empty() {
            return;
        }
        self.summary = None;
        self.run = Some(runner::tool(
            self.tk.clone(),
            tool,
            files,
            args,
            self.waker.clone(),
        ));
    }

    fn poll_run(&mut self, ui: &Ui) {
        let Some(run) = &self.run else { return };
        let mut finished = None;
        let mut events = Vec::new();
        while let Ok(ev) = run.rx.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                RunEvent::Done {
                    item,
                    output,
                    bytes_out,
                    note,
                } => {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == item) {
                        self.run_bytes.0 += it.bytes;
                        self.run_bytes.1 += bytes_out;
                        it.status = Status::Done {
                            output,
                            bytes: bytes_out,
                            note,
                        };
                        it.selected = false;
                    }
                    self.run_done += 1;
                }
                RunEvent::Failed { item, error } => {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == item) {
                        it.status = Status::Failed(error);
                    }
                    self.run_done += 1;
                }
                RunEvent::Skipped { item, reason } => {
                    if let Some(it) = self.items.iter_mut().find(|i| i.id == item) {
                        it.status = Status::Skipped(reason);
                    }
                    self.run_done += 1;
                }
                RunEvent::Tool(r) => match r {
                    Ok(r) => {
                        if let Some(text) = r.clipboard.clone() {
                            self.requests.push(Request::CopyText(text));
                        }
                        let warn = !r.failures.is_empty();
                        let text = if warn {
                            format!("{} ({} failed)", r.message, r.failures.len())
                        } else {
                            r.message.clone()
                        };
                        self.toast = Some(Toast {
                            text,
                            error: warn,
                            at: ui.input.time,
                            show: r.outputs.first().cloned(),
                        });
                    }
                    Err(e) => self.say(ui, e, true),
                },
                RunEvent::Finished { ms } => finished = Some(ms),
            }
        }
        if let Some(ms) = finished {
            let failed = self
                .items
                .iter()
                .filter(|i| matches!(i.status, Status::Failed(_)))
                .count();
            let done = self
                .items
                .iter()
                .filter(|i| matches!(i.status, Status::Done { .. }))
                .count();
            if self.run.as_ref().is_some_and(|r| r.total > 0) && self.run_bytes.0 > 0 {
                let (a, b) = self.run_bytes;
                self.summary = Some(format!(
                    "{done} converted{} · {} → {} · {:.1} s",
                    if failed > 0 {
                        format!(", {failed} failed")
                    } else {
                        String::new()
                    },
                    human(a),
                    human(b),
                    ms as f64 / 1000.0
                ));
            }
            for it in &mut self.items {
                if it.status == Status::Queued {
                    it.status = Status::Skipped("cancelled".into());
                }
            }
            self.run = None;
            let snap = self.frecency.clone();
            std::thread::spawn(move || save_frecency(&snap));
            self.targets.clear();
        }
    }

    fn keys(&mut self, ui: &mut Ui) {
        if ui.input.ctrl_key(Key::F) {
            ui.set_focus(Id::new("search"));
            return;
        }
        if ui.has_any_focus() {
            return;
        }
        if ui.input.key(Key::Escape) && !self.search.is_empty() {
            self.search.clear();
            return;
        }
        if ui.input.ctrl_key(Key::A) {
            let all = !self.items.iter().all(|i| i.selected);
            for it in &mut self.items {
                it.selected = all;
            }
        }
        if ui.input.key(Key::Escape) {
            for it in &mut self.items {
                it.selected = false;
            }
        }
        if ui.input.key(Key::Delete) && !self.running() {
            self.remove_selected();
        }
        if ui.input.key(Key::Enter) && !self.items.is_empty() {
            self.start_convert(ui);
        }
    }

    fn remove_selected(&mut self) {
        let known = &mut self.known;
        self.items.retain(|i| {
            if i.selected {
                known.remove(&i.path);
            }
            !i.selected
        });
        self.anchor = None;
    }

    // ----- drawing -----

    fn title_bar(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        widgets::caption(ui, r);
        let mut row = r;
        row.cut_left(16.0);
        let ic = row.cut_left_gap(22.0, 10.0);
        if let Some(icon) = &self.icon {
            ui.image(ic.centered(22.0, 22.0), icon, 0.0, false);
        }
        let w = ui.measure("Swirvo", size::BODY_L, Weight::Semibold);
        let tr = row.cut_left_gap(w, 6.0);
        ui.text_in(
            tr,
            "Swirvo",
            size::BODY_L,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let w2 = ui.measure("Board", size::BODY_L, Weight::Regular);
        let tr2 = row.cut_left_gap(w2, 24.0);
        ui.text_in(
            tr2,
            "Board",
            size::BODY_L,
            Weight::Regular,
            t.text2,
            Align::Left,
        );
        widgets::window_controls(ui, r, false);
        let mut tools = row;
        tools.cut_right(46.0 * 3.0 + 8.0);
        let gear = tools.cut_right(36.0).centered(32.0, 32.0);
        tools.cut_right(8.0);
        if widgets::icon_button(ui, Id::new("settings"), gear, Icon::Settings, "Settings").clicked {
            self.requests.push(Request::OpenSettings);
        }
        let add = tools.cut_left_gap(116.0, 8.0).centered(116.0, 32.0);
        if widgets::button(
            ui,
            Id::new("add"),
            add,
            "Add files",
            Some(Icon::Add),
            ButtonKind::Secondary,
            true,
        )
        .clicked
        {
            self.requests.push(Request::PickFiles);
        }
        if !self.items.is_empty() {
            let any = self.any_selected();
            let sel = tools.cut_left_gap(112.0, 4.0).centered(112.0, 32.0);
            let label = if self.items.iter().all(|i| i.selected) {
                "Select none"
            } else {
                "Select all"
            };
            if widgets::button(
                ui,
                Id::new("selall"),
                sel,
                label,
                Some(Icon::SelectAll),
                ButtonKind::Ghost,
                true,
            )
            .clicked
            {
                let all = !self.items.iter().all(|i| i.selected);
                for it in &mut self.items {
                    it.selected = all;
                }
            }
            let has_done = self
                .items
                .iter()
                .any(|i| matches!(i.status, Status::Done { .. }));
            let clr = tools.cut_left_gap(120.0, 4.0).centered(120.0, 32.0);
            if widgets::button(
                ui,
                Id::new("cleardone"),
                clr,
                "Clear done",
                Some(Icon::Broom),
                ButtonKind::Ghost,
                has_done && !self.running(),
            )
            .clicked
            {
                let known = &mut self.known;
                self.items.retain(|i| {
                    let done = matches!(i.status, Status::Done { .. });
                    if done {
                        known.remove(&i.path);
                    }
                    !done
                });
            }
            let rm = tools.cut_left_gap(104.0, 4.0).centered(104.0, 32.0);
            if widgets::button(
                ui,
                Id::new("remove"),
                rm,
                "Remove",
                Some(Icon::Trash),
                ButtonKind::Ghost,
                any && !self.running(),
            )
            .clicked
            {
                self.remove_selected();
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui, panel: Rect) {
        let t = ui.theme;
        ui.glass(panel, size::R_PANEL, t.panel, 18.0);
        let mut r = panel.inset(16.0);
        let seg = r.cut_top_gap(32.0, 18.0);
        widgets::segmented(
            ui,
            Id::new("tab"),
            seg,
            &["Convert", "Tools"],
            &mut self.tab,
        );
        // Search, under the tabs (not while a tool's options are open).
        if self.form.is_none() {
            let mut sr = r.cut_top_gap(32.0, 14.0);
            let ic = sr.cut_left_gap(18.0, 8.0);
            ui.icon(ic.centered(16.0, 16.0), Icon::Search, t.text3);
            if !self.search.is_empty() {
                let x = sr.cut_right(30.0).centered(26.0, 26.0);
                if widgets::icon_button(ui, Id::new("search-clear"), x, Icon::Close, "Clear")
                    .clicked
                {
                    self.search.clear();
                }
            }
            let hint = if self.tab == 1 {
                "Search tools (Ctrl+F)"
            } else {
                "Search formats and files (Ctrl+F)"
            };
            widgets::text_field(ui, Id::new("search"), sr, &mut self.search, hint, false);
        }

        // Bottom block first (RectCut from the bottom), so the list gets what's left: the
        // Convert controls, the form's Run button, or nothing under the tool list.
        let bottom_h = match (self.tab, &self.form) {
            (0, _) => 212.0,
            (_, Some(_)) => 96.0,
            (_, None) => 0.0,
        };
        let mut bottom = r.cut_bottom(bottom_h);
        let scope = self.scope();
        let n = scope.len();
        if self.tab == 0 {
            let btn = bottom.cut_bottom(40.0);
            bottom.cut_bottom(10.0);
            let info = bottom.cut_bottom(20.0);
            bottom.cut_bottom(6.0);
            if let Some(run) = &self.run {
                let total = run.total.max(1);
                let frac = self.run_done as f32 / total as f32;
                let label = format!("Converting {} of {total}…", (self.run_done + 1).min(total));
                ui.text_in(
                    info,
                    &label,
                    size::CAPTION,
                    Weight::Regular,
                    t.text2,
                    Align::Left,
                );
                widgets::progress(ui, Rect::new(btn.x, btn.y - 12.0, btn.w, 4.0), Some(frac));
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
                    && let Some(r) = &self.run
                {
                    r.cancel.cancel();
                }
            } else {
                if let Some(s) = &self.summary {
                    ui.icon(
                        Rect::new(info.x, info.y + 2.0, 16.0, 16.0),
                        Icon::Check,
                        t.success,
                    );
                    let s = s.clone();
                    ui.text_in(
                        info.inset_xy(0.0, 0.0).translate(22.0, 0.0),
                        &s,
                        size::CAPTION,
                        Weight::Regular,
                        t.text2,
                        Align::Left,
                    );
                }
                let label = match (n, self.any_selected()) {
                    (0, _) => "Convert".to_string(),
                    (1, _) => "Convert 1 file".to_string(),
                    (n, true) => format!("Convert {n} selected"),
                    (n, false) => format!("Convert {n} files"),
                };
                if widgets::button(
                    ui,
                    Id::new("convert"),
                    btn,
                    &label,
                    Some(Icon::Arrow),
                    ButtonKind::Primary,
                    n > 0,
                )
                .clicked
                {
                    self.start_convert(ui);
                }
            }
            let mut save = bottom.cut_bottom(32.0);
            let sl = save.cut_left(64.0);
            ui.text_in(
                sl,
                "Save to",
                size::BODY,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
            let mut opts = vec!["Next to originals".to_string()];
            if let Some(d) = &self.out_dir {
                opts.push(file_name(d));
            }
            opts.push("Choose folder…".to_string());
            let mut sel = self.save_to.min(opts.len() - 1);
            if widgets::dropdown(ui, Id::new("saveto"), save, &opts, &mut sel) {
                if sel == opts.len() - 1 {
                    self.requests.push(Request::PickFolder);
                } else {
                    self.save_to = sel;
                }
            }
            bottom.cut_bottom(10.0);
            let mut q = bottom.cut_bottom(32.0);
            let ql = q.cut_left(64.0);
            ui.text_in(
                ql,
                "Quality",
                size::BODY,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
            widgets::segmented(
                ui,
                Id::new("preset"),
                q,
                &["Best", "Balanced", "Small"],
                &mut self.preset,
            );
        }

        let head = r.cut_top_gap(18.0, 8.0);
        let heading = match (&self.form, self.tab) {
            (_, 0) => "CONVERT TO",
            (Some(_), _) => "OPTIONS",
            (None, _) => "TOOLS FOR THESE FILES",
        };
        ui.text_in(head, heading, 11.0, Weight::Semibold, t.text3, Align::Left);
        let searching_tools = self.tab == 1 && !self.search.trim().is_empty();
        if self.items.is_empty() && !searching_tools {
            let msg = r.cut_top(40.0);
            ui.text_in(
                msg,
                "Add files to see what they can become.",
                size::BODY,
                Weight::Regular,
                t.text3,
                Align::Left,
            );
            return;
        }
        if self.tab == 0 {
            let groups: Vec<_> = self
                .groups(&scope)
                .into_iter()
                .filter(|(fmt, _, _)| {
                    let Some(f) = fmt else {
                        return self.found(&["other"]);
                    };
                    let name = self.tk.registry.primary_ext(*f).to_string();
                    let targets = self.targets_for(*f).labels.join(" ");
                    self.found(&[&name, &targets])
                })
                .collect();
            if scope.is_empty() {
                // Every file is converted (nothing selected): say so, not "no match".
                let line = r.cut_top(40.0);
                ui.text_in(
                    line,
                    "All converted. Select files to convert them again.",
                    size::BODY,
                    Weight::Regular,
                    t.text3,
                    Align::Left,
                );
                return;
            }
            if groups.is_empty() {
                // Nothing to convert matches: point at the tools that do.
                let n = Tool::ALL.iter().filter(|t| self.tool_found(**t)).count();
                let line = r.cut_top(40.0);
                let msg = if n > 0 {
                    format!("No formats match. {n} tools do: see Tools")
                } else {
                    format!("Nothing matches “{}”", self.search.trim())
                };
                if widgets::button(
                    ui,
                    Id::new("search-to-tools"),
                    line,
                    &msg,
                    None,
                    ButtonKind::Ghost,
                    n > 0,
                )
                .clicked
                {
                    self.tab = 1;
                }
                return;
            }
            let row_h = 52.0;
            let content = groups.len() as f32 * row_h;
            let sid = Id::new("groups");
            let off = widgets::scroll_begin(ui, sid, r, content);
            for (gi, (fmt, count, kind)) in groups.iter().enumerate() {
                let row = Rect::new(r.x, r.y + gi as f32 * row_h - off, r.w, row_h - 6.0);
                if row.bottom() < r.y || row.y > r.bottom() {
                    continue;
                }
                let mut rr = row;
                let badge = rr.cut_left_gap(34.0, 10.0).centered(34.0, 34.0);
                ui.kind_badge(badge, *kind);
                let name = fmt.map_or("Other".to_string(), |f| {
                    self.tk.registry.primary_ext(f).to_ascii_uppercase()
                });
                let dd = rr.cut_right(146.0).centered(146.0, 32.0);
                let mut col = rr;
                let top = col.cut_top(col.h / 2.0);
                ui.text_in(
                    Rect::new(top.x, top.y + 4.0, top.w, top.h - 4.0),
                    &name,
                    size::BODY,
                    Weight::Semibold,
                    t.text,
                    Align::Left,
                );
                let cnt = if *count == 1 {
                    "1 file".to_string()
                } else {
                    format!("{count} files")
                };
                ui.text_in(
                    Rect::new(col.x, col.y - 4.0, col.w, col.h),
                    &cnt,
                    size::CAPTION,
                    Weight::Regular,
                    t.text2,
                    Align::Left,
                );
                match fmt {
                    Some(f) => {
                        let f = *f;
                        let labels = self.targets_for(f).labels.clone();
                        if labels.is_empty() {
                            ui.text_in(
                                dd,
                                "No conversions",
                                size::CAPTION,
                                Weight::Regular,
                                t.text3,
                                Align::Right,
                            );
                        } else {
                            let mut sel = self
                                .choice
                                .get(&f)
                                .copied()
                                .unwrap_or(0)
                                .min(labels.len() - 1);
                            if widgets::dropdown(
                                ui,
                                Id::new("target").with(f.index()),
                                dd,
                                &labels,
                                &mut sel,
                            ) {
                                self.choice.insert(f, sel);
                            }
                        }
                    }
                    None => ui.text_in(
                        dd,
                        "Unknown type",
                        size::CAPTION,
                        Weight::Regular,
                        t.text3,
                        Align::Right,
                    ),
                }
            }
            widgets::scroll_end(ui, sid, r, content);
        } else if self.form.is_some() {
            let files: Vec<PathBuf> = scope.iter().map(|&i| self.items[i].path.clone()).collect();
            self.tool_form(ui, r, bottom, files);
        } else {
            let files: Vec<PathBuf> = scope.iter().map(|&i| self.items[i].path.clone()).collect();
            let usable: Vec<Tool> = tools_for(&self.tk, &files)
                .into_iter()
                .filter(|t| *t != Tool::PdfPages)
                .collect();
            // Rows: the tools for these files that match the search, then (while searching)
            // other tools that match, dimmed, under a heading. None is that heading.
            let mut rows: Vec<Option<(Tool, bool)>> = usable
                .iter()
                .filter(|t| self.tool_found(**t))
                .map(|t| Some((*t, true)))
                .collect();
            if !self.search.trim().is_empty() {
                let others: Vec<Tool> = Tool::ALL
                    .iter()
                    .copied()
                    .filter(|t| *t != Tool::PdfPages && !usable.contains(t) && self.tool_found(*t))
                    .collect();
                if !others.is_empty() {
                    if rows.is_empty() && !files.is_empty() {
                        rows.push(None);
                    }
                    rows.push(None);
                    rows.extend(others.into_iter().map(|t| Some((t, false))));
                }
            }
            if rows.is_empty() {
                let msg = format!("No tools match “{}”", self.search.trim());
                ui.text_in(
                    r.cut_top(40.0),
                    &msg,
                    size::BODY,
                    Weight::Regular,
                    t.text3,
                    Align::Left,
                );
                return;
            }
            let row_h = 44.0;
            let content = rows.len() as f32 * row_h;
            let sid = Id::new("tools");
            let off = widgets::scroll_begin(ui, sid, r, content);
            for (i, entry) in rows.iter().enumerate() {
                let row = Rect::new(r.x, r.y + i as f32 * row_h - off, r.w, row_h - 4.0);
                if row.bottom() < r.y || row.y > r.bottom() {
                    continue;
                }
                let Some((tool, usable)) = *entry else {
                    // Two headings in a row: the first says nothing here matched.
                    let none_here = i == 0 && matches!(rows.get(1), Some(None));
                    let what = if none_here {
                        "None of these match"
                    } else if files.is_empty() {
                        "ADD A FILE TO USE THESE"
                    } else {
                        "NOT FOR THESE FILES"
                    };
                    ui.text_in(
                        Rect::new(row.x, row.bottom() - 18.0, row.w, 18.0),
                        what,
                        11.0,
                        Weight::Semibold,
                        t.text3,
                        Align::Left,
                    );
                    continue;
                };
                let id = Id::new("tool").with(tool.id());
                let resp = ui.interact(id, row);
                let hovered = resp.hovered && usable;
                let h = ui.anim(id.with("h"), if hovered { 1.0 } else { 0.0 }, 22.0);
                ui.fill_rrect(row, size::R_CONTROL, t.control_hover.fade(h));
                let (label, cap) = tool.label();
                let (ink, ink2, icon) = if usable {
                    (t.text, t.text2, t.accent)
                } else {
                    (t.text3, t.text3, t.text3)
                };
                let mut rr = row.inset_xy(10.0, 0.0);
                let ic = rr.cut_left_gap(18.0, 10.0);
                ui.icon(ic.centered(18.0, 18.0), Icon::Sparkle, icon);
                let run_r = rr.cut_right(56.0);
                let lw = ui.measure(label, size::BODY, Weight::Semibold);
                let lr = rr.cut_left_gap(lw, 8.0);
                ui.text_in(lr, label, size::BODY, Weight::Semibold, ink, Align::Left);
                ui.text_in(rr, cap, size::CAPTION, Weight::Regular, ink2, Align::Left);
                let verb = match (usable, self.running(), tool.fields().is_empty()) {
                    (false, _, _) | (true, true, _) => "",
                    (true, false, true) => "Run",
                    (true, false, false) => "Options",
                };
                ui.text_in(
                    run_r,
                    verb,
                    size::CAPTION,
                    Weight::Semibold,
                    t.accent.fade(0.4 + 0.6 * h),
                    Align::Right,
                );
                if resp.clicked && usable {
                    if tool.fields().is_empty() {
                        self.start_tool(tool, files.clone(), ToolArgs::new());
                    } else {
                        self.form = Some(Form::new(tool));
                    }
                }
            }
            widgets::scroll_end(ui, sid, r, content);
            if self.running() {
                let p = Rect::new(r.x, r.bottom() - 4.0, r.w, 4.0);
                widgets::progress(ui, p, None);
            }
        }
    }

    /// The open tool's options, with Back and Run.
    fn tool_form(&mut self, ui: &mut Ui, r: Rect, mut bottom: Rect, files: Vec<PathBuf>) {
        let t = ui.theme;
        // Taken out for the frame (so `self` stays free) and put back at the end.
        let Some(mut form) = self.form.take() else {
            return;
        };
        let tool = form.tool;
        let fields = tool.fields();
        let mut area = r;
        // Header: back, then what the tool does.
        let mut head = area.cut_top_gap(32.0, 12.0);
        let back = head.cut_left_gap(32.0, 8.0);
        let mut close = false;
        if widgets::button(
            ui,
            Id::new("form-back"),
            back,
            "",
            Some(Icon::ChevronLeft),
            ButtonKind::Ghost,
            true,
        )
        .clicked
        {
            close = true;
        }
        if self.preview.is_some() && self.preview_hidden {
            let pv = head.cut_right(84.0);
            if widgets::button(
                ui,
                Id::new("form-preview"),
                pv,
                "Preview",
                None,
                ButtonKind::Ghost,
                true,
            )
            .clicked
            {
                self.preview_hidden = false;
            }
        }
        let (label, caption) = tool.label();
        let lw = ui.measure(label, size::BODY, Weight::Semibold);
        let lr = head.cut_left_gap(lw, 8.0);
        ui.text_in(lr, label, size::BODY, Weight::Semibold, t.text, Align::Left);
        ui.text_in(
            head,
            caption,
            size::CAPTION,
            Weight::Regular,
            t.text2,
            Align::Left,
        );

        // Page numbers are checked against the PDFs' page counts once they're known.
        let pages = self.page_count(&files);
        let n_files = files.len();
        // What's wrong with each field, checked as the user types (docs/17 §3.1).
        let errors: Vec<Option<String>> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let v = &form.values[i];
                if v.trim().is_empty() {
                    return None;
                }
                f.validate_for(v, pages).err()
            })
            .collect();
        // One page selection per row: (row text, its error).
        let row_errors = |rows: &[String]| -> Vec<Option<String>> {
            rows.iter()
                .map(|r| {
                    if r.trim().is_empty() {
                        None
                    } else if let Some(n) = pages {
                        swirvo_jobs::pages::parse(r, n).err()
                    } else {
                        swirvo_jobs::pages::check(r).err()
                    }
                })
                .collect()
        };
        let heights: Vec<f32> = fields
            .iter()
            .enumerate()
            .map(|(i, f)| match f.kind {
                FieldKind::PageGroups => {
                    let rows: f32 = row_errors(&form.rows[i])
                        .iter()
                        .map(|e| 32.0 + if e.is_some() { 22.0 } else { 8.0 })
                        .sum();
                    20.0 + rows + 32.0 + 6.0 + 18.0 + 12.0
                }
                _ => {
                    let label = if matches!(f.kind, FieldKind::Flag) {
                        0.0
                    } else {
                        20.0
                    };
                    let below = !f.hint.is_empty() || errors[i].is_some();
                    label + 32.0 + if below { 18.0 } else { 0.0 } + 12.0
                }
            })
            .collect();
        let content: f32 = heights.iter().sum();
        let sid = Id::new("form").with(tool.id());
        let off = widgets::scroll_begin(ui, sid, area, content);
        let mut y = area.y - off;
        let mut submitted = false;
        for (i, f) in fields.iter().enumerate() {
            let fid = Id::new("field").with(tool.id()).with(f.key);
            let h = heights[i];
            if y + h < area.y || y > area.bottom() {
                y += h;
                continue;
            }
            let mut row = Rect::new(area.x, y, area.w, h - 12.0);
            if !matches!(f.kind, FieldKind::Flag) {
                let lab = row.cut_top(20.0);
                ui.text_in(
                    lab,
                    f.label,
                    size::CAPTION,
                    Weight::Semibold,
                    t.text2,
                    Align::Left,
                );
                if let (FieldKind::Pages | FieldKind::PageGroups, Some(n)) = (f.kind, pages) {
                    let count = if n == 1 {
                        "1 page".to_string()
                    } else {
                        format!("{n} pages")
                    };
                    ui.text_in(
                        lab,
                        &count,
                        size::CAPTION,
                        Weight::Regular,
                        t.text3,
                        Align::Right,
                    );
                }
            }
            if f.kind == FieldKind::PageGroups {
                submitted |= self.groups_field(ui, &mut form, i, fid, row, &row_errors, &files);
                y += h;
                continue;
            }
            let ctrl = row.cut_top(32.0);
            let value = &mut form.values[i];
            let invalid = errors[i].is_some();
            match f.kind {
                FieldKind::Text | FieldKind::Number { .. } | FieldKind::Pages => {
                    submitted |=
                        widgets::text_field(ui, fid, ctrl, value, f.default, invalid).submitted;
                }
                FieldKind::Secret => {
                    submitted |= widgets::secret_field(ui, fid, ctrl, value, "", false).submitted;
                }
                FieldKind::Choice(opts) => {
                    let labels: Vec<String> = opts.iter().map(|o| o.1.to_string()).collect();
                    let sel = &mut form.choice[i];
                    if widgets::dropdown(ui, fid, ctrl, &labels, sel) {
                        *value = opts[(*sel).min(opts.len() - 1)].0.to_string();
                    }
                }
                FieldKind::Flag => {
                    let on = value == "true";
                    let cb = Rect::new(ctrl.x, ctrl.y + 6.0, 20.0, 20.0);
                    if widgets::checkbox(ui, fid, cb, on).clicked {
                        *value = if on { "false" } else { "true" }.to_string();
                    }
                    let lab = Rect::new(ctrl.x + 30.0, ctrl.y, ctrl.w - 30.0, ctrl.h);
                    ui.text_in(
                        lab,
                        f.label,
                        size::BODY,
                        Weight::Regular,
                        t.text,
                        Align::Left,
                    );
                }
                FieldKind::File => {
                    let mut c = ctrl;
                    let pick = c.cut_right(92.0);
                    c.cut_right(8.0);
                    widgets::text_field(ui, fid, c, value, "No file", invalid);
                    if widgets::button(
                        ui,
                        fid.with("pick"),
                        pick,
                        "Choose…",
                        None,
                        ButtonKind::Secondary,
                        true,
                    )
                    .clicked
                    {
                        self.pick_for = Some(i);
                        self.requests.push(Request::PickFiles);
                    }
                }
                FieldKind::PageGroups => {}
            }
            // Under the field: what's wrong with it, else the hint.
            let below = row.cut_top(18.0);
            match &errors[i] {
                Some(e) => {
                    let e = e.clone();
                    ui.text_in(
                        below,
                        &e,
                        size::CAPTION,
                        Weight::Regular,
                        t.error,
                        Align::Left,
                    );
                }
                None if !f.hint.is_empty() => ui.text_in(
                    below,
                    f.hint,
                    size::CAPTION,
                    Weight::Regular,
                    t.text3,
                    Align::Left,
                ),
                None => {}
            }
            y += h;
        }
        widgets::scroll_end(ui, sid, area, content);

        // Bottom: why it can't run yet (or why it failed), and Run.
        let btn = bottom.cut_bottom(40.0);
        bottom.cut_bottom(10.0);
        let info = bottom.cut_bottom(36.0);
        let problem = fields.iter().enumerate().find_map(|(i, f)| {
            let v = form.values[i].trim();
            if v.is_empty() && f.required {
                Some(format!("{} is needed", f.label))
            } else {
                errors[i].clone()
            }
        });
        let (msg, color, icon) = match (&form.error, &problem) {
            (Some(e), _) => (Some(e.clone()), t.error, Icon::Warning),
            (None, Some(p)) => (Some(p.clone()), t.text2, Icon::Info),
            (None, None) => (None, t.text2, Icon::Info),
        };
        if let Some(m) = msg {
            ui.icon(
                Rect::new(info.x, info.center().y - 8.0, 16.0, 16.0),
                icon,
                color,
            );
            ui.text_in(
                info.translate(22.0, 0.0),
                &m,
                size::CAPTION,
                Weight::Regular,
                color,
                Align::Left,
            );
        }
        let run_label = if n_files == 1 {
            format!("{label} 1 file")
        } else {
            format!("{label} {n_files} files")
        };
        let can_run = n_files > 0 && !self.running() && problem.is_none();
        let run = widgets::button(
            ui,
            Id::new("form-run"),
            btn,
            &run_label,
            Some(Icon::Arrow),
            ButtonKind::Primary,
            can_run,
        )
        .clicked;
        if self.running() {
            widgets::progress(ui, Rect::new(btn.x, btn.y - 12.0, btn.w, 4.0), None);
        }
        if (run || submitted) && can_run {
            match form.args() {
                Ok(args) => {
                    form.error = None;
                    self.start_tool(tool, files, args);
                }
                Err(e) => form.error = Some(e),
            }
        }
        if close {
            self.pick_for = None;
        } else {
            self.form = Some(form);
        }
    }

    /// A page-groups field: a row per output PDF ("PDF 1 [1-15, 16] ×"), then "+ Add
    /// another PDF" and "Pick visually" (the page picker's Groups mode). Returns true
    /// when Enter was pressed in a row.
    #[allow(clippy::too_many_arguments)]
    fn groups_field(
        &mut self,
        ui: &mut Ui,
        form: &mut Form,
        i: usize,
        fid: Id,
        mut r: Rect,
        row_errors: &dyn Fn(&[String]) -> Vec<Option<String>>,
        files: &[PathBuf],
    ) -> bool {
        let t = ui.theme;
        let errs = row_errors(&form.rows[i]);
        let mut submitted = false;
        let mut remove = None;
        let many = form.rows[i].len() > 1;
        for (k, e) in errs.iter().enumerate() {
            let mut line = r.cut_top(32.0);
            let tag = line.cut_left_gap(58.0, 6.0);
            let dot = Rect::new(tag.x, tag.center().y - 4.0, 8.0, 8.0);
            ui.fill_circle(dot.center().x, dot.center().y, 4.0, t.series(k));
            ui.text_in(
                Rect::new(tag.x + 14.0, tag.y, tag.w - 14.0, tag.h),
                &format!("PDF {}", k + 1),
                size::BODY,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
            if many {
                let x = line.cut_right(28.0).centered(28.0, 28.0);
                line.cut_right(4.0);
                if widgets::icon_button(ui, fid.with("x").with(k), x, Icon::Close, "Remove").clicked
                {
                    remove = Some(k);
                }
            }
            let hint = if k == 0 {
                "e.g. 1-15, 16, 18-20"
            } else {
                "e.g. 25-40, 42"
            };
            let resp = widgets::text_field(
                ui,
                fid.with("row").with(k),
                line,
                &mut form.rows[i][k],
                hint,
                e.is_some(),
            );
            submitted |= resp.submitted;
            if resp.changed {
                form.sync_rows(i);
            }
            let below = r.cut_top(if e.is_some() { 22.0 } else { 8.0 });
            if let Some(e) = e {
                ui.text_in(
                    Rect::new(line.x, below.y + 2.0, line.w, 16.0),
                    e,
                    size::CAPTION,
                    Weight::Regular,
                    t.error,
                    Align::Left,
                );
            }
        }
        if let Some(k) = remove {
            form.rows[i].remove(k);
            form.sync_rows(i);
        }
        let mut buttons = r.cut_top(32.0);
        let add = buttons.cut_left_gap(150.0, 8.0);
        if widgets::button(
            ui,
            fid.with("add"),
            add,
            "Add another PDF",
            Some(Icon::Add),
            ButtonKind::Secondary,
            form.rows[i].len() < 1000,
        )
        .clicked
        {
            form.rows[i].push(String::new());
        }
        let pdf = files.iter().find(|f| {
            self.tk
                .registry
                .from_path(f)
                .is_some_and(|id| self.tk.registry.key(id) == "pdf")
        });
        if let Some(pdf) = pdf
            && widgets::button(
                ui,
                fid.with("pick"),
                buttons,
                "Pick visually",
                Some(Icon::Pages),
                ButtonKind::Ghost,
                true,
            )
            .clicked
        {
            self.requests.push(Request::OpenPages {
                tool: form.tool.id().to_string(),
                file: pdf.clone(),
                spec: form.values[i].clone(),
            });
        }
        r.cut_top(6.0);
        let hint = r.cut_top(18.0);
        ui.text_in(
            hint,
            "Each row is one PDF",
            size::CAPTION,
            Weight::Regular,
            t.text3,
            Align::Left,
        );
        submitted
    }

    /// The smallest page count among the PDFs in `files`, once all are known. Counting
    /// happens on a background thread (no I/O on the UI thread).
    fn page_count(&mut self, files: &[PathBuf]) -> Option<usize> {
        while let Ok((p, n)) = self.counts_rx.try_recv() {
            self.page_counts.insert(p, n);
        }
        let mut least: Option<usize> = None;
        let mut all = true;
        for f in files {
            let is_pdf = self
                .tk
                .registry
                .from_path(f)
                .is_some_and(|id| self.tk.registry.key(id) == "pdf");
            if !is_pdf {
                continue;
            }
            match self.page_counts.get(f) {
                Some(Some(n)) => least = Some(least.map_or(*n, |l| l.min(*n))),
                Some(None) => all = false,
                None => {
                    all = false;
                    if self.page_counts.len() > 10_000 {
                        self.page_counts.clear();
                    }
                    self.page_counts.insert(f.clone(), None);
                    let (tx, w, f) = (self.counts_tx.clone(), self.waker.clone(), f.clone());
                    std::thread::spawn(move || {
                        let n = swirvo_engines::pdf_tools::page_count(&f).ok();
                        if let Some(n) = n {
                            let _ = tx.send((f, Some(n)));
                            w();
                        }
                    });
                }
            }
        }
        if all { least } else { None }
    }

    /// Does `hay` contain every word of the search (any case)? True when it's empty.
    fn found(&self, hay: &[&str]) -> bool {
        let q = self.search.trim().to_lowercase();
        if q.is_empty() {
            return true;
        }
        let hay = hay.join(" ").to_lowercase();
        q.split_whitespace().all(|w| hay.contains(w))
    }

    /// What a search looks through for a tool: its name, caption and id.
    fn tool_found(&self, t: Tool) -> bool {
        let (label, cap) = t.label();
        self.found(&[label, cap, t.id()])
    }

    /// Start, keep or drop the live preview for the open tool's first PDF.
    fn sync_preview(&mut self) {
        let want = self.form.as_ref().and_then(|f| {
            let files: Vec<PathBuf> = self
                .scope()
                .iter()
                .map(|&i| self.items[i].path.clone())
                .collect();
            crate::preview::wants(&self.tk, f.tool, &files).map(|p| (p, f.tool))
        });
        match want {
            None => self.preview = None,
            Some((file, tool)) => {
                if self
                    .preview
                    .as_ref()
                    .is_none_or(|p| p.file != file || p.tool != tool)
                {
                    self.preview = Some(crate::preview::Preview::new(
                        self.tk.clone(),
                        self.waker.clone(),
                        file,
                        tool,
                    ));
                    self.preview_hidden = false;
                }
            }
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, area: Rect) {
        let t = ui.theme;
        let zone = area.centered(area.w.min(520.0), 300.0_f32.min(area.h));
        let hot = ui.input.drag_hover;
        ui.glass(zone, 16.0, if hot { t.accent_wash } else { t.panel }, 0.0);
        let border = if hot { t.accent } else { t.text3.fade(0.7) };
        // Dashed border, drawn as short segments around the rounded zone.
        let z = zone.inset(10.0);
        ui.stroke_logical(
            |pb, s| {
                let dash = 7.0;
                let gap = 6.0;
                let mut seg = |x0: f32, y0: f32, x1: f32, y1: f32| {
                    let len = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
                    let n = (len / (dash + gap)).floor() as i32;
                    for i in 0..n {
                        let a = i as f32 * (dash + gap) / len;
                        let b = (i as f32 * (dash + gap) + dash) / len;
                        pb.move_to((x0 + (x1 - x0) * a) * s, (y0 + (y1 - y0) * a) * s);
                        pb.line_to((x0 + (x1 - x0) * b) * s, (y0 + (y1 - y0) * b) * s);
                    }
                };
                let k = 10.0;
                seg(z.x + k, z.y, z.right() - k, z.y);
                seg(z.right(), z.y + k, z.right(), z.bottom() - k);
                seg(z.right() - k, z.bottom(), z.x + k, z.bottom());
                seg(z.x, z.bottom() - k, z.x, z.y + k);
                // Rounded corners, as quarter arcs.
                for (cx, cy, a0) in [
                    (z.right() - k, z.y + k, -std::f32::consts::FRAC_PI_2),
                    (z.right() - k, z.bottom() - k, 0.0),
                    (z.x + k, z.bottom() - k, std::f32::consts::FRAC_PI_2),
                    (z.x + k, z.y + k, std::f32::consts::PI),
                ] {
                    for i in 0..=8 {
                        let a = a0 + std::f32::consts::FRAC_PI_2 * i as f32 / 8.0;
                        let (x, y) = ((cx + k * a.cos()) * s, (cy + k * a.sin()) * s);
                        if i == 0 {
                            pb.move_to(x, y);
                        } else {
                            pb.line_to(x, y);
                        }
                    }
                }
            },
            border,
            1.5,
        );
        let mut c = zone.inset(28.0);
        c.cut_top(10.0);
        let ic = c.cut_top_gap(64.0, 16.0);
        if let Some(icon) = &self.icon {
            ui.image(ic.centered(64.0, 64.0), icon, 0.0, false);
        }
        let title = c.cut_top_gap(30.0, 6.0);
        let headline = if hot {
            "Drop to add"
        } else {
            "Drop files here"
        };
        ui.text_in(
            title,
            headline,
            size::SUBTITLE + 2.0,
            Weight::Semibold,
            t.text,
            Align::Center,
        );
        let sub = c.cut_top_gap(20.0, 18.0);
        ui.text_in(
            sub,
            "Photos, PDFs, documents, audio, video, archives: convert many at once.",
            size::BODY,
            Weight::Regular,
            t.text2,
            Align::Center,
        );
        let b = c.cut_top(36.0).centered(150.0, 36.0);
        if widgets::button(
            ui,
            Id::new("add-empty"),
            b,
            "Add files…",
            Some(Icon::Add),
            ButtonKind::Primary,
            true,
        )
        .clicked
        {
            self.requests.push(Request::PickFiles);
        }
    }

    fn card(&mut self, ui: &mut Ui, i: usize, c: Rect, any_selected: bool, shift: bool) {
        let t = ui.theme;
        let id = Id::new("card").with(self.items[i].id);
        let resp = ui.interact(id, c);
        let h = ui.anim(id.with("h"), if resp.hovered { 1.0 } else { 0.0 }, 20.0);
        let selected = self.items[i].selected;
        let lift = 2.0 * h;
        let c = c.translate(0.0, -lift);
        if h > 0.01 {
            ui.shadow(c, size::R_CARD, 14.0, 6.0, t.shadow.fade(h * 0.8));
        }
        ui.fill_rrect(c, size::R_CARD, t.card.lerp(t.card_hover, h));
        if selected {
            ui.fill_rrect(c, size::R_CARD, t.accent_wash);
            ui.stroke_rrect_w(c, size::R_CARD, t.accent, 1.5);
        } else {
            ui.stroke_rrect(c, size::R_CARD, t.panel_stroke);
        }
        let thumb = Rect::new(c.x + 8.0, c.y + 8.0, c.w - 16.0, 116.0);
        ui.fill_rrect(
            thumb,
            7.0,
            if t.dark {
                Rgba::BLACK.fade(0.25)
            } else {
                Rgba::BLACK.fade(0.04)
            },
        );
        let (path, kind, format) = (
            self.items[i].path.clone(),
            self.items[i].kind,
            self.items[i].format,
        );
        let px = ((thumb.w.max(thumb.h) * ui.scale) / 32.0).ceil() as u32 * 32;
        let cover = kind == Kind::Image;
        match self.thumbs.get(&path, px.clamp(64, 512)) {
            Some(Some(pm)) => {
                let pm = pm.clone();
                if cover {
                    ui.image(thumb, &pm, 7.0, true);
                } else {
                    ui.image(thumb.inset(6.0), &pm, 3.0, false);
                }
            }
            Some(None) => ui.kind_badge(thumb.centered(52.0, 52.0), kind),
            None => {
                ui.kind_badge(thumb.centered(52.0, 52.0), kind);
            }
        }
        // Type pill on the thumbnail.
        let ext = format.map_or_else(
            || {
                path.extension()
                    .map_or("?".into(), |e| e.to_string_lossy().to_ascii_uppercase())
            },
            |f| self.tk.registry.primary_ext(f).to_ascii_uppercase(),
        );
        let ew = ui.measure(&ext, 10.5, Weight::Semibold) + 12.0;
        let pill = Rect::new(thumb.right() - ew - 6.0, thumb.bottom() - 24.0, ew, 18.0);
        ui.fill_rrect(pill, 9.0, Rgba::BLACK.fade(0.55));
        ui.text_in(
            pill,
            &ext,
            10.5,
            Weight::Semibold,
            Rgba::WHITE,
            Align::Center,
        );

        let mut body = Rect::new(
            c.x + 12.0,
            thumb.bottom() + 8.0,
            c.w - 24.0,
            c.bottom() - thumb.bottom() - 12.0,
        );
        let name_r = body.cut_top(20.0);
        let name = self.items[i].name.clone();
        ui.text_in(
            name_r,
            &name,
            size::BODY,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let meta = body.cut_top(18.0);
        ui.text_in(
            meta,
            &human(self.items[i].bytes),
            size::CAPTION,
            Weight::Regular,
            t.text2,
            Align::Left,
        );
        let st = body.cut_top(22.0);
        let status = self.items[i].status.clone();
        match &status {
            Status::Ready => {
                let target = format.and_then(|f| self.chosen(f));
                match target {
                    Some(to) => {
                        let label = format!(
                            "→ {}",
                            self.tk.registry.primary_ext(to).to_ascii_uppercase()
                        );
                        ui.text_in(
                            st,
                            &label,
                            size::CAPTION,
                            Weight::Semibold,
                            t.accent,
                            Align::Left,
                        );
                    }
                    None => ui.text_in(
                        st,
                        "No conversion",
                        size::CAPTION,
                        Weight::Regular,
                        t.text3,
                        Align::Left,
                    ),
                }
            }
            Status::Queued => {
                widgets::spinner(
                    ui,
                    swirvo_ui::Point::new(st.x + 7.0, st.center().y),
                    6.0,
                    t.accent,
                );
                ui.text_in(
                    st.translate(20.0, 0.0),
                    "Converting…",
                    size::CAPTION,
                    Weight::Regular,
                    t.text2,
                    Align::Left,
                );
            }
            Status::Done {
                output,
                bytes,
                note,
            } => {
                // A note (saved elsewhere, fallback engine) shows as an info mark with the
                // explanation on hover.
                let (icon, color) = match note {
                    Some(_) => (Icon::Info, t.warning),
                    None => (Icon::Check, t.success),
                };
                ui.icon(
                    Rect::new(st.x, st.center().y - 7.0, 14.0, 14.0),
                    icon,
                    color,
                );
                if let (Some(n), true) = (note, resp.hovered) {
                    ui.tooltip(n.clone());
                }
                let before = self.items[i].bytes.max(1) as f64;
                let delta = (*bytes as f64 - before) / before * 100.0;
                let ext = output
                    .extension()
                    .map_or(String::new(), |e| e.to_string_lossy().to_ascii_uppercase());
                let label = format!(
                    "{ext} · {} · {}{:.0}%",
                    human(*bytes),
                    if delta >= 0.0 { "+" } else { "−" },
                    delta.abs()
                );
                ui.text_in(
                    Rect::new(st.x + 20.0, st.y, st.w - 20.0, st.h),
                    &label,
                    size::CAPTION,
                    Weight::Semibold,
                    t.success,
                    Align::Left,
                );
            }
            Status::Failed(e) => {
                ui.icon(
                    Rect::new(st.x, st.center().y - 7.0, 14.0, 14.0),
                    Icon::Warning,
                    t.error,
                );
                ui.text_in(
                    Rect::new(st.x + 20.0, st.y, st.w - 20.0, st.h),
                    e,
                    size::CAPTION,
                    Weight::Regular,
                    t.error,
                    Align::Left,
                );
                if resp.hovered {
                    ui.tooltip(e.clone());
                }
            }
            Status::Skipped(r) => {
                ui.text_in(st, r, size::CAPTION, Weight::Regular, t.text3, Align::Left);
            }
        }

        // Hover controls (drawn after the card so they win the hover).
        let mut sub_clicked = false;
        if resp.hovered || selected || any_selected {
            let cb = Rect::new(thumb.x + 6.0, thumb.y + 6.0, 22.0, 22.0);
            ui.fill_rrect(cb, 6.0, Rgba::BLACK.fade(0.25));
            if widgets::checkbox(ui, id.with("cb"), cb, selected).clicked {
                self.items[i].selected = !selected;
                self.anchor = Some(i);
                sub_clicked = true;
            }
        }
        if resp.hovered && !self.running() {
            let x = Rect::new(thumb.right() - 30.0, thumb.y + 6.0, 24.0, 24.0);
            ui.fill_rrect(x, 6.0, Rgba::BLACK.fade(0.35));
            if widgets::icon_button(ui, id.with("x"), x, Icon::Close, "Remove from the Board")
                .clicked
            {
                let p = self.items[i].path.clone();
                self.known.remove(&p);
                self.items.remove(i);
                self.anchor = None;
                return;
            }
        }
        if let Status::Done { output, .. } = &status
            && resp.hovered
        {
            let f = Rect::new(thumb.right() - 58.0, thumb.y + 6.0, 24.0, 24.0);
            ui.fill_rrect(f, 6.0, Rgba::BLACK.fade(0.35));
            if widgets::icon_button(ui, id.with("show"), f, Icon::Folder, "Show in folder").clicked
            {
                self.requests.push(Request::ShowInFolder(output.clone()));
                sub_clicked = true;
            }
            if resp.double_clicked {
                self.requests.push(Request::Open(output.clone()));
            }
        }
        if resp.clicked && !sub_clicked {
            match (shift, self.anchor) {
                (true, Some(a)) if a < self.items.len() => {
                    let (lo, hi) = (a.min(i), a.max(i));
                    for it in &mut self.items[lo..=hi] {
                        it.selected = true;
                    }
                }
                _ => {
                    self.items[i].selected = !selected;
                    self.anchor = Some(i);
                }
            }
        }
    }

    fn grid(&mut self, ui: &mut Ui, area: Rect) {
        let t = ui.theme;
        let mut a = area;
        let head = a.cut_top_gap(36.0, 8.0);
        let total: u64 = self.items.iter().map(|i| i.bytes).sum();
        let all = self.items.len();
        // The search narrows the cards to files whose name matches; when none does, it's a
        // search for tools or formats and every file stays.
        let mut shown: Vec<usize> = (0..all)
            .filter(|&i| self.found(&[&file_name(&self.items[i].path)]))
            .collect();
        if shown.is_empty() {
            shown = (0..all).collect();
        }
        let n = shown.len();
        let title = match (n, all) {
            (n, a) if n < a => format!("{n} of {a} files"),
            (1, _) => "1 file".to_string(),
            (n, _) => format!("{n} files"),
        };
        let mut hr = head;
        let tw = ui.measure(&title, size::SUBTITLE, Weight::Semibold);
        let tr = hr.cut_left_gap(tw, 10.0);
        ui.text_in(
            tr,
            &title,
            size::SUBTITLE,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let sel = self.items.iter().filter(|i| i.selected).count();
        let sub = if sel > 0 {
            format!("{} · {sel} selected", human(total))
        } else {
            human(total)
        };
        ui.text_in(
            hr.translate(0.0, 1.0),
            &sub,
            size::BODY,
            Weight::Regular,
            t.text2,
            Align::Left,
        );

        let view = a;
        let (cols, cw) = swirvo_ui::geom::grid_columns(view.w - 10.0, CARD_MIN_W, GAP);
        let rows = n.div_ceil(cols);
        let content = rows as f32 * (CARD_H + GAP) + 8.0;
        let sid = Id::new("grid");
        let off = widgets::scroll_begin(ui, sid, view, content);
        let any_selected = sel > 0;
        let shift = ui.input.mods.shift;
        for (k, &i) in shown.iter().enumerate() {
            let (row, col) = (k / cols, k % cols);
            let c = Rect::new(
                view.x + col as f32 * (cw + GAP),
                view.y + 4.0 + row as f32 * (CARD_H + GAP) - off,
                cw,
                CARD_H,
            );
            let before = self.items.len();
            if c.bottom() >= view.y - 8.0 && c.y <= view.bottom() + 8.0 {
                self.card(ui, i, c, any_selected, shift);
            }
            if self.items.len() != before {
                // A card removed itself; the indices moved, so stop until the next frame.
                break;
            }
        }
        widgets::scroll_end(ui, sid, view, content);
        if ui.input.drag_hover {
            ui.stroke_rrect_w(area.inset(-4.0), size::R_PANEL, t.accent, 2.0);
            let pill = Rect::new(area.center().x - 80.0, area.bottom() - 52.0, 160.0, 36.0);
            ui.glass(pill, 18.0, t.accent, 10.0);
            ui.text_in(
                pill,
                "Drop to add",
                size::BODY,
                Weight::Semibold,
                t.on_accent,
                Align::Center,
            );
        }
    }

    fn status_bar(&mut self, ui: &mut Ui, r: Rect) {
        let t = ui.theme;
        ui.hline(r.x, r.right(), r.y, t.divider);
        let mut row = r.inset_xy(16.0, 0.0);
        let right = row.cut_right(row.w / 2.0);
        let total: u64 = self.items.iter().map(|i| i.bytes).sum();
        let left = if self.items.is_empty() {
            "Originals are never changed. New files go next to them.".to_string()
        } else {
            format!("{} files · {}", self.items.len(), human(total))
        };
        ui.text_in(
            row,
            &left,
            size::CAPTION,
            Weight::Regular,
            t.text2,
            Align::Left,
        );
        let e = self.engines.clone();
        ui.text_in(
            right,
            &e,
            size::CAPTION,
            Weight::Regular,
            t.text3,
            Align::Right,
        );
    }

    fn toast(&mut self, ui: &mut Ui, area: Rect) {
        let Some(toast) = &self.toast else { return };
        let age = (ui.input.time - toast.at) as f32;
        let life = if toast.show.is_some() { 8.0 } else { 5.0 };
        if age > life {
            self.toast = None;
            return;
        }
        ui.request_repaint((life - age).max(0.05));
        let t = ui.theme;
        let (text, error, show) = (toast.text.clone(), toast.error, toast.show.clone());
        let extra = if show.is_some() { 132.0 } else { 0.0 };
        let w = (ui.measure(&text, size::BODY, Weight::Regular) + 56.0 + extra).min(area.w - 32.0);
        let r = Rect::new(area.center().x - w / 2.0, area.bottom() - 60.0, w, 44.0);
        let fill = if t.dark {
            Rgba::hexa(0x2C2C2E, 245)
        } else {
            Rgba::hexa(0xFFFFFF, 248)
        };
        ui.glass(r, 12.0, fill, 16.0);
        let mut rr = r.inset_xy(14.0, 0.0);
        let ic = rr.cut_left_gap(18.0, 10.0);
        ui.icon(
            ic.centered(18.0, 18.0),
            if error { Icon::Warning } else { Icon::Check },
            if error { t.error } else { t.success },
        );
        if let Some(p) = show {
            let b = rr.cut_right(124.0).centered(124.0, 30.0);
            if widgets::button(
                ui,
                Id::new("toast-show"),
                b,
                "Show in folder",
                None,
                ButtonKind::Secondary,
                true,
            )
            .clicked
            {
                self.requests.push(Request::ShowInFolder(p));
                self.toast = None;
            }
        }
        ui.text_in(rr, &text, size::BODY, Weight::Regular, t.text, Align::Left);
    }
}

impl App for Board {
    fn title(&self) -> String {
        "Swirvo Board".into()
    }

    fn min_size(&self) -> (f32, f32) {
        (760.0, 520.0)
    }

    fn on_event(&mut self, ev: HostEvent) {
        match ev {
            // A file chosen for a form field (a watermark picture), not for the list.
            HostEvent::FilesPicked(v) if self.pick_for.is_some() => {
                if let (Some(i), Some(f), Some(form)) =
                    (self.pick_for.take(), v.first(), self.form.as_mut())
                    && let Some(slot) = form.values.get_mut(i)
                {
                    *slot = f.to_string_lossy().into_owned();
                }
            }
            HostEvent::OpenTool(id) => self.open_tool(&id),
            HostEvent::FilesPicked(v) | HostEvent::AddFiles(v) => self.add_paths(v),
            HostEvent::FolderPicked(d) => {
                self.out_dir = Some(d);
                self.save_to = 1;
            }
        }
    }

    fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn frame(&mut self, ui: &mut Ui) {
        self.take_added(ui);
        self.poll_run(ui);
        if self.thumbs.poll() {
            ui.request_repaint(0.0);
        }
        if !ui.input.dropped.is_empty() {
            let v = std::mem::take(&mut ui.input.dropped);
            self.add_paths(v);
        }
        if ui.input.ctrl_key(Key::V) {
            // Paste is handled by the host as dropped files; nothing to do here.
        }
        self.keys(ui);
        let mut r = ui.bounds();
        let title = r.cut_top(TITLE_H);
        let status = r.cut_bottom(STATUS_H);
        let body = r.inset_xy(12.0, 0.0);
        let mut body = Rect::new(body.x, body.y, body.w, body.h - 12.0);
        let side = body.cut_left_gap(SIDEBAR_W, 16.0);
        self.title_bar(ui, title);
        let main = body;
        self.sync_preview();
        let previewing = !self.preview_hidden && self.preview.is_some() && self.form.is_some();
        if previewing {
            if let (Some(p), Some(f)) = (self.preview.as_mut(), self.form.as_mut())
                && !p.draw(ui, main, f)
            {
                self.preview_hidden = true;
            }
        } else if self.items.is_empty() {
            if self.adding.is_empty() {
                self.empty_state(ui, main);
            } else {
                widgets::spinner(ui, main.center(), 14.0, ui.theme.accent);
            }
        } else {
            self.grid(ui, main);
        }
        self.sidebar(ui, side);
        self.status_bar(ui, status);
        self.toast(ui, main);
        self.thumbs.end_frame();
        if self.thumbs.busy() || !self.adding.is_empty() || self.running() {
            // Results arrive through the waker; this is only a safety net.
            ui.request_repaint(0.5);
        }
    }
}

impl Board {
    /// For tests and the snapshot example: wait for background adds to land.
    pub fn settle(&mut self, ui: &Ui) {
        for _ in 0..400 {
            self.take_added(ui);
            if self.adding.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// For tests: run a conversion to completion.
    pub fn convert_and_wait(&mut self, ui: &Ui) {
        self.start_convert(ui);
        for _ in 0..2000 {
            self.poll_run(ui);
            if !self.running() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    pub fn select(&mut self, idx: &[usize]) {
        for (i, it) in self.items.iter_mut().enumerate() {
            it.selected = idx.contains(&i);
        }
    }

    pub fn set_target(&mut self, from: &str, to: &str) -> bool {
        let (Some(f), Some(t)) = (self.tk.registry.id(from), self.tk.registry.id(to)) else {
            return false;
        };
        let Some(pos) = self.targets_for(f).ids.iter().position(|&x| x == t) else {
            return false;
        };
        self.choice.insert(f, pos);
        true
    }

    /// (name, status label) per item, for tests.
    pub fn statuses(&self) -> Vec<(String, String)> {
        self.items
            .iter()
            .map(|i| {
                let s = match &i.status {
                    Status::Ready => "ready".to_string(),
                    Status::Queued => "queued".to_string(),
                    Status::Done { output, .. } => format!("done:{}", file_name(output)),
                    Status::Failed(e) => format!("failed:{e}"),
                    Status::Skipped(r) => format!("skipped:{r}"),
                };
                (i.name.clone(), s)
            })
            .collect()
    }

    /// For tests: type `q` into the search, on the Tools tab or the Convert tab.
    pub fn set_search(&mut self, q: &str, tools_tab: bool) {
        self.search = q.to_string();
        self.tab = usize::from(tools_tab);
        self.form = None;
    }

    /// For tests: the names of the files the search leaves on the Board.
    pub fn shown_files(&self) -> Vec<String> {
        let all: Vec<String> = self.items.iter().map(|i| file_name(&i.path)).collect();
        let hit: Vec<String> = all.iter().filter(|n| self.found(&[n])).cloned().collect();
        if hit.is_empty() { all } else { hit }
    }

    /// For tests: set an option of the open tool's form.
    pub fn form_set(&mut self, key: &str, value: &str) {
        if let Some(f) = &mut self.form {
            f.set(key, value);
        }
    }

    /// For tests: start the open tool's preview and wait for its first result: (page of
    /// the output shown, output page count). None when there's no preview or it failed.
    pub fn preview_wait(&mut self) -> Option<(usize, usize)> {
        self.sync_preview();
        let (p, f) = (self.preview.as_mut()?, self.form.as_ref()?);
        p.settle(f)
    }

    pub fn paths(&self) -> Vec<&Path> {
        self.items.iter().map(|i| i.path.as_path()).collect()
    }
}
