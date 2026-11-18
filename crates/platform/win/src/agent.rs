//! Win32 overlay, spike S1 (docs/03 §3, docs/04 §5, docs/08).
//!
//! One hidden, layered, topmost, never-activated popup does everything:
//! - **Trigger.** Raw Input (`RIDEV_INPUTSINK`) sees Shift / Ctrl / Alt go down anywhere,
//!   with no hook DLL. If a mouse button is held, the gesture machine starts probing.
//! - **Probe.** The window appears as a 64 px square of alpha-1 pixels under the cursor and
//!   the cursor is nudged 1 px and back, so the drag source's OLE loop hit-tests us at once.
//!   If a drag with files is in progress we get `IDropTarget::DragEnter`; otherwise the probe
//!   times out after 150 ms and nobody ever saw it.
//! - **Wheel.** The same window grows to the wheel canvas; tiny-skia renders it and
//!   `UpdateLayeredWindow` presents premultiplied BGRA. Alpha-1 pixels around the rim keep
//!   drag events flowing in the push-through band.
//! - **Commit.** The drop is answered with COPY or NONE (never MOVE) and the work runs on a
//!   background thread. This spike runs conversions in-process; production moves them into
//!   the sandboxed `swirvo-worker` (PROJECT_GUIDE.md rule 3).
//!
//! Re-entrancy: COM calls can pump messages, so agent state lives in a thread-local
//! `RefCell` that is only ever `try_borrow_mut`ed, and no borrow is held across a COM call.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use swirvo_core::CancelToken;
use swirvo_jobs::{JobEvent, Options, Toolkit, load_frecency, save_frecency, tools};
use swirvo_render::{Pixmap, Renderer, Scene, Theme, load_ui_font};
use swirvo_wheel::Frecency;
use swirvo_wheel::fsm::{DropEffect, Effect, Event, Machine, State, Wheel};
use windows::Win32::Foundation::{
    COLORREF, HWND, LPARAM, LRESULT, POINT, POINTL, RECT, SIZE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IDataObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize,
    RegisterDragDrop,
};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LBUTTON,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_MENU, VK_RBUTTON, VK_RCONTROL, VK_RMENU, VK_RSHIFT,
    VK_SHIFT,
};
use windows::Win32::UI::Input::{
    GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RID_INPUT,
    RIDEV_INPUTSINK, RIM_TYPEKEYBOARD, RegisterRawInputDevices,
};
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, IDropTargetHelper, ILCreateFromPathW, ILFree, SHOpenFolderAndSelectItems,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetMessageW,
    GetSystemMetrics, HWND_TOPMOST, KillTimer, MSG, RegisterClassExW, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow, TranslateMessage,
    ULW_ALPHA, UpdateLayeredWindow, WM_INPUT, WM_TIMER, WNDCLASSEXW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};
use windows_core::{PCWSTR, Ref, implement, w};

use crate::model::{Menu, Placement, Work};
use crate::tray;
use crate::util::{Surface, dropped_files, set_clipboard, wide};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};

const TICK_TIMER: usize = 1;
const TICK_MS: u32 = 16;
const PROBE_PX: i32 = 64;
/// Opacity while the pointer is far away (docs/02 §3.8).
const FADED_ALPHA: u8 = 70;
/// A button must stay up this long before we call it "released without a drop": the drop
/// itself arrives over COM a few ms after the release, and must win the race.
const BUTTON_UP_GRACE_MS: u64 = 120;

struct Agent {
    hwnd: HWND,
    machine: Machine,
    tk: Arc<Toolkit>,
    frecency: Frecency,
    renderer: Renderer,
    /// Reused frame and scene: hover repaints allocate nothing.
    frame: Option<Pixmap>,
    scene: Option<Scene>,
    theme: Theme,
    epoch: Instant,
    /// Shift, Ctrl, Alt currently down (raw input repeats make-codes while held).
    down: [bool; 3],
    files: Vec<PathBuf>,
    menu: Option<Menu>,
    placement: Option<Placement>,
    hub_cancel: bool,
    faded: bool,
    surface: Option<Surface>,
    timer: bool,
    button_up_since: Option<u64>,
    trigger_at: Option<Instant>,
    /// Sub-ring labels for the Terminal wedge (`shells::all()` order).
    shells: Vec<String>,
}

thread_local! {
    static AGENT: RefCell<Option<Agent>> = const { RefCell::new(None) };
    static HELPER: RefCell<Option<IDropTargetHelper>> = const { RefCell::new(None) };
}

/// Run `f` on the agent unless it's already borrowed further up the stack (re-entrancy).
fn with_agent<R>(f: impl FnOnce(&mut Agent) -> R) -> Option<R> {
    AGENT.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn helper() -> Option<IDropTargetHelper> {
    HELPER.with(|h| h.borrow().clone())
}

fn log(msg: impl AsRef<str>) {
    println!("[swirvo] {}", msg.as_ref());
}

impl Agent {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    fn step(&mut self, ev: Event) -> Option<DropEffect> {
        let fx = self.machine.step(ev);
        self.apply(fx)
    }

    /// Perform effects; returns the drop answer if the step produced one.
    fn apply(&mut self, fx: Vec<Effect>) -> Option<DropEffect> {
        let mut answer = None;
        let mut dirty = false;
        let mut follow_up = Vec::new();
        for f in fx {
            match f {
                Effect::ShowProbe => self.show_probe(),
                Effect::HideProbe | Effect::HideWheel => self.hide(),
                Effect::ShowWheel(w) | Effect::SwitchWheel(w) => {
                    self.open_wheel(w);
                    dirty = true;
                }
                Effect::Highlight(_)
                | Effect::OpenSub(_)
                | Effect::CloseSub
                | Effect::HighlightSub(_) => dirty = true,
                Effect::HubCancel(on) => {
                    self.hub_cancel = on;
                    dirty = true;
                }
                Effect::Fade(on) => {
                    self.faded = on;
                    dirty = true;
                }
                Effect::DropResult(e) => answer = Some(e),
                Effect::Commit { wedge, preset, .. } => {
                    self.commit(wedge, preset);
                    follow_up.push(Event::JobAccepted { now: self.now() });
                }
                Effect::ShowReceipt | Effect::HideReceipt => {} // console receipt for the spike
            }
        }
        if dirty && matches!(self.machine.state(), State::Open(_)) {
            self.present_wheel();
        }
        for ev in follow_up {
            let fx = self.machine.step(ev);
            self.apply(fx);
        }
        if matches!(self.machine.state(), State::Idle | State::Receipt { .. })
            && self.menu.is_some()
        {
            self.end_session();
        }
        self.sync_timer();
        answer
    }

    fn sync_timer(&mut self) {
        let want = !matches!(self.machine.state(), State::Idle);
        if want != self.timer {
            // SAFETY: timer on our own window.
            unsafe {
                if want {
                    SetTimer(Some(self.hwnd), TICK_TIMER, TICK_MS, None);
                } else {
                    let _ = KillTimer(Some(self.hwnd), TICK_TIMER);
                }
            }
            self.timer = want;
            self.button_up_since = None;
        }
    }

    fn show_probe(&mut self) {
        let mut pt = POINT::default();
        // SAFETY: out-pointer to a local.
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return;
        }
        let (w, h) = (PROBE_PX, PROBE_PX);
        if !self.ensure_surface(w, h) {
            return;
        }
        if let Some(s) = self.surface.as_mut() {
            // Premultiplied black at alpha 1: invisible, but hit-testable.
            s.pixels().as_chunks_mut::<4>().0.fill([0, 0, 0, 1]);
        }
        self.present(
            POINT {
                x: pt.x - w / 2,
                y: pt.y - h / 2,
            },
            w,
            h,
            255,
        );
        self.nudge(pt);
    }

    /// Move the cursor 1 px and back with real input, so the drag source re-hit-tests now
    /// rather than on the user's next movement.
    fn nudge(&self, pt: POINT) {
        // SAFETY: plain metric queries and SendInput with a local array.
        unsafe {
            let (vx, vy) = (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
            );
            let (vw, vh) = (
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2),
            );
            let abs = |x: i32, y: i32| INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32,
                        dy: ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32,
                        dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                        ..Default::default()
                    },
                },
            };
            let inputs = [abs(pt.x + 1, pt.y), abs(pt.x, pt.y)];
            SendInput(&inputs, size_of::<INPUT>() as i32);
        }
    }

    fn hide(&mut self) {
        // SAFETY: our own window.
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Forget the gesture once it's over. Runs after *all* effects of a step, because a
    /// drop emits `HideWheel` before `Commit` and the commit still needs the menu.
    fn end_session(&mut self) {
        self.menu = None;
        self.scene = None;
        self.placement = None;
        self.files.clear();
        self.hub_cancel = false;
        self.faded = false;
    }

    fn open_wheel(&mut self, wheel: Wheel) {
        let menu = Menu::build(&self.tk, wheel, &self.files, &self.frecency, self.now())
            .with_shells(&self.shells);
        self.machine.set_layout(menu.layout());
        self.menu = Some(menu);
        self.scene = None;
        self.hub_cancel = false;
        if self.placement.is_none() {
            let mut pt = POINT::default();
            // SAFETY: out-pointers to locals; monitor handle from the system.
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(mon, &mut mi);
                let (mut dx, mut dy) = (96u32, 96u32);
                let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
                let RECT {
                    left,
                    top,
                    right,
                    bottom,
                } = mi.rcWork;
                self.placement = Some(Placement::around(
                    (pt.x, pt.y),
                    (left, top, right - left, bottom - top),
                    dx as f32 / 96.0,
                ));
            }
        }
    }

    fn present_wheel(&mut self) {
        let (Some(menu), Some(p)) = (self.menu.as_ref(), self.placement) else {
            return;
        };
        match self.scene.as_mut() {
            Some(sc) => menu.refresh(sc, self.machine.state(), self.hub_cancel),
            None => {
                self.scene =
                    Some(menu.scene(self.machine.state(), self.hub_cancel, self.theme, p.scale))
            }
        }
        let Some(scene) = self.scene.as_ref() else {
            return;
        };
        // Taken out and put back (no allocation) so `self` stays borrowable below.
        let Some(mut frame) = self.frame.take().or_else(|| self.renderer.render(scene)) else {
            return;
        };
        let ok = self.renderer.render_into(scene, &mut frame);
        let (w, h) = (frame.width() as i32, frame.height() as i32);
        if ok
            && self.ensure_surface(w, h)
            && let Some(s) = self.surface.as_mut()
        {
            crate::model::rgba_to_bgra(frame.data(), s.pixels());
        }
        self.frame = Some(frame);
        if !ok {
            return;
        }
        let alpha = if self.faded { FADED_ALPHA } else { 255 };
        self.present(
            POINT {
                x: p.left,
                y: p.top,
            },
            w,
            h,
            alpha,
        );
        if let Some(t) = self.trigger_at.take() {
            log(format!(
                "trigger → wheel on screen: {:.1} ms",
                t.elapsed().as_secs_f64() * 1e3
            ));
        }
    }

    fn ensure_surface(&mut self, w: i32, h: i32) -> bool {
        if self.surface.as_ref().is_some_and(|s| s.w == w && s.h == h) {
            return true;
        }
        self.surface = Surface::new(w, h);
        self.surface.is_some()
    }

    fn present(&mut self, pos: POINT, w: i32, h: i32, alpha: u8) {
        let Some(s) = self.surface.as_ref() else {
            return;
        };
        let size = SIZE { cx: w, cy: h };
        let src = POINT::default();
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: alpha,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        // SAFETY: our window and our memory DC; pointers to locals.
        unsafe {
            if let Err(e) = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&pos),
                Some(&size),
                Some(s.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            ) {
                log(format!("UpdateLayeredWindow failed: {e}"));
                return;
            }
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn commit(&mut self, wedge: usize, sub: Option<usize>) {
        let Some(menu) = self.menu.as_ref() else {
            return;
        };
        let work = crate::model::resolve(menu, wedge, sub, &self.files);
        if let (Work::Convert { to, .. }, Some(&first)) = (
            &work,
            self.files
                .first()
                .and_then(|f| self.tk.registry.from_path(f))
                .as_ref(),
        ) {
            let now = self.now();
            self.frecency
                .bump(self.tk.registry.key(first), self.tk.registry.key(*to), now);
            let snapshot = self.frecency.clone();
            std::thread::spawn(move || save_frecency(&snapshot));
        }
        match work {
            Work::CopyPath { text } => match set_clipboard(self.hwnd, &text) {
                Ok(()) => tray::notify("Path copied", &text, false),
                Err(e) => log(format!("couldn't copy: {e}")),
            },
            Work::Nothing => {}
            other => {
                let tk = Arc::clone(&self.tk);
                let hwnd = self.hwnd.0 as isize;
                std::thread::spawn(move || run_work(&tk, other, hwnd));
            }
        }
    }

    fn on_key(&mut self, vkey: u16, up: bool) {
        let slot = match VIRTUAL_KEY(vkey) {
            VK_SHIFT | VK_LSHIFT | VK_RSHIFT => Some((0, Wheel::Convert)),
            VK_CONTROL | VK_LCONTROL | VK_RCONTROL => Some((1, Wheel::Tools)),
            VK_MENU | VK_LMENU | VK_RMENU => Some((2, Wheel::Actions)),
            _ => None,
        };
        if let Some((i, wheel)) = slot {
            let was = std::mem::replace(&mut self.down[i], !up);
            if up || was {
                return; // release, or auto-repeat
            }
            // SAFETY: plain key-state queries.
            let held = unsafe {
                GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0
                    || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0
            };
            if held && matches!(self.machine.state(), State::Idle | State::Receipt { .. }) {
                self.trigger_at = Some(Instant::now());
            }
            // Settings → Wheel → Pause the wheel (or the tray menu): keys pass through.
            if swirvo_jobs::settings::current().wheel_paused {
                return;
            }
            let now = self.now();
            self.step(Event::Trigger {
                wheel,
                button_held: held,
                now,
            });
        } else if VIRTUAL_KEY(vkey) == VK_ESCAPE && !up {
            self.step(Event::Escape);
        }
    }

    fn on_tick(&mut self) {
        let now = self.now();
        if matches!(self.machine.state(), State::Probing { .. } | State::Open(_)) {
            // SAFETY: plain key-state queries.
            let held = unsafe {
                GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0
                    || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0
            };
            if held {
                self.button_up_since = None;
            } else {
                let since = *self.button_up_since.get_or_insert(now);
                if now.saturating_sub(since) >= BUTTON_UP_GRACE_MS {
                    self.button_up_since = None;
                    self.step(Event::ButtonUp { now });
                    return;
                }
            }
        }
        self.step(Event::Tick { now });
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Message carrying a `Box<String>` (in LPARAM) for the UI thread to put on the clipboard.
const WM_APP_CLIPBOARD: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 2;

/// Hand text to the UI thread, which owns the window the clipboard needs.
fn post_clipboard(hwnd: isize, text: String) {
    let ptr = Box::into_raw(Box::new(text));
    // SAFETY: on success the UI thread takes ownership of the box; on failure we reclaim it.
    unsafe {
        if PostMessageW(
            Some(HWND(hwnd as *mut c_void)),
            WM_APP_CLIPBOARD,
            WPARAM(0),
            LPARAM(ptr as isize),
        )
        .is_err()
        {
            drop(Box::from_raw(ptr));
        }
    }
}

fn run_work(tk: &Toolkit, work: Work, hwnd: isize) {
    match work {
        Work::Convert { files, to, preset } => {
            let opts = Options {
                preset,
                ..Options::from_settings()
            };
            let started = Instant::now();
            // The first new file, for "Show the new file in its folder" (Settings → Output).
            let first_out = std::sync::Mutex::new(None::<PathBuf>);
            let fmt = tk.registry.primary_ext(to).to_ascii_uppercase();
            // (first success line, first failure line, any note) for the notification.
            let first = std::sync::Mutex::new((None::<String>, None::<String>, false));
            let jobs: Vec<(PathBuf, swirvo_core::FormatId)> =
                files.iter().map(|f| (f.clone(), to)).collect();
            let s =
                swirvo_jobs::worker::convert_each(tk, &jobs, &opts, &CancelToken::new(), &|ev| {
                    let mut f = first.lock().unwrap_or_else(|p| p.into_inner());
                    match ev {
                        JobEvent::Done {
                            input,
                            output,
                            route,
                            ms,
                            lossless,
                            note,
                            ..
                        } => {
                            log(format!("✓ {} ({route}, {ms} ms)", output.display()));
                            first_out
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .get_or_insert_with(|| output.clone());
                            if let Some(n) = note {
                                log(format!("  note: {n}"));
                            }
                            let q = if *lossless { "lossless" } else { "lossy" };
                            // A note (saved to Downloads, fallback engine) matters more than speed.
                            f.0.get_or_insert_with(|| match note {
                                Some(n) => {
                                    format!("{} → {}. {n}", file_name(input), file_name(output))
                                }
                                None => format!(
                                    "{} → {} · {ms} ms · {q}",
                                    file_name(input),
                                    file_name(output)
                                ),
                            });
                            if note.is_some() {
                                f.2 = true;
                            }
                        }
                        JobEvent::Failed { input, error, .. } => {
                            log(format!("✗ {}: {error}", input.display()));
                            f.1.get_or_insert_with(|| format!("{}: {error}", file_name(input)));
                        }
                        JobEvent::Skipped { input, reason, .. } => {
                            log(format!("– {}: {reason}", input.display()))
                        }
                    }
                });
            let ms = started.elapsed().as_millis();
            log(format!(
                "{} done, {} failed, {} skipped in {ms} ms",
                s.done, s.failed, s.skipped
            ));
            let (ok, err, noted) = first.into_inner().unwrap_or_else(|p| p.into_inner());
            if swirvo_jobs::settings::current().show_in_folder
                && let Some(p) = first_out.into_inner().unwrap_or_else(|p| p.into_inner())
            {
                show_in_folder(&p);
            }
            match (s.done, s.failed) {
                (0, 0) => tray::notify("Nothing to convert", &format!("Already {fmt}"), false),
                (1, 0) => tray::notify(
                    &format!("Converted to {fmt}"),
                    &ok.unwrap_or_default(),
                    noted,
                ),
                (n, 0) if noted => tray::notify(
                    &format!("Converted {n} files to {fmt}"),
                    "Some need a look: open the Board or the log for details.",
                    true,
                ),
                (n, 0) => tray::notify(
                    &format!("Converted {n} files to {fmt}"),
                    &format!("in {ms} ms, saved next to the originals"),
                    false,
                ),
                (0, _) => tray::notify("Couldn't convert", &err.unwrap_or_default(), true),
                (n, f) => tray::notify(
                    &format!("Converted {n}, {f} failed"),
                    &err.unwrap_or_default(),
                    true,
                ),
            }
        }
        Work::Tool { tool, files } if wants_picker(tool, &files) => {
            if let Err(e) = crate::apps::launch_pages(tool, &files[0]) {
                tray::notify("Couldn't open the page picker", &e.to_string(), true);
            }
        }
        Work::Tool { tool, files } => {
            let (label, _) = tool.label();
            let none = swirvo_jobs::args::ToolArgs::new();
            match swirvo_jobs::worker::run_tool_with(tk, tool, &files, &none, &CancelToken::new()) {
                Ok(r) => {
                    log(format!("✓ {label}: {}", r.message));
                    for (f, e) in &r.failures {
                        log(format!("✗ {}: {e}", f.display()));
                    }
                    if let Some(text) = r.clipboard {
                        post_clipboard(hwnd, text);
                    }
                    let warn = !r.failures.is_empty();
                    let body = if warn {
                        format!("{} ({} failed)", r.message, r.failures.len())
                    } else {
                        r.message
                    };
                    tray::notify(label, &body, warn);
                }
                Err(e) => {
                    log(format!("✗ {label}: {e}"));
                    tray::notify(&format!("{label} didn't work"), &e.to_string(), true);
                }
            }
        }
        Work::ShowInFolder { file } => show_in_folder(&file),
        Work::OpenBoard { files, tool } => {
            if let Err(e) = crate::apps::launch_board(&files, tool.as_deref()) {
                tray::notify("Couldn't open the Board", &e.to_string(), true);
            }
        }
        Work::OpenTerminal { at, shell } => {
            let dir = if at.is_dir() {
                at
            } else {
                at.parent().map_or(at.clone(), Path::to_path_buf)
            };
            if let Err(e) = crate::shells::open(shell, &dir) {
                tray::notify("Couldn't open a terminal", &e.to_string(), true);
            }
        }
        Work::CopyPath { .. } | Work::Nothing => {}
    }
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
}

fn set_effect(p: *mut DROPEFFECT, e: DROPEFFECT) {
    if !p.is_null() {
        // SAFETY: OLE passes a valid in/out pointer.
        unsafe { *p = e };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        obj: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        // COM call first, with no agent borrow held (it can pump messages).
        let files = obj.as_ref().map(dropped_files).unwrap_or_default();
        if let (Some(h), Some(o)) = (helper(), obj.as_ref()) {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ =
                unsafe { h.DragEnter(self.hwnd, o, &POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY) };
        }
        with_agent(|a| {
            let now = a.now();
            if matches!(a.machine.state(), State::Probing { .. }) {
                a.files = files.clone();
            }
            a.step(Event::DragEnter {
                files: files.len(),
                now,
            });
            if let Some(p) = a.placement {
                let z = p.zone((pt.x, pt.y), a.machine.state(), a.machine.layout().len());
                a.step(Event::Pointer { zone: z, now });
            }
        });
        set_effect(
            effect,
            if files.is_empty() {
                DROPEFFECT_NONE
            } else {
                DROPEFFECT_COPY
            },
        );
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        if let Some(h) = helper() {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ = unsafe { h.DragOver(&POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY) };
        }
        let open = with_agent(|a| {
            let Some(p) = a.placement else { return false };
            let z = p.zone((pt.x, pt.y), a.machine.state(), a.machine.layout().len());
            let now = a.now();
            a.step(Event::Pointer { zone: z, now });
            matches!(a.machine.state(), State::Open(_))
        });
        // COPY, never MOVE: the source must never delete the original.
        set_effect(
            effect,
            if open == Some(true) {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            },
        );
        Ok(())
    }

    fn DragLeave(&self) -> windows_core::Result<()> {
        if let Some(h) = helper() {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ = unsafe { h.DragLeave() };
        }
        with_agent(|a| {
            let now = a.now();
            a.step(Event::DragLeave { now })
        });
        Ok(())
    }

    fn Drop(
        &self,
        obj: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        if let (Some(h), Some(o)) = (helper(), obj.as_ref()) {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ = unsafe { h.Drop(o, &POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY) };
        }
        let answer = with_agent(|a| {
            let now = a.now();
            let zone = a
                .placement
                .map(|p| p.zone((pt.x, pt.y), a.machine.state(), a.machine.layout().len()));
            match zone {
                Some(zone) => a.step(Event::Drop { zone, now }),
                None => a.step(Event::Drop {
                    zone: swirvo_wheel::geom::Zone::Outside { dist: f32::MAX },
                    now,
                }),
            }
        })
        .flatten();
        set_effect(
            effect,
            if answer == Some(DropEffect::Copy) {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            },
        );
        Ok(())
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            if let Some((vkey, up)) = read_key(HRAWINPUT(lp.0 as *mut c_void)) {
                with_agent(|a| a.on_key(vkey, up));
            }
        }
        WM_TIMER if wp.0 == TICK_TIMER => {
            with_agent(Agent::on_tick);
            return LRESULT(0);
        }
        WM_APP_CLIPBOARD => {
            // SAFETY: posted by post_clipboard with a leaked Box<String>; reclaimed exactly once.
            let text = unsafe { Box::from_raw(lp.0 as *mut String) };
            if let Err(e) = set_clipboard(hwnd, &text) {
                log(format!("couldn't copy: {e}"));
            }
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling (WM_INPUT must reach DefWindowProc for cleanup).
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn read_key(h: HRAWINPUT) -> Option<(u16, bool)> {
    let mut raw = RAWINPUT::default();
    let mut size = size_of::<RAWINPUT>() as u32;
    // SAFETY: buffer is a RAWINPUT; keyboard packets always fit.
    let n = unsafe {
        GetRawInputData(
            h,
            RID_INPUT,
            Some((&mut raw as *mut RAWINPUT).cast()),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    if n == u32::MAX || n == 0 || raw.header.dwType != RIM_TYPEKEYBOARD.0 {
        return None;
    }
    // SAFETY: dwType says the union holds a keyboard record.
    let kb = unsafe { raw.data.keyboard };
    Some((
        kb.VKey,
        kb.Flags as u32 & windows::Win32::UI::WindowsAndMessaging::RI_KEY_BREAK != 0,
    ))
}

/// Page tools on one multi-page PDF open the page picker first (docs/12 §4.1). Runs on
/// the job thread: counting pages opens the file.
fn wants_picker(tool: tools::Tool, files: &[PathBuf]) -> bool {
    if tool.page_op().is_none() || files.len() != 1 {
        return false;
    }
    tool == tools::Tool::PdfPages || tools::pdf_page_count(&files[0]).is_some_and(|n| n > 1)
}

/// Open Explorer with `file` selected.
pub(crate) fn show_in_folder(file: &std::path::Path) {
    // SAFETY: COM on the calling (worker) thread; the PIDL is freed after use.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let wide = wide(file.as_os_str());
        let pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
        if !pidl.is_null() {
            if let Err(e) = SHOpenFolderAndSelectItems(pidl, None, 0) {
                log(format!("couldn't open the folder: {e}"));
            }
            ILFree(Some(pidl));
        }
    }
}

/// Settings were re-read (another window saved them): follow the theme.
pub(crate) fn settings_changed() {
    let theme = system_theme();
    with_agent(|a| a.theme = theme);
}

/// Settings → Privacy → Clear history: forget the in-memory copy too, so the next save
/// doesn't bring it back.
pub(crate) fn forget_history() {
    with_agent(|a| a.frecency = Frecency::default());
}

pub(crate) fn system_dark() -> bool {
    matches!(system_theme(), Theme::Dark)
}

fn system_theme() -> Theme {
    if let Some(t) = std::env::var_os("SWIRVO_THEME") {
        return if t == "dark" {
            Theme::Dark
        } else {
            Theme::Light
        };
    }
    // Settings → General → Theme; "System" falls through to Windows' choice.
    match swirvo_jobs::settings::current().theme {
        swirvo_jobs::settings::ThemeChoice::Light => return Theme::Light,
        swirvo_jobs::settings::ThemeChoice::Dark => return Theme::Dark,
        swirvo_jobs::settings::ThemeChoice::System => {}
    }
    let mut v: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: reads one DWORD into a local.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut v as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if r.is_ok() && v == 0 {
        Theme::Dark
    } else {
        Theme::Light
    }
}

/// Start the agent and pump messages until Quit. Errors are shown in a message box too,
/// since a double-clicked app has no console.
pub fn run() -> Result<()> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // A worker serves one request on its pipes and exits: no console (it mustn't attach
    // to ours), no window, no message box (docs/03 §9; crates/jobs/src/worker.rs).
    if args
        .first()
        .is_some_and(|a| a == swirvo_jobs::worker::WORKER_ARG)
    {
        std::process::exit(swirvo_jobs::worker::serve());
    }
    // Started from a terminal: print logs there. Double-clicked: no console, no flash.
    // SAFETY: plain process-level call; failure just means there is no parent console.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
        // One identity for every Swirvo process, matching the Start-menu shortcut.
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(
            &windows_core::HSTRING::from(crate::install::APP_ID),
        );
    }
    // Everything this process starts (wheel conversions, the Board's and the page picker's
    // runs) goes to workers: this same executable with `--worker`. SWIRVO_IN_PROCESS=1
    // keeps the work in-process, for debugging.
    if std::env::var_os("SWIRVO_IN_PROCESS").is_none() {
        swirvo_jobs::worker::set_worker(std::env::current_exe().ok());
    }
    let r = match args.first().and_then(|a| a.to_str()) {
        Some("--board") => crate::apps::board(&args[1..]),
        Some("--settings") => crate::apps::settings(),
        Some("--install") => crate::install::install(),
        Some("--uninstall") => crate::install::uninstall(),
        Some("--pages") => crate::apps::pages(args.get(1), args.get(2), args.get(3)),
        _ => run_inner(),
    };
    if let Err(e) = &r {
        let text: Vec<u16> = format!("Swirvo couldn't start:\n\n{e:#}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: modal error box with a NUL-terminated string.
        unsafe {
            MessageBoxW(
                None,
                PCWSTR(text.as_ptr()),
                w!("Swirvo"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    r
}

fn run_inner() -> Result<()> {
    // SAFETY: process/thread setup calls, each checked; handles live for the process.
    unsafe {
        // One agent per user session; a second launch just says where the first one is.
        let _instance =
            CreateMutexW(None, false, w!("Local\\Swirvo.Agent")).context("CreateMutexW")?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            MessageBoxW(
                None,
                w!(
                    "Swirvo is already running.\n\nLook for the Swirvo icon in the notification area (bottom right, maybe under the ^ arrow). Double-click it to open the Board; right-click it for help or to quit."
                ),
                w!("Swirvo"),
                MB_OK | MB_ICONINFORMATION,
            );
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(None).context("OleInitialize")?;

        let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let class = w!("SwirvoOverlay");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            anyhow::bail!(
                "RegisterClassExW failed: {}",
                windows_core::Error::from_thread()
            );
        }
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Swirvo"),
            WS_POPUP,
            0,
            0,
            PROBE_PX,
            PROBE_PX,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("CreateWindowExW")?;

        let kb = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x06,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        RegisterRawInputDevices(&[kb], size_of::<RAWINPUTDEVICE>() as u32)
            .context("RegisterRawInputDevices")?;

        let target: IDropTarget = DropTarget { hwnd }.into();
        RegisterDragDrop(hwnd, &target).context("RegisterDragDrop")?;
        if let Ok(h) = CoCreateInstance::<_, IDropTargetHelper>(
            &CLSID_DragDropHelper,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            HELPER.with(|c| *c.borrow_mut() = Some(h));
        }

        let started = Instant::now();
        let tk = Arc::new(Toolkit::new().context("building the conversion graph")?);
        let font = load_ui_font();
        if font.is_none() {
            log("no UI font found (set SWIRVO_FONT); wedges will have no labels");
        }
        let shells = crate::shells::labels();
        log(format!("terminals found: {}", shells.join(", ")));
        let agent = Agent {
            hwnd,
            machine: Machine::new(),
            tk,
            frecency: load_frecency(),
            renderer: Renderer::new(font),
            frame: None,
            scene: None,
            theme: system_theme(),
            epoch: Instant::now(),
            down: [false; 3],
            files: Vec::new(),
            menu: None,
            placement: None,
            hub_cancel: false,
            faded: false,
            surface: None,
            timer: false,
            button_up_since: None,
            trigger_at: None,
            shells,
        };
        AGENT.with(|a| *a.borrow_mut() = Some(agent));
        if let Err(e) = tray::create() {
            log(format!("no tray icon: {e:#}"));
        }
        if swirvo_jobs::settings::current().startup_tip {
            tray::notify(
                "Swirvo is running",
                "Drag a file, tap Shift, drop it on a format. Double-click the tray icon for the Board, right-click for help.",
                false,
            );
        }
        // First run of a copied exe: offer to install it (Start menu, Apps list).
        crate::install::offer_once();
        crate::install::sync_startup_later();
        log(format!(
            "ready in {} ms. Drag a file, tap Shift (Convert), Ctrl (Tools) or Alt (Actions), drop on a wedge. Ctrl+C quits.",
            started.elapsed().as_millis()
        ));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}
