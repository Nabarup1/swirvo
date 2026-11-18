//! Per-user install, uninstall and "Start with Windows". No admin rights.
//!
//! - **Install** (`Swirvo.exe --install`): asks the running agent to quit and waits for it,
//!   copies Swirvo's files to `%LOCALAPPDATA%\Programs\Swirvo`, adds a Start-menu shortcut
//!   (so Swirvo shows up in Start search), an entry in Settings → Apps with an uninstaller,
//!   and the Run key when "Start with Windows" is on; then starts the installed copy.
//! - **Uninstall** (`--uninstall`, from Settings → Apps or our Settings window): asks
//!   first, closes Swirvo's windows, removes all of the above, optionally the settings, and
//!   deletes the program folder once this process has exited.
//! - **MSIX** (Microsoft Store): Windows owns install and uninstall; starting with Windows
//!   goes through the package's `windows.startupTask` (TaskId [`STARTUP_TASK`]).
//!
//! Registry values and the shortcut are written with APIs, and processes are started with
//! argument vectors (PROJECT_GUIDE rules 6–7); the one exception is documented at
//! [`delete_later`].

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW,
    RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IDYES, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    PostMessageW, RegisterWindowMessageW, WM_CLOSE,
};
use windows_core::{HSTRING, Interface, PCWSTR, w};

use crate::util::wide;

/// The app's identity for the taskbar, the Start-menu shortcut and notifications.
pub(crate) const APP_ID: &str = "Swirvo.Swirvo";
/// The MSIX manifest's `windows.startupTask` id.
pub(crate) const STARTUP_TASK: &str = "SwirvoStartup";

const RUN_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const UNINSTALL_KEY: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Swirvo");
const AGENT_MUTEX: PCWSTR = w!("Local\\Swirvo.Agent");
/// The files that make up Swirvo (what install copies and uninstall removes).
const FILES: [&str; 4] = [
    "Swirvo.exe",
    "swirvo-cli.exe",
    "THIRD-PARTY-NOTICES.txt",
    "How to use.txt",
];

/// Registered message: the agent quits (sent by install and uninstall).
pub(crate) fn quit_message() -> u32 {
    // SAFETY: plain registration of a named message.
    unsafe { RegisterWindowMessageW(w!("Swirvo.Quit")) }
}

/// Registered message: settings changed on disk; every Swirvo window reloads them.
pub(crate) fn settings_message() -> u32 {
    // SAFETY: plain registration of a named message.
    unsafe { RegisterWindowMessageW(w!("Swirvo.SettingsChanged")) }
}

/// Registered message: the recent-format history was cleared.
pub(crate) fn history_message() -> u32 {
    // SAFETY: plain registration of a named message.
    unsafe { RegisterWindowMessageW(w!("Swirvo.HistoryCleared")) }
}

/// Tell every top-level window (Swirvo's react to it) with a registered message.
pub(crate) fn broadcast(msg: u32) {
    // SAFETY: HWND_BROADCAST with a registered message is the documented way to reach other
    // apps' top-level windows; nothing is passed by pointer.
    unsafe {
        let _ = PostMessageW(Some(HWND(0xffff as *mut _)), msg, WPARAM(0), LPARAM(0));
    }
}

/// Running as an MSIX package (installed from the Microsoft Store)?
pub(crate) fn packaged() -> bool {
    let mut len = 0u32;
    // SAFETY: a size query with no buffer; APPMODEL_ERROR_NO_PACKAGE (15700) when unpackaged.
    let r = unsafe { GetCurrentPackageFullName(&mut len, None) };
    r.0 != 15700
}

/// `%LOCALAPPDATA%\Programs\Swirvo`.
pub(crate) fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").join("Swirvo"))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a.as_os_str().eq_ignore_ascii_case(b.as_os_str()),
    }
}

/// Is this copy the installed one?
pub(crate) fn is_installed() -> bool {
    let (Some(dir), Ok(exe)) = (install_dir(), std::env::current_exe()) else {
        return false;
    };
    exe.parent().is_some_and(|p| same_dir(p, &dir))
}

/// What the Settings window should offer.
pub(crate) fn state() -> swirvo_board::InstallState {
    use swirvo_board::InstallState;
    if packaged() {
        InstallState::Store
    } else if is_installed() {
        InstallState::Installed
    } else {
        InstallState::NotInstalled
    }
}

fn wstr(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// REG_SZ bytes (UTF-16 with its NUL).
fn sz(s: &OsStr) -> Vec<u8> {
    wstr(s).iter().flat_map(|c| c.to_le_bytes()).collect()
}

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: closing a key we opened.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn create_key(path: PCWSTR) -> Result<Key> {
    let mut k = HKEY::default();
    // SAFETY: creates/opens a key under HKCU into a local handle, closed by `Key`.
    let r = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            path,
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut k,
            None,
        )
    };
    if r != ERROR_SUCCESS {
        bail!("couldn't open the registry key: {r:?}");
    }
    Ok(Key(k))
}

fn set_sz(k: &Key, name: PCWSTR, value: &OsStr) -> Result<()> {
    let data = sz(value);
    // SAFETY: writes a REG_SZ from a local buffer.
    let r = unsafe { RegSetValueExW(k.0, name, None, REG_SZ, Some(&data)) };
    if r != ERROR_SUCCESS {
        bail!("couldn't write the registry: {r:?}");
    }
    Ok(())
}

fn set_dword(k: &Key, name: PCWSTR, v: u32) -> Result<()> {
    // SAFETY: writes a REG_DWORD from a local.
    let r = unsafe { RegSetValueExW(k.0, name, None, REG_DWORD, Some(&v.to_le_bytes())) };
    if r != ERROR_SUCCESS {
        bail!("couldn't write the registry: {r:?}");
    }
    Ok(())
}

/// The Run value's current command, if any.
fn run_value() -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: reads a REG_SZ into a local buffer of the stated size.
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            w!("Swirvo"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
}

/// The exe Windows should start at sign-in: the installed one, else this one.
fn startup_target() -> Option<PathBuf> {
    match install_dir().map(|d| d.join("Swirvo.exe")) {
        Some(p) if p.is_file() => Some(p),
        _ => std::env::current_exe().ok(),
    }
}

/// Turn "Start with Windows" on or off: the package's startup task under MSIX, otherwise
/// the per-user Run key. Does registry or WinRT work: call it off the UI thread.
pub(crate) fn set_startup(on: bool) -> Result<()> {
    if packaged() {
        use windows::ApplicationModel::{StartupTask, StartupTaskState};
        let task = StartupTask::GetAsync(&HSTRING::from(STARTUP_TASK))?.join()?;
        if on {
            if task.State()? == StartupTaskState::Disabled {
                task.RequestEnableAsync()?.join()?;
            }
        } else {
            task.Disable()?;
        }
        return Ok(());
    }
    if on {
        let exe = startup_target().context("can't find Swirvo.exe")?;
        // The command line Windows runs: the path quoted, no arguments.
        let cmd = format!("\"{}\"", exe.display());
        if run_value().as_deref() != Some(cmd.as_str()) {
            let k = create_key(RUN_KEY)?;
            set_sz(&k, w!("Swirvo"), OsStr::new(&cmd))?;
        }
    } else if run_value().is_some() {
        let mut k = HKEY::default();
        // SAFETY: opens the Run key to delete our value; closed by `Key`.
        unsafe {
            if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut k)
                == ERROR_SUCCESS
            {
                let k = Key(k);
                let _ = RegDeleteValueW(k.0, w!("Swirvo"));
            }
        }
    }
    Ok(())
}

/// Ask the running agent to quit and wait (up to 10 s) until it has.
fn stop_agent() {
    let msg = quit_message();
    // SAFETY: FindWindow/PostMessage on the agent's tray window by class; OpenMutex to see
    // whether the agent still exists (its named mutex goes when its process does).
    unsafe {
        if let Ok(h) = FindWindowW(w!("SwirvoTray"), PCWSTR::null())
            && !h.is_invalid()
        {
            let _ = PostMessageW(Some(h), msg, WPARAM(0), LPARAM(0));
        }
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(10) {
            match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, AGENT_MUTEX) {
                Ok(m) => {
                    let _ = CloseHandle(m);
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(_) => return,
            }
        }
    }
}

/// Close Swirvo's other windows (Board, page picker, Settings) so their exe can be replaced
/// or deleted.
fn close_windows() {
    for class in [w!("SwirvoBoard"), w!("SwirvoPages"), w!("SwirvoSettings")] {
        // SAFETY: WM_CLOSE to windows of our own classes, found by class name.
        unsafe {
            while let Ok(h) = FindWindowW(class, PCWSTR::null()) {
                if h.is_invalid() {
                    break;
                }
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
                std::thread::sleep(Duration::from_millis(150));
                if FindWindowW(class, PCWSTR::null()).is_ok_and(|g| g == h) {
                    break; // it didn't close (a dialog is open); go on without it
                }
            }
        }
    }
}

fn message(text: &str, ask: bool) -> bool {
    let t: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: a modal message box with NUL-terminated strings.
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(t.as_ptr()),
            w!("Swirvo"),
            if ask {
                MB_YESNO | MB_ICONQUESTION
            } else {
                MB_OK | MB_ICONINFORMATION
            },
        ) == IDYES
    }
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Swirvo.lnk`.
fn shortcut_path() -> Option<PathBuf> {
    // SAFETY: the returned string is copied, then freed with CoTaskMemFree.
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Programs, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(|s| PathBuf::from(s).join("Swirvo.lnk"))
    }
}

/// A Start-menu shortcut to `exe`, with its icon and the app's AppUserModelID (so the
/// taskbar groups Swirvo's windows under it). COM must be initialised on this thread.
fn make_shortcut(exe: &Path, link: &Path) -> Result<()> {
    // SAFETY: standard IShellLink/IPropertyStore/IPersistFile sequence with owned strings.
    unsafe {
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let e = wide(exe.as_os_str());
        sl.SetPath(PCWSTR(e.as_ptr()))?;
        if let Some(dir) = exe.parent() {
            let d = wide(dir.as_os_str());
            sl.SetWorkingDirectory(PCWSTR(d.as_ptr()))?;
        }
        sl.SetIconLocation(PCWSTR(e.as_ptr()), 0)?;
        sl.SetDescription(w!("Convert files with a drag and a tap"))?;
        let store: IPropertyStore = sl.cast()?;
        store.SetValue(
            &windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID,
            &PROPVARIANT::from(APP_ID),
        )?;
        store.Commit()?;
        let pf: IPersistFile = sl.cast()?;
        let l = wide(link.as_os_str());
        pf.Save(PCWSTR(l.as_ptr()), true)?;
    }
    Ok(())
}

fn dir_size_kb(dir: &Path) -> u32 {
    let bytes: u64 = std::fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    (bytes / 1024).min(u32::MAX as u64) as u32
}

/// Copy Swirvo's files from `from` into `to`. A file in use (a Board still open from an
/// older copy) is renamed aside first; leftovers are removed by the next install.
fn copy_files(from: &Path, exe: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
    if let Ok(rd) = std::fs::read_dir(to) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for name in FILES {
        let src = if name == "Swirvo.exe" {
            exe.to_path_buf()
        } else {
            from.join(name)
        };
        if !src.is_file() {
            continue;
        }
        let dst = to.join(name);
        if dst.exists() && std::fs::remove_file(&dst).is_err() {
            let aside = to.join(format!("{name}.{}.old", std::process::id()));
            std::fs::rename(&dst, &aside)
                .with_context(|| format!("{} is in use", dst.display()))?;
        }
        std::fs::copy(&src, &dst).with_context(|| format!("copying {name}"))?;
    }
    Ok(())
}

/// `Swirvo.exe --install`: install this copy for the current user, then start it.
pub(crate) fn install() -> Result<()> {
    if packaged() {
        bail!("this copy came from the Microsoft Store; Windows manages it");
    }
    let exe = std::env::current_exe().context("finding Swirvo.exe")?;
    let from = exe
        .parent()
        .context("finding Swirvo's folder")?
        .to_path_buf();
    let to = install_dir().context("no LOCALAPPDATA folder")?;
    stop_agent();
    if !same_dir(&from, &to) {
        close_windows();
        copy_files(&from, &exe, &to)?;
    }
    let installed = to.join("Swirvo.exe");
    // SAFETY: COM for the shortcut objects on this thread.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    if let Some(link) = shortcut_path() {
        make_shortcut(&installed, &link).context("creating the Start menu shortcut")?;
    }
    let k = create_key(UNINSTALL_KEY)?;
    let mut uninstall = std::ffi::OsString::from("\"");
    uninstall.push(installed.as_os_str());
    uninstall.push("\" --uninstall");
    let mut icon = installed.as_os_str().to_os_string();
    icon.push(",0");
    set_sz(&k, w!("DisplayName"), OsStr::new("Swirvo"))?;
    set_sz(
        &k,
        w!("DisplayVersion"),
        OsStr::new(env!("CARGO_PKG_VERSION")),
    )?;
    set_sz(&k, w!("Publisher"), OsStr::new("Swirvo"))?;
    set_sz(&k, w!("DisplayIcon"), &icon)?;
    set_sz(&k, w!("InstallLocation"), to.as_os_str())?;
    set_sz(&k, w!("UninstallString"), &uninstall)?;
    set_dword(&k, w!("NoModify"), 1)?;
    set_dword(&k, w!("NoRepair"), 1)?;
    set_dword(&k, w!("EstimatedSize"), dir_size_kb(&to))?;
    drop(k);
    if swirvo_jobs::settings::current().start_with_windows {
        set_startup(true)?;
    }
    std::process::Command::new(&installed)
        .spawn()
        .context("starting the installed Swirvo")?;
    Ok(())
}

/// Delete `dir` a few seconds after this process exits (it holds its own exe open).
///
/// The one place a command line is built from a string (PROJECT_GUIDE rule 7): `cmd /c`
/// has no argument vector of its own. It's only done for our install folder under
/// `%LOCALAPPDATA%`, and only when the path has none of the characters cmd treats
/// specially; otherwise the folder is left for the user (and opened in Explorer).
fn delete_later(dir: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    let s = dir.to_string_lossy();
    let safe = install_dir().is_some_and(|d| same_dir(&d, dir))
        && !s.chars().any(|c| "\"&|<>^%!()\r\n".contains(c));
    if !safe {
        return false;
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd.exe")
        .raw_arg(format!("/d /c ping -n 3 127.0.0.1 >nul & rd /s /q \"{s}\""))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

/// `Swirvo.exe --uninstall`.
pub(crate) fn uninstall() -> Result<()> {
    if packaged() {
        message(
            "Swirvo came from the Microsoft Store. Uninstall it from Settings → Apps.",
            false,
        );
        return Ok(());
    }
    if !message(
        "Uninstall Swirvo?\n\nThis removes Swirvo, its Start menu entry and starting with Windows. Your files and everything Swirvo made stay where they are.",
        true,
    ) {
        return Ok(());
    }
    let keep = message(
        "Keep your Swirvo settings and recent-format history, in case you install it again?",
        true,
    );
    stop_agent();
    close_windows();
    let _ = set_startup(false);
    if let Some(link) = shortcut_path() {
        let _ = std::fs::remove_file(link);
    }
    // SAFETY: deletes our own uninstall entry under HKCU.
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, UNINSTALL_KEY);
    }
    if !keep && let Some(d) = swirvo_jobs::data_dir() {
        let _ = std::fs::remove_dir_all(d);
    }
    let dir = install_dir();
    let running_from_it = dir.as_ref().is_some_and(|_| is_installed());
    match dir {
        Some(d) if running_from_it => {
            if !delete_later(&d) {
                message(
                    &format!(
                        "Swirvo is uninstalled. Delete this folder to finish:\n\n{}",
                        d.display()
                    ),
                    false,
                );
            }
        }
        Some(d) => {
            let _ = std::fs::remove_dir_all(d);
        }
        None => {}
    }
    Ok(())
}

/// The first time a copy that isn't installed starts: offer to install it. Runs on its own
/// thread (a message box waits for the user); remembers that it asked.
pub(crate) fn offer_once() {
    std::thread::spawn(|| {
        let s = swirvo_jobs::settings::current();
        if s.install_offered || packaged() || is_installed() {
            return;
        }
        let mut s = (*s).clone();
        s.install_offered = true;
        let _ = swirvo_jobs::settings::save(&s);
        if message(
            "Install Swirvo?\n\nIt copies Swirvo to your user folder and adds it to the Start menu, so you can find it in Start search and it starts with Windows (you can turn that off in Settings). No administrator rights needed.\n\nYou can also do this later from the tray icon's menu.",
            true,
        ) && let Ok(exe) = std::env::current_exe()
        {
            let _ = std::process::Command::new(exe).arg("--install").spawn();
        }
    });
}

/// At start: keep the Run key pointing at this copy when it's the installed one (it may
/// have moved), as the setting says. Off the UI thread.
pub(crate) fn sync_startup_later() {
    std::thread::spawn(|| {
        if packaged() || is_installed() {
            let _ = set_startup(swirvo_jobs::settings::current().start_with_windows);
        }
    });
}
