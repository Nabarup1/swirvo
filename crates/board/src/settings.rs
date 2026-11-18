//! The Settings window (docs/12 §11): `Swirvo.exe --settings`, the tray's "Settings…", or
//! the gear in the Board's title bar. Sections on the left, their settings on the right.
//!
//! Changes apply as they're made: each one is saved (atomically, off the UI thread) and the
//! host is asked to tell the other Swirvo windows and the agent ([`Request::SettingsChanged`]),
//! which re-read the file. Detecting FFmpeg, LibreOffice and Office runs on a background
//! thread when the window opens.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use swirvo_jobs::settings::{self, MAX_JOBS, PresetChoice, Settings, ThemeChoice};
use swirvo_ui::theme::size;
use swirvo_ui::widgets::{self, ButtonKind};
use swirvo_ui::{Align, App, HostEvent, Icon, Id, Pixmap, Rect, Request, Ui, Waker, Weight};

const TITLE_H: f32 = 48.0;
const NAV_W: f32 = 200.0;
const ROW_GAP: f32 = 10.0;

const SECTIONS: [&str; 6] = ["General", "Output", "Wheel", "Engines", "Privacy", "About"];

/// What the platform can do about the install (Windows only; elsewhere nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstallState {
    /// Not offered here (Linux, tests).
    #[default]
    Unavailable,
    /// Running from a copied folder: "Install Swirvo" adds it to the Start menu.
    NotInstalled,
    /// Installed for this user under Programs\Swirvo.
    Installed,
    /// Installed from the Microsoft Store (an MSIX package): the Store handles it.
    Store,
}

/// Engine packs found on this PC: (FFmpeg, LibreOffice, Office installed).
type Found = (Option<PathBuf>, Option<PathBuf>, bool);

pub struct SettingsApp {
    s: Settings,
    section: usize,
    found: Option<Found>,
    found_rx: Option<Receiver<Found>>,
    /// The folder with Swirvo.exe and THIRD-PARTY-NOTICES.txt (None in tests).
    app_dir: Option<PathBuf>,
    install: InstallState,
    note: Option<(String, bool)>,
    requests: Vec<Request>,
    icon: Option<Pixmap>,
    content_h: f32,
}

impl SettingsApp {
    pub fn new(waker: Waker, app_dir: Option<PathBuf>, install: InstallState) -> SettingsApp {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(swirvo_engines::found_packs());
            waker();
        });
        SettingsApp {
            s: (*settings::current()).clone(),
            section: 0,
            found: None,
            found_rx: Some(rx),
            app_dir,
            install,
            note: None,
            requests: Vec::new(),
            icon: crate::app_icon(64),
            content_h: 0.0,
        }
    }

    /// Open on `section` (0 General … 5 About).
    pub fn show(&mut self, section: usize) {
        self.section = section.min(SECTIONS.len() - 1);
    }

    /// For tests: the settings as edited.
    pub fn settings(&self) -> &Settings {
        &self.s
    }

    /// Save the edited settings, then let the other windows know.
    fn changed(&mut self) {
        // A few hundred bytes to the local disk, saved before the others are told to reload.
        if let Err(e) = settings::save(&self.s) {
            self.note = Some((format!("Couldn't save the settings: {e}"), true));
        }
        self.requests.push(Request::SettingsChanged);
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.found_rx
            && let Ok(f) = rx.try_recv()
        {
            self.found = Some(f);
            self.found_rx = None;
        }
    }

    /// A label with a line of explanation on the left of `r`; the control goes on the right.
    fn label(ui: &mut Ui, r: Rect, title: &str, desc: &str) -> f32 {
        let t = ui.theme;
        let mut col = r;
        let top = col.cut_top(22.0);
        ui.text_in(
            top,
            title,
            size::BODY,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        let lines = if desc.is_empty() {
            Vec::new()
        } else {
            ui.fonts.wrap(desc, size::CAPTION, Weight::Regular, r.w)
        };
        for l in &lines {
            let lr = col.cut_top(17.0);
            ui.text_in(lr, l, size::CAPTION, Weight::Regular, t.text2, Align::Left);
        }
        22.0 + 17.0 * lines.len() as f32
    }

    /// A row with a check box on the right. Returns true when it was clicked.
    #[allow(clippy::too_many_arguments)]
    fn toggle(
        ui: &mut Ui,
        id: Id,
        y: &mut f32,
        x: f32,
        w: f32,
        title: &str,
        desc: &str,
        on: bool,
    ) -> bool {
        let text = Rect::new(x, *y, w - 60.0, 200.0);
        let h = Self::label(ui, text, title, desc).max(32.0);
        let cb = Rect::new(x + w - 28.0, *y + (h - 22.0) / 2.0, 22.0, 22.0);
        let clicked = widgets::checkbox(ui, id, cb, on).clicked;
        *y += h + ROW_GAP * 1.6;
        clicked
    }

    /// A row with a control of width `cw` on the right; returns its rectangle.
    fn control_row(
        ui: &mut Ui,
        y: &mut f32,
        x: f32,
        w: f32,
        cw: f32,
        title: &str,
        desc: &str,
    ) -> Rect {
        let text = Rect::new(x, *y, w - cw - 24.0, 200.0);
        let h = Self::label(ui, text, title, desc).max(34.0);
        let c = Rect::new(x + w - cw, *y + (h - 32.0) / 2.0, cw, 32.0);
        *y += h + ROW_GAP * 1.6;
        c
    }

    fn heading(ui: &mut Ui, y: &mut f32, x: f32, w: f32, text: &str) {
        let t = ui.theme;
        ui.text_in(
            Rect::new(x, *y, w, 30.0),
            text,
            size::SUBTITLE,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        *y += 44.0;
    }

    fn general(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        let installed = matches!(self.install, InstallState::Installed | InstallState::Store);
        let start_desc = if installed || self.install == InstallState::Unavailable {
            "Swirvo waits in the notification area, ready for the wheel."
        } else {
            "Starts this copy. Install Swirvo below to keep it in one place and in the Start menu."
        };
        if Self::toggle(
            ui,
            Id::new("s-start"),
            y,
            x,
            w,
            "Start with Windows",
            start_desc,
            self.s.start_with_windows,
        ) {
            self.s.start_with_windows = !self.s.start_with_windows;
            self.changed();
        }
        if Self::toggle(
            ui,
            Id::new("s-tip"),
            y,
            x,
            w,
            "Show a tip when Swirvo starts",
            "A short reminder of the gesture in the notification area.",
            self.s.startup_tip,
        ) {
            self.s.startup_tip = !self.s.startup_tip;
            self.changed();
        }
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            240.0,
            "Theme",
            "Follow Windows, or always light or dark.",
        );
        let mut sel = match self.s.theme {
            ThemeChoice::System => 0,
            ThemeChoice::Light => 1,
            ThemeChoice::Dark => 2,
        };
        if widgets::segmented(
            ui,
            Id::new("s-theme"),
            c,
            &["System", "Light", "Dark"],
            &mut sel,
        ) {
            self.s.theme = [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark][sel.min(2)];
            self.changed();
        }
        match self.install {
            InstallState::NotInstalled => {
                let c = Self::control_row(
                    ui,
                    y,
                    x,
                    w,
                    170.0,
                    "Install Swirvo",
                    "Copies Swirvo to your user folder and adds it to the Start menu and to Settings → Apps. No administrator rights needed.",
                );
                if widgets::button(
                    ui,
                    Id::new("s-install"),
                    c,
                    "Install",
                    Some(Icon::Add),
                    ButtonKind::Primary,
                    true,
                )
                .clicked
                {
                    self.requests.push(Request::Install);
                }
            }
            InstallState::Installed => {
                let c = Self::control_row(
                    ui,
                    y,
                    x,
                    w,
                    170.0,
                    "Uninstall Swirvo",
                    "Removes Swirvo, its Start menu entry and starting with Windows. It asks before deleting your settings.",
                );
                if widgets::button(
                    ui,
                    Id::new("s-uninstall"),
                    c,
                    "Uninstall…",
                    Some(Icon::Trash),
                    ButtonKind::Secondary,
                    true,
                )
                .clicked
                {
                    self.requests.push(Request::Uninstall);
                }
            }
            InstallState::Store => {
                Self::label(
                    ui,
                    Rect::new(x, *y, w, 80.0),
                    "Installed from the Microsoft Store",
                    "Updates and uninstalling are handled by Windows.",
                );
                *y += 56.0;
            }
            InstallState::Unavailable => {}
        }
    }

    fn output(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            260.0,
            "Save new files",
            "Originals are never changed or replaced, wherever new files go.",
        );
        let opts = vec![
            "Next to the original".to_string(),
            "In a folder I choose".to_string(),
        ];
        let mut sel = usize::from(self.s.out_dir.is_some());
        if widgets::dropdown(ui, Id::new("s-where"), c, &opts, &mut sel) {
            if sel == 1 {
                self.requests.push(Request::PickFolder);
            } else {
                self.s.out_dir = None;
                self.changed();
            }
        }
        if let Some(d) = self.s.out_dir.clone() {
            let t = ui.theme;
            let mut r = Rect::new(x, *y - ROW_GAP, w, 32.0);
            let change = r.cut_right(110.0);
            ui.icon(Rect::new(r.x, r.y + 8.0, 16.0, 16.0), Icon::Folder, t.text2);
            ui.text_in(
                Rect::new(r.x + 24.0, r.y, r.w - 24.0, r.h),
                &d.display().to_string(),
                size::CAPTION,
                Weight::Regular,
                t.text2,
                Align::Left,
            );
            if widgets::button(
                ui,
                Id::new("s-choose"),
                change,
                "Choose…",
                None,
                ButtonKind::Ghost,
                true,
            )
            .clicked
            {
                self.requests.push(Request::PickFolder);
            }
            *y += 32.0 + ROW_GAP;
        }
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            300.0,
            "Quality",
            "Best keeps the most detail, Smallest makes the smallest files. Each wheel wedge and the Board can still pick another.",
        );
        let mut sel = match self.s.preset {
            PresetChoice::Best => 0,
            PresetChoice::Balanced => 1,
            PresetChoice::Smallest => 2,
        };
        if widgets::segmented(
            ui,
            Id::new("s-preset"),
            c,
            &["Best", "Balanced", "Smallest"],
            &mut sel,
        ) {
            self.s.preset = [
                PresetChoice::Best,
                PresetChoice::Balanced,
                PresetChoice::Smallest,
            ][sel.min(2)];
            self.changed();
        }
        if Self::toggle(
            ui,
            Id::new("s-show"),
            y,
            x,
            w,
            "Show the new file in its folder",
            "After a conversion from the wheel, open Explorer with the new file selected.",
            self.s.show_in_folder,
        ) {
            self.s.show_in_folder = !self.s.show_in_folder;
            self.changed();
        }
    }

    fn wheel(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        let t = ui.theme;
        Self::label(
            ui,
            Rect::new(x, *y, w, 60.0),
            "How the wheel opens",
            "Start dragging a file, keep the mouse button down, and tap a key:",
        );
        *y += 46.0;
        for (key, what) in [
            ("Shift", "Convert: formats it can become"),
            ("Ctrl", "Tools: resize, compress, PDF pages, merge…"),
            ("Alt", "Actions: show in folder, copy path, terminal, Board"),
        ] {
            let mut r = Rect::new(x, *y, w, 28.0);
            let k = r.cut_left_gap(64.0, 12.0);
            ui.fill_rrect(k.centered(60.0, 24.0), size::R_CONTROL, t.control);
            ui.stroke_rrect(k.centered(60.0, 24.0), size::R_CONTROL, t.control_stroke);
            ui.text_in(
                k,
                key,
                size::CAPTION,
                Weight::Semibold,
                t.text,
                Align::Center,
            );
            ui.text_in(r, what, size::BODY, Weight::Regular, t.text2, Align::Left);
            *y += 32.0;
        }
        *y += ROW_GAP * 2.0;
        if Self::toggle(
            ui,
            Id::new("s-pause"),
            y,
            x,
            w,
            "Pause the wheel",
            "Shift, Ctrl and Alt work as usual while dragging, for apps that need them. The tray menu has this too.",
            self.s.wheel_paused,
        ) {
            self.s.wheel_paused = !self.s.wheel_paused;
            self.changed();
        }
    }

    fn engines(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        let t = ui.theme;
        let found = |p: &Option<Option<PathBuf>>| match p {
            None => "Looking…".to_string(),
            Some(Some(p)) => format!("Found: {}", p.display()),
            Some(None) => "Not installed".to_string(),
        };
        let (ff, lo, office) = match &self.found {
            Some((f, l, o)) => (Some(f.clone()), Some(l.clone()), Some(*o)),
            None => (None, None, None),
        };
        let office_desc = match office {
            None => "Looking…".to_string(),
            Some(true) => "Found Word, Excel or PowerPoint. Used for old .doc, .xls and .ppt files, and when our own engines can't open a file.".to_string(),
            Some(false) => "Not installed".to_string(),
        };
        let rows: [(&str, String, bool, u8); 3] = [
            ("Use Microsoft Office", office_desc, self.s.use_office, 0),
            (
                "Use LibreOffice",
                format!(
                    "{}. Used when our own engines can't open a document.",
                    found(&lo)
                ),
                self.s.use_libreoffice,
                1,
            ),
            (
                "Use FFmpeg",
                format!(
                    "{}. Adds video and audio formats beyond what Windows plays.",
                    found(&ff)
                ),
                self.s.use_ffmpeg,
                2,
            ),
        ];
        for (title, desc, on, k) in rows {
            if Self::toggle(
                ui,
                Id::new("s-engine").with(k as usize),
                y,
                x,
                w,
                title,
                &desc,
                on,
            ) {
                match k {
                    0 => self.s.use_office = !self.s.use_office,
                    1 => self.s.use_libreoffice = !self.s.use_libreoffice,
                    _ => self.s.use_ffmpeg = !self.s.use_ffmpeg,
                }
                self.changed();
                self.note = Some(("Engine changes apply to new Board windows now, and to the wheel the next time Swirvo starts.".into(), false));
            }
        }
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            150.0,
            "Files at once",
            "How many files are converted at the same time. Auto uses one per processor core; fewer leaves the PC more responsive.",
        );
        let mut labels = vec!["Auto".to_string()];
        labels.extend((1..=MAX_JOBS).map(|n| n.to_string()));
        let mut sel = self.s.jobs.min(MAX_JOBS);
        if widgets::dropdown(ui, Id::new("s-jobs"), c, &labels, &mut sel) {
            self.s.jobs = sel;
            self.changed();
        }
        let _ = t;
    }

    fn privacy(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        if Self::toggle(
            ui,
            Id::new("s-offline"),
            y,
            x,
            w,
            "Offline mode",
            "Swirvo never connects to the internet. Future update checks will only run if you turn this off. Files are always converted on this PC.",
            self.s.offline,
        ) {
            self.s.offline = !self.s.offline;
            self.changed();
        }
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            150.0,
            "Recent formats",
            "The wheel puts the formats you use most first. This history stays on this PC.",
        );
        if widgets::button(
            ui,
            Id::new("s-clear"),
            c,
            "Clear history",
            Some(Icon::Broom),
            ButtonKind::Secondary,
            true,
        )
        .clicked
        {
            let _ = settings::clear_history();
            self.requests.push(Request::HistoryCleared);
            self.note = Some(("History cleared.".into(), false));
        }
    }

    fn about(&mut self, ui: &mut Ui, y: &mut f32, x: f32, w: f32) {
        let t = ui.theme;
        if let Some(icon) = &self.icon {
            let icon = icon.clone();
            ui.image(Rect::new(x, *y, 56.0, 56.0), &icon, 0.0, false);
        }
        ui.text_in(
            Rect::new(x + 72.0, *y + 2.0, w - 72.0, 28.0),
            "Swirvo",
            size::SUBTITLE,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        ui.text_in(
            Rect::new(x + 72.0, *y + 30.0, w - 72.0, 22.0),
            &format!("Version {}", env!("CARGO_PKG_VERSION")),
            size::BODY,
            Weight::Regular,
            t.text2,
            Align::Left,
        );
        *y += 76.0;
        let notices = self
            .app_dir
            .as_ref()
            .map(|d| d.join("THIRD-PARTY-NOTICES.txt"))
            .filter(|p| p.is_file());
        let c = Self::control_row(
            ui,
            y,
            x,
            w,
            150.0,
            "Open-source licences",
            "Swirvo is built with open-source components; their licences and notices.",
        );
        if widgets::button(
            ui,
            Id::new("s-notices"),
            c,
            "Show",
            None,
            ButtonKind::Secondary,
            notices.is_some(),
        )
        .clicked
            && let Some(p) = notices
        {
            self.requests.push(Request::Open(p));
        }
        Self::label(
            ui,
            Rect::new(x, *y, w, 80.0),
            "Privacy",
            "Swirvo works on your files only on this PC. It collects no data, shows no ads and sends nothing anywhere. The full privacy policy is published with the app's store listing.",
        );
        *y += 80.0;
    }
}

impl App for SettingsApp {
    fn title(&self) -> String {
        "Swirvo Settings".into()
    }

    fn min_size(&self) -> (f32, f32) {
        (720.0, 520.0)
    }

    fn on_event(&mut self, ev: HostEvent) {
        if let HostEvent::FolderPicked(d) = ev {
            self.s.out_dir = Some(d);
            self.changed();
        }
    }

    fn take_requests(&mut self) -> Vec<Request> {
        std::mem::take(&mut self.requests)
    }

    fn frame(&mut self, ui: &mut Ui) {
        self.poll();
        let t = ui.theme;
        if !ui.has_any_focus() && ui.input.key(swirvo_ui::Key::Escape) {
            self.requests.push(Request::Close);
        }
        let mut r = ui.bounds();
        let title = r.cut_top(TITLE_H);
        widgets::caption(ui, title);
        let mut tr = title;
        tr.cut_left(16.0);
        let ic = tr.cut_left_gap(22.0, 10.0);
        if let Some(icon) = &self.icon {
            let icon = icon.clone();
            ui.image(ic.centered(22.0, 22.0), &icon, 0.0, false);
        }
        ui.text_in(
            tr,
            "Settings",
            size::BODY_L,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        widgets::window_controls(ui, title, false);

        let body = r.inset_xy(12.0, 0.0);
        let mut body = Rect::new(body.x, body.y, body.w, body.h - 12.0);
        let nav = body.cut_left_gap(NAV_W, 12.0);
        ui.glass(nav, size::R_PANEL, t.panel, 18.0);
        let mut n = nav.inset(10.0);
        for (i, name) in SECTIONS.iter().enumerate() {
            let row = n.cut_top_gap(38.0, 4.0);
            let id = Id::new("s-nav").with(i);
            let resp = ui.interact(id, row);
            let on = i == self.section;
            if on {
                ui.fill_rrect(row, size::R_CONTROL, t.accent_wash);
            } else if resp.hovered {
                ui.fill_rrect(row, size::R_CONTROL, t.control_hover);
            }
            ui.text_in(
                row.inset_xy(12.0, 0.0),
                name,
                size::BODY,
                if on {
                    Weight::Semibold
                } else {
                    Weight::Regular
                },
                if on { t.text } else { t.text2 },
                Align::Left,
            );
            if resp.clicked {
                self.section = i;
                self.note = None;
            }
        }

        ui.glass(body, size::R_PANEL, t.panel, 18.0);
        let mut view = body.inset(24.0);
        if let Some((msg, err)) = self.note.clone() {
            let line = view.cut_bottom(24.0);
            ui.text_in(
                line,
                &msg,
                size::CAPTION,
                Weight::Regular,
                if err { t.error } else { t.text2 },
                Align::Left,
            );
        }
        let sid = Id::new("s-scroll").with(self.section);
        let off = widgets::scroll_begin(ui, sid, view, self.content_h);
        let (x, w) = (view.x, view.w.min(640.0));
        let start = view.y - off;
        let mut y = start;
        Self::heading(ui, &mut y, x, w, SECTIONS[self.section]);
        match self.section {
            0 => self.general(ui, &mut y, x, w),
            1 => self.output(ui, &mut y, x, w),
            2 => self.wheel(ui, &mut y, x, w),
            3 => self.engines(ui, &mut y, x, w),
            4 => self.privacy(ui, &mut y, x, w),
            _ => self.about(ui, &mut y, x, w),
        }
        self.content_h = y - start;
        widgets::scroll_end(ui, sid, view, self.content_h);
        if self.found.is_none() {
            ui.request_repaint(0.5);
        }
    }
}
