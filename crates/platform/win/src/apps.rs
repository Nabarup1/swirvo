//! The Board and page-picker processes (docs/12 §6): the same `Swirvo.exe`, started
//! with `--board [files…]` or `--pages <tool> <file>`. Started by the agent with argument
//! vectors only (PROJECT_GUIDE.md rule 7).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use swirvo_board::{Board, PagePicker, SettingsApp};
use swirvo_jobs::Toolkit;
use swirvo_jobs::tools::{PageOp, Tool};
use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
use windows_core::w;

use crate::window::{self, WindowSpec};

const BOARD_CLASS: windows_core::PCWSTR = w!("SwirvoBoard");
const PAGES_CLASS: windows_core::PCWSTR = w!("SwirvoPages");
const SETTINGS_CLASS: windows_core::PCWSTR = w!("SwirvoSettings");

/// Open the Settings window (an open one just comes forward: it is single-instance).
pub(crate) fn launch_settings() -> std::io::Result<()> {
    spawn_self(&["--settings".as_ref()])
}

/// Run `Swirvo.exe --install` or `--uninstall` as its own process (it may stop this one).
pub(crate) fn launch_install(uninstall: bool) -> std::io::Result<()> {
    spawn_self(&[if uninstall {
        "--uninstall"
    } else {
        "--install"
    }
    .as_ref()])
}

/// `--settings`: the Settings window, one per user session.
pub(crate) fn settings() -> Result<()> {
    // SAFETY: named mutex for single-instance; the handle lives for the process.
    unsafe {
        let _m = CreateMutexW(None, false, w!("Local\\Swirvo.Settings")).context("CreateMutexW")?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(h) = windows::Win32::UI::WindowsAndMessaging::FindWindowW(
                SETTINGS_CLASS,
                windows_core::PCWSTR::null(),
            ) && !h.is_invalid()
            {
                let _ = windows::Win32::UI::WindowsAndMessaging::ShowWindow(
                    h,
                    windows::Win32::UI::WindowsAndMessaging::SW_RESTORE,
                );
                let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(h);
            }
            return Ok(());
        }
    }
    let app_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let install = crate::install::state();
    window::run(
        WindowSpec {
            class: SETTINGS_CLASS,
            size: (880.0, 640.0),
        },
        move |waker| Box::new(SettingsApp::new(waker, app_dir, install)),
    )
}

fn spawn_self(args: &[&std::ffi::OsStr]) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    // SAFETY: lets the new process take the foreground (it opens a window for the user).
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    std::process::Command::new(exe)
        .args(args)
        .spawn()
        .map(|_| ())
}

/// Open the Board (or add `files` to the open one), optionally on the Tools tab with
/// `tool`'s options ("" = the tool list).
pub(crate) fn launch_board(files: &[PathBuf], tool: Option<&str>) -> std::io::Result<()> {
    let mut args: Vec<&std::ffi::OsStr> = vec!["--board".as_ref()];
    if let Some(t) = tool {
        args.push("--tool".as_ref());
        args.push(t.as_ref());
    }
    args.extend(files.iter().map(|f| f.as_os_str()));
    spawn_self(&args)
}

/// Open the page picker for one PDF, starting on `tool`'s operation.
pub(crate) fn launch_pages(tool: Tool, file: &Path) -> std::io::Result<()> {
    spawn_self(&["--pages".as_ref(), tool.id().as_ref(), file.as_os_str()])
}

/// Open the page picker with what a form had typed (`spec`: Split ranges' groups).
pub(crate) fn launch_pages_with(tool: Tool, file: &Path, spec: &str) -> std::io::Result<()> {
    if spec.trim().is_empty() {
        return launch_pages(tool, file);
    }
    spawn_self(&[
        "--pages".as_ref(),
        tool.id().as_ref(),
        file.as_os_str(),
        spec.as_ref(),
    ])
}

/// `--board [--tool ID] [files…]`.
pub(crate) fn board(args: &[std::ffi::OsString]) -> Result<()> {
    let (tool, rest) = match args {
        [flag, id, rest @ ..] if flag == "--tool" => {
            (Some(id.to_string_lossy().into_owned()), rest)
        }
        rest => (None, rest),
    };
    let files: Vec<PathBuf> = rest.iter().map(PathBuf::from).collect();
    // SAFETY: named mutex for single-instance; the handle lives for the process.
    unsafe {
        let _m = CreateMutexW(None, false, w!("Local\\Swirvo.Board")).context("CreateMutexW")?;
        if GetLastError() == ERROR_ALREADY_EXISTS
            && window::send_to_existing(BOARD_CLASS, &files, tool.as_deref())
        {
            return Ok(());
        }
        let tk = Arc::new(Toolkit::new().context("building the conversion graph")?);
        window::run(
            WindowSpec {
                class: BOARD_CLASS,
                size: (1240.0, 780.0),
            },
            move |waker| {
                let mut b = Board::new(tk, waker, files);
                if let Some(t) = &tool {
                    b.open_tool(t);
                }
                Box::new(b)
            },
        )
    }
}

pub(crate) fn pages(
    tool: Option<&std::ffi::OsString>,
    file: Option<&std::ffi::OsString>,
    spec: Option<&std::ffi::OsString>,
) -> Result<()> {
    let file = PathBuf::from(file.context("usage: Swirvo.exe --pages <tool> <file.pdf>")?);
    let op = tool
        .and_then(|t| t.to_str())
        .and_then(Tool::from_id)
        .and_then(Tool::page_op)
        .unwrap_or(PageOp::Images {
            png: false,
            dpi: 150,
        });
    let tk = Arc::new(Toolkit::new().context("building the conversion graph")?);
    window::run(
        WindowSpec {
            class: PAGES_CLASS,
            size: (1060.0, 740.0),
        },
        move |waker| {
            let mut p = PagePicker::new(tk, waker, file, op);
            // Split ranges opens on Groups, with the groups typed in the Board.
            if tool.and_then(|t| t.to_str()) == Some(Tool::PdfSplitRanges.id()) {
                p.start_groups(spec.and_then(|s| s.to_str()).unwrap_or(""));
            }
            Box::new(p)
        },
    )
}
