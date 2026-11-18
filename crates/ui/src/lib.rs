//! `swirvo-ui`: a small immediate-mode UI toolkit on tiny-skia (docs/12 §5).
//!
//! Portable: no OS calls. A host (the Win32 window in `platform/win`, or a snapshot in
//! tests) feeds [`Input`], calls [`App::frame`] between [`Ui::begin_frame`] and
//! [`Ui::end_frame`], and presents [`Ui::pixmap`] (premultiplied RGBA).

mod ctx;
pub mod geom;
mod icons;
pub mod input;
mod paint;
pub mod text;
pub mod theme;
pub mod widgets;

use std::path::PathBuf;
use std::sync::Arc;

pub use ctx::{Cursor, Id, Response, Ui};
pub use geom::{Point, Rect};
pub use icons::Icon;
pub use input::{Input, Key, Mods, Zone};
pub use paint::{Align, fake_backdrop};
pub use text::{Fonts, Weight};
pub use theme::{Kind, Rgba, Theme};
pub use tiny_skia::{IntSize as PixSize, Path, PathBuilder, Pixmap};

/// Something the app asks the host to do after the frame (never inside it).
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    PickFiles,
    PickFolder,
    ShowInFolder(PathBuf),
    Open(PathBuf),
    CopyText(String),
    SetTitle(String),
    /// Open the page picker on `file`, starting on `tool`'s operation; `spec` carries
    /// what was typed so far (Split ranges: the groups).
    OpenPages {
        tool: String,
        file: PathBuf,
        spec: String,
    },
    /// Open the Settings window.
    OpenSettings,
    /// Settings were saved: the host tells the other Swirvo windows and the agent.
    SettingsChanged,
    /// The recent-format history was cleared: the agent forgets its copy too.
    HistoryCleared,
    /// Install Swirvo for this user (Start menu, Apps list).
    Install,
    /// Uninstall Swirvo (asks first).
    Uninstall,
    Close,
}

/// What the host tells the app.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    FilesPicked(Vec<PathBuf>),
    FolderPicked(PathBuf),
    /// Files handed over by a second launch (single-instance Board).
    AddFiles(Vec<PathBuf>),
    /// Show a tool's options (a tool id), or the tool list (empty), from the wheel.
    OpenTool(String),
}

/// Wakes the host from a background thread (posts a message to the window).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

pub trait App {
    fn title(&self) -> String;
    /// Build and paint one frame.
    fn frame(&mut self, ui: &mut Ui);
    fn on_event(&mut self, ev: HostEvent);
    /// Requests produced by the last frame.
    fn take_requests(&mut self) -> Vec<Request>;
    /// Minimum window size in logical px.
    fn min_size(&self) -> (f32, f32) {
        (640.0, 420.0)
    }
}

/// Render `app` headlessly at `w` × `h` logical px over a fake glass backdrop: for
/// snapshot tests and design review. Runs `frames` frames (animations settle at ~10).
pub fn snapshot(
    app: &mut dyn App,
    ui: &mut Ui,
    w: f32,
    h: f32,
    scale: f32,
    frames: usize,
) -> Pixmap {
    let (pw, ph) = ((w * scale).round() as u32, (h * scale).round() as u32);
    ui.backdrop = true;
    for i in 0..frames.max(1) {
        ui.input.dt = if i == 0 { 0.0 } else { 0.25 };
        ui.input.time += ui.input.dt as f64;
        ui.begin_frame(pw, ph, scale);
        app.frame(ui);
        ui.end_frame();
    }
    let mut out = Pixmap::new(pw.max(1), ph.max(1)).unwrap_or_else(|| ui.pixmap().clone());
    fake_backdrop(&mut out, ui.theme.dark);
    out.draw_pixmap(
        0,
        0,
        ui.pixmap().as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    out
}

#[cfg(test)]
mod tests;
