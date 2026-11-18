//! Notification-area presence: the Swirvo icon, its menu (Board · Settings · Pause the
//! wheel · Install · How to use · Licences · Quit) and toast notifications for finished jobs. Double-click opens the
//! Board; a single click or right-click shows the menu. Lives on its own hidden window
//! so the menu can take the foreground (the overlay window is never activated).

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use anyhow::{Context, Result};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NOTIFY_ICON_INFOTIP_FLAGS, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CS_DBLCLKS, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, GetCursorPos, HICON, ICONINFO, KillTimer, MB_ICONINFORMATION, MB_OK, MF_CHECKED,
    MF_DISABLED, MF_SEPARATOR, MF_STRING, MessageBoxW, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, SetTimer,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_APP, WM_CONTEXTMENU,
    WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSEXW, WS_POPUP,
};
use windows_core::{BOOL, PCWSTR, w};

const WM_TRAY: u32 = WM_APP + 1;
const ID_HELP: usize = 1;
const ID_QUIT: usize = 2;
const ID_BOARD: usize = 3;
const ID_LICENSES: usize = 4;
const ID_SETTINGS: usize = 5;
const ID_PAUSE: usize = 6;
const ID_INSTALL: usize = 7;
/// Posted to the tray window by a settings reload thread when it's done.
const WM_SETTINGS_LOADED: u32 = WM_APP + 2;
/// A left click waits this long for a second click before showing the menu.
const CLICK_TIMER: usize = 1;

pub const HOW_TO: &str = "Swirvo converts files with a drag and a tap.\n\n\
1. Start dragging a file (in Explorer, on the desktop, from a browser…).\n\
2. While still holding the mouse button, tap a key:\n\
     Shift = Convert   (JPG, PNG, WebP, PDF, CSV, JSON, ZIP…)\n\
     Ctrl  = Tools     (Extract here, …)\n\
     Alt   = Actions   (Show in folder, Copy path, Terminal here)\n\
3. Drop the file on a wedge.\n\n\
Rest on a format to pick Best / Balanced / Smallest; rest on Terminal to pick a shell (PowerShell, Cmd, WSL, Git Bash: whichever are installed). Drop on the middle, or press Esc, to cancel.\n\
The new file appears next to the original; the original is never changed.\n\n\
Double-click the tray icon to open the Board (batch conversions).";

/// The tray window, for notifications from worker threads (0 = not created).
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
/// Explorer broadcasts this after it restarts; the icon must be added again.
static TASKBAR_CREATED: AtomicIsize = AtomicIsize::new(0);
static ICON: AtomicIsize = AtomicIsize::new(0);
/// The button-up that ends a double-click must not start another single click.
static SWALLOW_UP: AtomicBool = AtomicBool::new(false);

fn copy_wide<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *d = c;
    }
}

fn base(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn add_icon(hwnd: HWND) {
    let mut nid = base(hwnd);
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = HICON(ICON.load(Ordering::Relaxed) as *mut _);
    copy_wide(&mut nid.szTip, "Swirvo: drag a file, tap Shift");
    // SAFETY: a fully initialized NOTIFYICONDATAW for our own window.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

/// Show a toast/balloon. Safe to call from any thread; a no-op before the tray exists.
pub fn notify(title: &str, body: &str, warning: bool) {
    let h = TRAY_HWND.load(Ordering::Relaxed);
    if h == 0 {
        return;
    }
    let mut nid = base(HWND(h as *mut _));
    nid.uFlags = NIF_INFO;
    copy_wide(&mut nid.szInfoTitle, title);
    copy_wide(&mut nid.szInfo, body);
    nid.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO } | NOTIFY_ICON_INFOTIP_FLAGS(0);
    // SAFETY: Shell_NotifyIcon only reads the struct; the window outlives the process' work.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

pub fn show_help(owner: Option<HWND>) {
    let text: Vec<u16> = HOW_TO.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: modal box; it pumps messages, so the overlay keeps working while it's open.
    unsafe {
        MessageBoxW(
            owner,
            PCWSTR(text.as_ptr()),
            w!("How to use Swirvo"),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn show_menu(hwnd: HWND) {
    // SAFETY: standard tray-menu sequence; the menu is destroyed before returning.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING | MF_DISABLED, 0, w!("Swirvo is running"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_BOARD, w!("Open Board…"));
        // Bold: what double-clicking the icon does.
        let _ = SetMenuDefaultItem(menu, ID_BOARD as u32, 0);
        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Settings…"));
        let paused = swirvo_jobs::settings::current().wheel_paused;
        let _ = AppendMenuW(
            menu,
            if paused {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            },
            ID_PAUSE,
            w!("Pause the wheel"),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        if !crate::install::packaged() && !crate::install::is_installed() {
            let _ = AppendMenuW(menu, MF_STRING, ID_INSTALL, w!("Install Swirvo…"));
        }
        let _ = AppendMenuW(menu, MF_STRING, ID_HELP, w!("How to use…"));
        let _ = AppendMenuW(menu, MF_STRING, ID_LICENSES, w!("Open-source licences…"));
        let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, w!("Quit Swirvo"));
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Without this the menu doesn't close when you click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        match cmd.0 as usize {
            ID_HELP => show_help(None),
            ID_BOARD => open_board(),
            ID_LICENSES => open_notices(hwnd),
            ID_SETTINGS => {
                if let Err(e) = crate::apps::launch_settings() {
                    notify("Couldn't open Settings", &e.to_string(), true);
                }
            }
            ID_PAUSE => toggle_pause(),
            ID_INSTALL => {
                let _ = crate::apps::launch_install(false);
            }
            ID_QUIT => quit(),
            _ => {}
        }
    }
}

/// Flip "Pause the wheel", save it, and tell the other Swirvo windows.
fn toggle_pause() {
    std::thread::spawn(|| {
        let mut s = (*swirvo_jobs::settings::current()).clone();
        s.wheel_paused = !s.wheel_paused;
        if swirvo_jobs::settings::save(&s).is_ok() {
            crate::install::broadcast(crate::install::settings_message());
            if s.wheel_paused {
                notify(
                    "The wheel is paused",
                    "Shift, Ctrl and Alt work as usual while dragging. Turn it back on from this menu.",
                    false,
                );
            }
        }
    });
}

/// Start the Board (an open one just comes forward: it is single-instance).
fn open_board() {
    if let Err(e) = crate::apps::launch_board(&[], None) {
        notify("Couldn't open the Board", &e.to_string(), true);
    }
}

/// The third-party notices shipped next to the exe (scripts/third-party-notices.py).
fn open_notices(hwnd: HWND) {
    let path = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("THIRD-PARTY-NOTICES.txt")))
        .filter(|p| p.is_file());
    let Some(path) = path else {
        notify(
            "Licences file missing",
            "THIRD-PARTY-NOTICES.txt should be next to Swirvo.exe.",
            true,
        );
        return;
    };
    let wp = crate::util::wide(path.as_os_str());
    // SAFETY: ShellExecute with a NUL-terminated path; no command line is built.
    unsafe {
        windows::Win32::UI::Shell::ShellExecuteW(
            Some(hwnd),
            w!("open"),
            PCWSTR(wp.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

/// Remove the icon and end the message loop.
pub fn quit() {
    let h = TRAY_HWND.swap(0, Ordering::Relaxed);
    if h != 0 {
        // SAFETY: deleting our own icon.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &base(HWND(h as *mut _)));
        }
    }
    // SAFETY: ends GetMessage on this (the UI) thread.
    unsafe { PostQuitMessage(0) };
}

extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        // Legacy callback (no NIM_SETVERSION): the low word is the mouse message.
        // SAFETY (all arms): timers on our own window, cancelled before acting.
        match (lp.0 & 0xFFFF) as u32 {
            WM_LBUTTONUP => {
                if !SWALLOW_UP.swap(false, Ordering::Relaxed) {
                    unsafe { SetTimer(Some(hwnd), CLICK_TIMER, GetDoubleClickTime(), None) };
                }
            }
            WM_LBUTTONDBLCLK => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), CLICK_TIMER);
                }
                SWALLOW_UP.store(true, Ordering::Relaxed);
                open_board();
            }
            WM_RBUTTONUP | WM_CONTEXTMENU => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), CLICK_TIMER);
                }
                show_menu(hwnd);
            }
            _ => {}
        }
        return LRESULT(0);
    }
    if msg == WM_TIMER && wp.0 == CLICK_TIMER {
        // SAFETY: one-shot: stop the timer, then show the menu for the single click.
        unsafe {
            let _ = KillTimer(Some(hwnd), CLICK_TIMER);
        }
        show_menu(hwnd);
        return LRESULT(0);
    }
    if msg == crate::install::quit_message() {
        // Install or uninstall is replacing this copy.
        quit();
        return LRESULT(0);
    }
    if msg == crate::install::settings_message() {
        // Re-read off this thread (it's the agent's UI thread), then apply here.
        let bits = hwnd.0 as isize;
        std::thread::spawn(move || {
            swirvo_jobs::settings::reload();
            // SAFETY: posting to our tray window; harmless if it has gone.
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(bits as *mut _)),
                    WM_SETTINGS_LOADED,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        });
        return LRESULT(0);
    }
    if msg == WM_SETTINGS_LOADED {
        crate::agent::settings_changed();
        return LRESULT(0);
    }
    if msg == crate::install::history_message() {
        crate::agent::forget_history();
        return LRESULT(0);
    }
    let restarted = TASKBAR_CREATED.load(Ordering::Relaxed);
    if restarted != 0 && msg == restarted as u32 {
        add_icon(hwnd);
        return LRESULT(0);
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

/// An HICON from the rendered app icon (32-bit color with alpha, plus the unused mask).
pub(crate) fn make_icon(px: u32) -> Option<HICON> {
    let pm = swirvo_render::app_icon(px)?;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: px as i32,
            biHeight: -(px as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: GDI objects created here are deleted after CreateIconIndirect copies them.
    unsafe {
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        if bits.is_null() {
            let _ = DeleteObject(color.into());
            return None;
        }
        let dst = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (px * px * 4) as usize);
        // Icons take straight (not premultiplied) BGRA.
        for (s, d) in pm.pixels().iter().zip(dst.as_chunks_mut::<4>().0) {
            let c = s.demultiply();
            *d = [c.blue(), c.green(), c.red(), c.alpha()];
        }
        let mask = CreateBitmap(px as i32, px as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: BOOL(1),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

/// Create the tray window and icon on the current (UI) thread.
pub fn create() -> Result<()> {
    // SAFETY: window-class registration and creation on this thread.
    unsafe {
        let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let class = w!("SwirvoTray");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(tray_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            anyhow::bail!("RegisterClassExW(tray) failed");
        }
        let hwnd = CreateWindowExW(
            Default::default(),
            class,
            w!("Swirvo"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("CreateWindowExW(tray)")?;
        TASKBAR_CREATED.store(
            RegisterWindowMessageW(w!("TaskbarCreated")) as isize,
            Ordering::Relaxed,
        );
        if let Some(icon) = make_icon(32) {
            ICON.store(icon.0 as isize, Ordering::Relaxed);
        }
        add_icon(hwnd);
        TRAY_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
    }
    Ok(())
}
