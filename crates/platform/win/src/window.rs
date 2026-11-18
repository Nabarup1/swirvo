//! Win32 host for `swirvo-ui` apps: the Board and the page picker (docs/12 §6).
//!
//! - **Glass.** Acrylic system backdrop (Windows 11 22H2+), frame extended into the whole
//!   client area, premultiplied-alpha pixels blitted from a 32-bpp DIB (BitBlt keeps alpha).
//! - **Custom title bar.** `WM_NCCALCSIZE` drops the caption; `WM_NCHITTEST` answers with
//!   the zones the UI recorded (caption, min, max, close), so dragging, double-click to
//!   maximize and Snap Layouts on the maximize button stay native.
//! - **Event-driven.** Frames are painted on input, on a waker from background threads
//!   (`WM_APP_WAKE`), or when an animation asks; idle CPU is zero.
//! - Requests that open dialogs run after the frame (`WM_APP_REQUESTS`), never inside it.
//!
//! Re-entrancy: like the agent, state lives in a thread-local `RefCell` that is only
//! `try_borrow_mut`ed, and no borrow is held across a modal dialog or COM call.

use std::cell::RefCell;
use std::ffi::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use swirvo_ui::{App, Cursor, HostEvent, Input, Key, Mods, Point, Request, Theme, Ui, Zone};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, POINTL, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_TRANSIENTWINDOW, DWMWA_CAPTION_BUTTON_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmDefWindowProc,
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, EndPaint, GetMonitorInfoW, InvalidateRect, MONITOR_DEFAULTTONEAREST,
    MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SRCCOPY, ScreenToClient,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, IDataObject,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize,
    RegisterDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
    SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT,
    TrackMouseEvent, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT,
    VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, FOS_ALLOWMULTISELECT, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS,
    FileOpenDialog, IDropTargetHelper, IFileOpenDialog, IShellItem, SIGDN_FILESYSPATH,
    ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowW,
    GetClientRect, GetCursorPos, GetMessageW, GetWindowRect, HTCAPTION, HTCLIENT, HTCLOSE,
    HTMAXBUTTON, HTMINBUTTON, HTTOP, HTTOPLEFT, HTTOPRIGHT, IDC_ARROW, IDC_HAND, IDC_IBEAM,
    IsZoomed, KillTimer, LoadCursorW, MINMAXINFO, MSG, NCCALCSIZE_PARAMS, PostMessageW,
    PostQuitMessage, RegisterClassExW, SM_CXPADDEDBORDER, SM_CYFRAME, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOW, SW_SHOWNORMAL, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageW, SetCursor, SetForegroundWindow, SetTimer, SetWindowPos, SetWindowTextW,
    ShowWindow, TranslateMessage, WM_ACTIVATE, WM_APP, WM_CHAR, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
    WM_DPICHANGED, WM_ERASEBKGND, WM_GETMINMAXINFO, WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCCALCSIZE, WM_NCHITTEST, WM_NCLBUTTONDOWN,
    WM_NCLBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_PAINT, WM_SETCURSOR, WM_SETTINGCHANGE,
    WM_SIZE, WM_SYSKEYDOWN, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};
use windows_core::{PCWSTR, Ref, implement, w};

use crate::util::{Paste, Surface, dropped_files, paste, set_clipboard, wide};

const WM_APP_WAKE: u32 = WM_APP + 10;
const WM_APP_REQUESTS: u32 = WM_APP + 11;
const FRAME_TIMER: usize = 7;
/// `WM_COPYDATA` tag for "add these files" from a second Board launch.
pub(crate) const COPYDATA_FILES: usize = 0xA9C1;

struct Host {
    hwnd: HWND,
    app: Box<dyn App>,
    ui: Ui,
    surface: Option<Surface>,
    epoch: Instant,
    last: Instant,
    tracking: bool,
    pending: Vec<Request>,
    nc_down: Option<Zone>,
    high_surrogate: Option<u16>,
}

thread_local! {
    static HOST: RefCell<Option<Host>> = const { RefCell::new(None) };
    static HELPER: RefCell<Option<IDropTargetHelper>> = const { RefCell::new(None) };
}

static WAKE_PENDING: AtomicBool = AtomicBool::new(false);

fn with_host<R>(f: impl FnOnce(&mut Host) -> R) -> Option<R> {
    HOST.with(|h| h.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn invalidate(hwnd: HWND) {
    // SAFETY: plain repaint request for our own window.
    unsafe {
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

fn scale_of(hwnd: HWND) -> f32 {
    // SAFETY: plain query on our window.
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

fn mods() -> Mods {
    // SAFETY: plain key-state queries.
    unsafe {
        Mods {
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
        }
    }
}

fn map_key(vk: u16) -> Option<Key> {
    Some(match vk {
        v if v == VK_RETURN.0 => Key::Enter,
        v if v == VK_ESCAPE.0 => Key::Escape,
        v if v == VK_TAB.0 => Key::Tab,
        v if v == VK_BACK.0 => Key::Backspace,
        v if v == VK_DELETE.0 => Key::Delete,
        v if v == VK_LEFT.0 => Key::Left,
        v if v == VK_RIGHT.0 => Key::Right,
        v if v == VK_UP.0 => Key::Up,
        v if v == VK_DOWN.0 => Key::Down,
        v if v == VK_HOME.0 => Key::Home,
        v if v == VK_END.0 => Key::End,
        v if v == VK_PRIOR.0 => Key::PageUp,
        v if v == VK_NEXT.0 => Key::PageDown,
        v if v == VK_SPACE.0 => Key::Space,
        0x41 => Key::A,
        0x43 => Key::C,
        0x44 => Key::D,
        0x46 => Key::F,
        0x56 => Key::V,
        0x58 => Key::X,
        0x5A => Key::Z,
        _ => return None,
    })
}

fn lparam_point(lp: LPARAM) -> (i32, i32) {
    let x = (lp.0 & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

fn apply_theme(hwnd: HWND, dark: bool) -> bool {
    // SAFETY: DWM attribute calls with properly sized values; failures just mean an older
    // Windows without the feature.
    unsafe {
        let d: i32 = dark.into();
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            (&d as *const i32).cast(),
            4,
        );
        let corner = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &corner as *const _ as *const c_void,
            4,
        );
        let backdrop = DWMSBT_TRANSIENTWINDOW;
        let ok = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const c_void,
            4,
        )
        .is_ok();
        let m = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &m);
        ok && std::env::var_os("SWIRVO_NO_GLASS").is_none()
    }
}

impl Host {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn paint(&mut self) {
        let hwnd = self.hwnd;
        let mut rc = RECT::default();
        // SAFETY: plain query on our window.
        if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
            return;
        }
        let (w, h) = ((rc.right - rc.left).max(1), (rc.bottom - rc.top).max(1));
        let scale = scale_of(hwnd);
        let now = Instant::now();
        self.ui.input.dt = now.duration_since(self.last).as_secs_f32();
        self.last = now;
        self.ui.input.time = self.now();
        // SAFETY: plain query.
        self.ui.input.maximized = unsafe { IsZoomed(hwnd) }.as_bool();
        self.ui.caption_buttons = caption_buttons(hwnd, scale);
        self.ui.begin_frame(w as u32, h as u32, scale);
        self.app.frame(&mut self.ui);
        let next = self.ui.end_frame();
        self.pending.extend(self.app.take_requests());
        if !self.pending.is_empty() {
            // SAFETY: posting to our own window.
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_APP_REQUESTS, WPARAM(0), LPARAM(0));
            }
        }
        if self.surface.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surface = Surface::new(w, h);
        }
        if let Some(s) = self.surface.as_mut() {
            let src = self.ui.pixmap().data();
            let dst = s.pixels();
            // tiny-skia is premultiplied RGBA; DIBs are premultiplied BGRA.
            for (d, sp) in dst
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(src.as_chunks::<4>().0)
            {
                d[0] = sp[2];
                d[1] = sp[1];
                d[2] = sp[0];
                d[3] = sp[3];
            }
            let mut ps = PAINTSTRUCT::default();
            // SAFETY: standard paint cycle; the DIB stays selected in its DC while blitted.
            unsafe {
                let hdc = BeginPaint(hwnd, &mut ps);
                let _ = BitBlt(hdc, 0, 0, w, h, Some(s.dc), 0, 0, SRCCOPY);
                let _ = EndPaint(hwnd, &ps);
            }
        }
        // SAFETY: timer on our own window; replaced each frame.
        unsafe {
            let _ = KillTimer(Some(hwnd), FRAME_TIMER);
            if let Some(secs) = next {
                let ms = (secs * 1000.0).clamp(1.0, 60_000.0) as u32;
                SetTimer(Some(hwnd), FRAME_TIMER, ms, None);
            }
        }
    }

    fn logical(&self, x: i32, y: i32) -> Point {
        let s = scale_of(self.hwnd);
        Point::new(x as f32 / s, y as f32 / s)
    }
}

/// The native caption buttons' rectangle in client logical px, if DWM draws them.
fn caption_buttons(hwnd: HWND, scale: f32) -> Option<swirvo_ui::Rect> {
    // SAFETY: plain queries on our own window into locals of the documented types.
    unsafe {
        let mut b = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_BUTTON_BOUNDS,
            (&mut b as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .ok()?;
        if b.right <= b.left {
            return None;
        }
        // Window-relative → client-relative.
        let mut wr = RECT::default();
        GetWindowRect(hwnd, &mut wr).ok()?;
        let mut origin = POINT::default();
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut origin);
        let (dx, dy) = (origin.x - wr.left, origin.y - wr.top);
        Some(swirvo_ui::Rect::new(
            (b.left - dx) as f32 / scale,
            (b.top - dy) as f32 / scale,
            (b.right - b.left) as f32 / scale,
            (b.bottom - b.top) as f32 / scale,
        ))
    }
}

fn pick(hwnd: HWND, folder: bool) -> Vec<PathBuf> {
    // SAFETY: standard IFileOpenDialog use on this STA thread; strings freed with
    // CoTaskMemFree. The dialog is modal and pumps messages (no host borrow is held).
    unsafe {
        let Ok(dlg) =
            CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
        else {
            return Vec::new();
        };
        let mut opts = dlg.GetOptions().unwrap_or_default() | FOS_FORCEFILESYSTEM;
        opts |= if folder {
            FOS_PICKFOLDERS
        } else {
            FOS_ALLOWMULTISELECT
        };
        let _ = dlg.SetOptions(opts);
        if dlg.Show(Some(hwnd)).is_err() {
            return Vec::new();
        }
        let path_of = |item: &IShellItem| -> Option<PathBuf> {
            let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
            let s = std::ffi::OsString::from_wide(p.as_wide());
            CoTaskMemFree(Some(p.0 as *const c_void));
            Some(PathBuf::from(s))
        };
        let mut out = Vec::new();
        if folder {
            if let Ok(item) = dlg.GetResult()
                && let Some(p) = path_of(&item)
            {
                out.push(p);
            }
        } else if let Ok(items) = dlg.GetResults() {
            let n = items.GetCount().unwrap_or(0);
            for i in 0..n {
                if let Ok(item) = items.GetItemAt(i)
                    && let Some(p) = path_of(&item)
                {
                    out.push(p);
                }
            }
        }
        out
    }
}

fn run_requests(hwnd: HWND) {
    let reqs = with_host(|h| std::mem::take(&mut h.pending)).unwrap_or_default();
    for r in reqs {
        match r {
            Request::PickFiles => {
                let files = pick(hwnd, false);
                if !files.is_empty() {
                    with_host(|h| h.app.on_event(HostEvent::FilesPicked(files)));
                }
            }
            Request::PickFolder => {
                if let Some(d) = pick(hwnd, true).pop() {
                    with_host(|h| h.app.on_event(HostEvent::FolderPicked(d)));
                }
            }
            Request::ShowInFolder(p) => {
                std::thread::spawn(move || crate::agent::show_in_folder(&p));
            }
            Request::Open(p) => {
                let wp = wide(p.as_os_str());
                // SAFETY: ShellExecute with NUL-terminated strings; no command line is built.
                unsafe {
                    ShellExecuteW(
                        Some(hwnd),
                        w!("open"),
                        PCWSTR(wp.as_ptr()),
                        PCWSTR::null(),
                        PCWSTR::null(),
                        SW_SHOWNORMAL,
                    );
                }
            }
            Request::OpenPages { tool, file, spec } => {
                if let Some(tool) = swirvo_jobs::tools::Tool::from_id(&tool) {
                    let _ = crate::apps::launch_pages_with(tool, &file, &spec);
                }
            }
            Request::CopyText(t) => {
                let _ = set_clipboard(hwnd, &t);
            }
            Request::OpenSettings => {
                let _ = crate::apps::launch_settings();
            }
            Request::SettingsChanged => {
                // Saved already: tell the agent and the other windows, and follow "Start with
                // Windows" (registry or the package's startup task) off the UI thread.
                crate::install::broadcast(crate::install::settings_message());
                std::thread::spawn(|| {
                    let on = swirvo_jobs::settings::current().start_with_windows;
                    let _ = crate::install::set_startup(on);
                });
            }
            Request::HistoryCleared => {
                crate::install::broadcast(crate::install::history_message());
            }
            Request::Install => {
                let _ = crate::apps::launch_install(false);
            }
            Request::Uninstall => {
                let _ = crate::apps::launch_install(true);
            }
            Request::SetTitle(t) => {
                let wt: Vec<u16> = t.encode_utf16().chain(std::iter::once(0)).collect();
                // SAFETY: NUL-terminated title.
                unsafe {
                    let _ = SetWindowTextW(hwnd, PCWSTR(wt.as_ptr()));
                }
            }
            Request::Close => {
                // SAFETY: closing our own window.
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                return;
            }
        }
    }
    invalidate(hwnd);
}

fn zone_ht(z: Option<Zone>) -> u32 {
    match z {
        Some(Zone::Caption) => HTCAPTION,
        Some(Zone::Minimize) => HTMINBUTTON,
        Some(Zone::Maximize) => HTMAXBUTTON,
        Some(Zone::Close) => HTCLOSE,
        None => HTCLIENT,
    }
}

fn ht_zone(ht: u32) -> Option<Zone> {
    match ht {
        HTMINBUTTON => Some(Zone::Minimize),
        HTMAXBUTTON => Some(Zone::Maximize),
        HTCLOSE => Some(Zone::Close),
        _ => None,
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // DWM draws native caption buttons in the extended frame; let it hit-test and animate
    // them first (Windows 11 Snap Layouts included).
    let mut lr = LRESULT(0);
    // SAFETY: forwards this message to DWM with a valid out-pointer.
    if unsafe { DwmDefWindowProc(hwnd, msg, wp, lp, &mut lr) }.as_bool() {
        return lr;
    }
    // SAFETY (for the unsafe blocks below): standard window-procedure plumbing on our own
    // window; pointers in WPARAM/LPARAM are the ones Windows documents for each message.
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => unsafe {
            // Keep the default side and bottom borders (resizing), drop the caption.
            let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
            let top = params.rgrc[0].top;
            let _ = DefWindowProcW(hwnd, msg, wp, lp);
            params.rgrc[0].top = top;
            if IsZoomed(hwnd).as_bool() {
                // Maximized windows overhang the monitor by the frame; pull the top in.
                let dpi = GetDpiForWindow(hwnd);
                params.rgrc[0].top += GetSystemMetricsForDpi(SM_CYFRAME, dpi)
                    + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
            }
            return LRESULT(0);
        },
        WM_NCHITTEST => unsafe {
            let def = DefWindowProcW(hwnd, msg, wp, lp);
            if def.0 as u32 != HTCLIENT {
                return def;
            }
            let (sx, sy) = lparam_point(lp);
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let s = scale_of(hwnd);
            let border = (6.0 * s) as i32;
            if !IsZoomed(hwnd).as_bool() && pt.y < border {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                return LRESULT(if pt.x < border * 2 {
                    HTTOPLEFT
                } else if pt.x > rc.right - border * 2 {
                    HTTOPRIGHT
                } else {
                    HTTOP
                } as isize);
            }
            let p = Point::new(pt.x as f32 / s, pt.y as f32 / s);
            let z = with_host(|h| h.ui.zone_at(p)).flatten();
            return LRESULT(zone_ht(z) as isize);
        },
        WM_NCMOUSEMOVE => {
            let z = ht_zone(wp.0 as u32);
            with_host(|h| {
                h.ui.input.pointer = None;
                if h.ui.input.zone_hover != z {
                    h.ui.input.zone_hover = z;
                    invalidate(hwnd);
                }
                let mut t = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE | TME_NONCLIENT,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                // SAFETY: valid TRACKMOUSEEVENT for our window.
                unsafe {
                    let _ = TrackMouseEvent(&mut t);
                }
                h.tracking = false;
            });
        }
        WM_NCMOUSELEAVE => {
            with_host(|h| {
                h.ui.input.zone_hover = None;
                h.ui.input.zone_pressed = None;
                h.nc_down = None;
            });
            invalidate(hwnd);
        }
        WM_NCLBUTTONDOWN => {
            if let Some(z) = ht_zone(wp.0 as u32) {
                // Swallow it, or DefWindowProc draws classic caption buttons over ours.
                with_host(|h| {
                    h.nc_down = Some(z);
                    h.ui.input.zone_pressed = Some(z);
                });
                invalidate(hwnd);
                return LRESULT(0);
            }
        }
        WM_NCLBUTTONUP => {
            if let Some(z) = ht_zone(wp.0 as u32) {
                let down = with_host(|h| {
                    h.ui.input.zone_pressed = None;
                    h.nc_down.take()
                })
                .flatten();
                if down == Some(z) {
                    // SAFETY: window commands on our own window.
                    unsafe {
                        match z {
                            Zone::Minimize => {
                                let _ = ShowWindow(hwnd, SW_MINIMIZE);
                            }
                            Zone::Maximize => {
                                let cmd = if IsZoomed(hwnd).as_bool() {
                                    SW_RESTORE
                                } else {
                                    SW_MAXIMIZE
                                };
                                let _ = ShowWindow(hwnd, cmd);
                            }
                            Zone::Close => {
                                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                            Zone::Caption => {}
                        }
                    }
                }
                invalidate(hwnd);
                return LRESULT(0);
            }
        }
        WM_MOUSEMOVE => {
            let (x, y) = lparam_point(lp);
            with_host(|h| {
                h.ui.input.pointer = Some(h.logical(x, y));
                h.ui.input.zone_hover = None;
                h.ui.input.mods = mods();
                if !h.tracking {
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    // SAFETY: valid TRACKMOUSEEVENT for our window.
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                    h.tracking = true;
                }
            });
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            with_host(|h| {
                h.ui.input.pointer = None;
                h.tracking = false;
            });
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let (x, y) = lparam_point(lp);
            // SAFETY: capture so a drag outside the window still ends here.
            unsafe {
                SetCapture(hwnd);
            }
            with_host(|h| {
                h.ui.input.pointer = Some(h.logical(x, y));
                h.ui.input.down = true;
                h.ui.input.pressed = true;
                h.ui.input.mods = mods();
                if msg == WM_LBUTTONDBLCLK {
                    h.ui.input.double_click = true;
                }
            });
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            let (x, y) = lparam_point(lp);
            // SAFETY: releasing our own capture.
            unsafe {
                let _ = ReleaseCapture();
            }
            with_host(|h| {
                h.ui.input.pointer = Some(h.logical(x, y));
                h.ui.input.down = false;
                h.ui.input.released = true;
            });
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
            with_host(|h| h.ui.input.wheel -= delta / 120.0 * 64.0);
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wp.0 as u16;
            let m = mods();
            if let Some(k) = map_key(vk) {
                if k == Key::V && m.ctrl {
                    match paste(hwnd) {
                        Paste::Files(f) => {
                            with_host(|h| h.ui.input.dropped.extend(f));
                        }
                        Paste::Text(t) => {
                            let t: String =
                                t.chars().filter(|c| *c != '\r' && *c != '\n').collect();
                            with_host(|h| h.ui.input.text.push_str(&t));
                        }
                        Paste::Nothing => {}
                    }
                } else {
                    with_host(|h| {
                        h.ui.input.keys.push((k, m));
                        h.ui.input.mods = m;
                    });
                }
                invalidate(hwnd);
            }
            if msg == WM_KEYDOWN {
                return LRESULT(0);
            }
        }
        WM_CHAR => {
            let u = wp.0 as u16;
            with_host(|h| {
                if (0xD800..0xDC00).contains(&u) {
                    h.high_surrogate = Some(u);
                    return;
                }
                let units: Vec<u16> = match h.high_surrogate.take() {
                    Some(hi) if (0xDC00..0xE000).contains(&u) => vec![hi, u],
                    _ => vec![u],
                };
                let s = String::from_utf16_lossy(&units);
                if !mods().ctrl && s.chars().all(|c| !c.is_control()) {
                    h.ui.input.text.push_str(&s);
                }
            });
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let c = with_host(|h| h.ui.cursor).unwrap_or_default();
            // SAFETY: system cursors.
            unsafe {
                let id = match c {
                    Cursor::Arrow => IDC_ARROW,
                    Cursor::Hand => IDC_HAND,
                    Cursor::Text => IDC_IBEAM,
                };
                if let Ok(cur) = LoadCursorW(None, id) {
                    SetCursor(Some(cur));
                }
            }
            return LRESULT(1);
        }
        WM_PAINT => {
            if with_host(Host::paint).is_none() {
                // Re-entrant paint (e.g. under a modal dialog): validate and retry later.
                let mut ps = PAINTSTRUCT::default();
                // SAFETY: empty paint cycle.
                unsafe {
                    BeginPaint(hwnd, &mut ps);
                    let _ = EndPaint(hwnd, &ps);
                    let _ = PostMessageW(Some(hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0));
                }
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_TIMER if wp.0 == FRAME_TIMER => {
            // SAFETY: one-shot: the next paint re-arms it if needed.
            unsafe {
                let _ = KillTimer(Some(hwnd), FRAME_TIMER);
            }
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_APP_WAKE => {
            WAKE_PENDING.store(false, Ordering::Release);
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_APP_REQUESTS => {
            run_requests(hwnd);
            return LRESULT(0);
        }
        WM_SIZE => invalidate(hwnd),
        WM_ACTIVATE => {
            let active = (wp.0 & 0xFFFF) != 0;
            with_host(|h| h.ui.input.active = active);
            invalidate(hwnd);
        }
        WM_GETMINMAXINFO => {
            let s = scale_of(hwnd);
            let min = with_host(|h| h.app.min_size()).unwrap_or((640.0, 420.0));
            // SAFETY: lParam points at the MINMAXINFO for this message.
            unsafe {
                let mm = &mut *(lp.0 as *mut MINMAXINFO);
                mm.ptMinTrackSize.x = (min.0 * s) as i32;
                mm.ptMinTrackSize.y = (min.1 * s) as i32;
            }
            return LRESULT(0);
        }
        WM_DPICHANGED => {
            // SAFETY: lParam is the suggested RECT for the new DPI.
            unsafe {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            invalidate(hwnd);
            return LRESULT(0);
        }
        WM_SETTINGCHANGE => {
            let dark = crate::agent::system_dark();
            with_host(|h| {
                if h.ui.theme.dark != dark {
                    h.ui.theme = Theme::for_dark(dark);
                    h.ui.backdrop = apply_theme(hwnd, dark);
                }
            });
            invalidate(hwnd);
        }
        WM_COPYDATA => {
            // SAFETY: lParam is a COPYDATASTRUCT valid for the duration of the message.
            let files = unsafe {
                let cds = &*(lp.0 as *const COPYDATASTRUCT);
                if cds.dwData != COPYDATA_FILES || cds.lpData.is_null() {
                    return LRESULT(0);
                }
                let units =
                    std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
                units
                    .split(|&u| u == 0)
                    .filter(|s| !s.is_empty())
                    .map(|s| PathBuf::from(std::ffi::OsString::from_wide(s)))
                    .collect::<Vec<_>>()
            };
            let (tools, files): (Vec<PathBuf>, Vec<PathBuf>) = files
                .into_iter()
                .partition(|p| p.to_string_lossy().starts_with(TOOL_PREFIX));
            if !files.is_empty() {
                with_host(|h| h.app.on_event(HostEvent::AddFiles(files)));
            }
            if let Some(t) = tools.first() {
                let id = t.to_string_lossy()[TOOL_PREFIX.len()..].to_string();
                with_host(|h| h.app.on_event(HostEvent::OpenTool(id)));
            }
            // SAFETY: bring our own window forward (the sender allowed it).
            unsafe {
                if IsZoomed(hwnd).as_bool() {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                } else {
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                }
                let _ = SetForegroundWindow(hwnd);
            }
            invalidate(hwnd);
            return LRESULT(1);
        }
        WM_CLOSE => {
            // SAFETY: destroying our own window.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            return LRESULT(0);
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        m if m == crate::install::settings_message() => {
            // Another Swirvo window saved the settings: re-read them off this thread, then
            // re-apply the theme as if Windows' own theme had changed.
            let bits = hwnd.0 as isize;
            std::thread::spawn(move || {
                swirvo_jobs::settings::reload();
                // SAFETY: posting to a window handle is harmless if it has gone.
                unsafe {
                    let _ = PostMessageW(
                        Some(HWND(bits as *mut c_void)),
                        WM_SETTINGCHANGE,
                        WPARAM(0),
                        LPARAM(0),
                    );
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn helper() -> Option<IDropTargetHelper> {
    HELPER.with(|h| h.borrow().clone())
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
}

fn set_effect(p: *mut DROPEFFECT, e: DROPEFFECT) {
    if !p.is_null() {
        // SAFETY: OLE passes a valid out-pointer.
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
        let has = obj.as_ref().is_some_and(|o| !dropped_files(o).is_empty());
        if let (Some(h), Some(o)) = (helper(), obj.as_ref()) {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ =
                unsafe { h.DragEnter(self.hwnd, o, &POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY) };
        }
        with_host(|h| h.ui.input.drag_hover = has);
        invalidate(self.hwnd);
        set_effect(
            effect,
            if has {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
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
        let has = with_host(|h| h.ui.input.drag_hover).unwrap_or(false);
        // COPY, never MOVE: the source must never delete the original.
        set_effect(
            effect,
            if has {
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
        with_host(|h| h.ui.input.drag_hover = false);
        invalidate(self.hwnd);
        Ok(())
    }

    fn Drop(
        &self,
        obj: Ref<'_, IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows_core::Result<()> {
        let files = obj.as_ref().map(dropped_files).unwrap_or_default();
        if let (Some(h), Some(o)) = (helper(), obj.as_ref()) {
            // SAFETY: forwards to the shell's drag-image helper.
            let _ = unsafe { h.Drop(o, &POINT { x: pt.x, y: pt.y }, DROPEFFECT_COPY) };
        }
        let n = files.len();
        with_host(|h| {
            h.ui.input.drag_hover = false;
            h.ui.input.dropped.extend(files);
        });
        invalidate(self.hwnd);
        set_effect(
            effect,
            if n > 0 {
                DROPEFFECT_COPY
            } else {
                DROPEFFECT_NONE
            },
        );
        Ok(())
    }
}

pub(crate) struct WindowSpec {
    pub class: PCWSTR,
    pub size: (f32, f32),
}

/// Hand `files` to an already-open window of `class`. Returns false if there is none.
/// Prefix of the entry carrying a tool request in the Board's `WM_COPYDATA` list. No
/// Windows path starts with `?` (the `\\?\` prefix starts with backslashes).
const TOOL_PREFIX: &str = "?tool=";

pub(crate) fn send_to_existing(class: PCWSTR, files: &[PathBuf], tool: Option<&str>) -> bool {
    // SAFETY: FindWindow + WM_COPYDATA with a buffer that outlives the synchronous send.
    unsafe {
        let Ok(hwnd) = FindWindowW(class, PCWSTR::null()) else {
            return false;
        };
        if hwnd.is_invalid() {
            return false;
        }
        let mut buf: Vec<u16> = Vec::new();
        if let Some(t) = tool {
            buf.extend(format!("{TOOL_PREFIX}{t}").encode_utf16());
            buf.push(0);
        }
        for f in files {
            buf.extend(f.as_os_str().encode_wide());
            buf.push(0);
        }
        buf.push(0);
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_FILES,
            cbData: (buf.len() * 2) as u32,
            lpData: buf.as_mut_ptr().cast(),
        };
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
            windows::Win32::UI::WindowsAndMessaging::ASFW_ANY,
        );
        SendMessageW(
            hwnd,
            WM_COPYDATA,
            Some(WPARAM(0)),
            Some(LPARAM(&cds as *const COPYDATASTRUCT as isize)),
        );
        true
    }
}

/// Open a window for the app built by `make` and pump messages until it closes.
pub(crate) fn run(
    spec: WindowSpec,
    make: impl FnOnce(swirvo_ui::Waker) -> Box<dyn App>,
) -> Result<()> {
    // SAFETY: process/thread setup; each call checked; handles live for the process.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        OleInitialize(None).context("OleInitialize")?;
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let icon = crate::tray::make_icon(32);
        let small = crate::tray::make_icon(16);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: spec.class,
            hIcon: icon.unwrap_or_default(),
            hIconSm: small.unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            anyhow::bail!(
                "RegisterClassExW failed: {}",
                windows_core::Error::from_thread()
            );
        }
        // Centered on the monitor under the pointer, sized for its DPI.
        let mut cur = POINT::default();
        let _ = GetCursorPos(&mut cur);
        let mon = MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let work = mi.rcWork;
        let mut dpi_x = 96u32;
        let mut dpi_y = 96u32;
        let _ = windows::Win32::UI::HiDpi::GetDpiForMonitor(
            mon,
            windows::Win32::UI::HiDpi::MDT_EFFECTIVE_DPI,
            &mut dpi_x,
            &mut dpi_y,
        );
        let s = dpi_x.max(96) as f32 / 96.0;
        let (ww, wh) = (
            ((spec.size.0 * s) as i32).min(work.right - work.left - 40),
            ((spec.size.1 * s) as i32).min(work.bottom - work.top - 40),
        );
        let x = work.left + (work.right - work.left - ww) / 2;
        let y = work.top + (work.bottom - work.top - wh) / 2;
        let hwnd = CreateWindowExW(
            Default::default(),
            spec.class,
            w!("Swirvo"),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            ww,
            wh,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("CreateWindowExW")?;
        let dark = crate::agent::system_dark();
        let backdrop = apply_theme(hwnd, dark);
        let fonts = swirvo_ui::Fonts::system().context("no UI font found")?;
        let mut ui = Ui::new(Theme::for_dark(dark), fonts);
        ui.backdrop = backdrop;
        ui.input = Input {
            active: true,
            ..Input::default()
        };
        let hwnd_bits = hwnd.0 as isize;
        let waker: swirvo_ui::Waker = Arc::new(move || {
            if !WAKE_PENDING.swap(true, Ordering::AcqRel) {
                // Posting to a window handle is harmless if it's gone (inside run's unsafe).
                let _ = PostMessageW(
                    Some(HWND(hwnd_bits as *mut c_void)),
                    WM_APP_WAKE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        let app = make(waker);
        let title: Vec<u16> = app
            .title()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let _ = SetWindowTextW(hwnd, PCWSTR(title.as_ptr()));
        HOST.with(|h| {
            *h.borrow_mut() = Some(Host {
                hwnd,
                app,
                ui,
                surface: None,
                epoch: Instant::now(),
                last: Instant::now(),
                tracking: false,
                pending: Vec::new(),
                nc_down: None,
                high_surrogate: None,
            })
        });
        let target: IDropTarget = DropTarget { hwnd }.into();
        let _ = RegisterDragDrop(hwnd, &target);
        if let Ok(h) = CoCreateInstance::<_, IDropTargetHelper>(
            &CLSID_DragDropHelper,
            None,
            CLSCTX_INPROC_SERVER,
        ) {
            HELPER.with(|c| *c.borrow_mut() = Some(h));
        }
        // Recompute the frame now that WM_NCCALCSIZE is ours.
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE
                | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE
                | SWP_NOZORDER,
        );
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        HOST.with(|h| h.borrow_mut().take());
    }
    Ok(())
}
