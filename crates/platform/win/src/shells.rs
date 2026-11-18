//! The shells the Terminal action can open (docs/01 §9), found once at startup.
//!
//! Only programs that exist on this PC are offered, best first: Windows Terminal,
//! PowerShell (7 if installed, else Windows PowerShell), Command Prompt, the WSL distros
//! (default first) and Git Bash. Dropping on the Terminal wedge opens the first; the
//! sub-ring picks another.
//!
//! Console shells start in a new console, so Windows' own "default terminal app" setting
//! decides whether they appear in Windows Terminal or the classic console window. Every
//! launch is an argument vector (PROJECT_GUIDE.md rule 7).

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_SZ, RegCloseKey,
    RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows_core::{PCWSTR, PWSTR, w};

const CREATE_NEW_CONSOLE: u32 = 0x10;
/// About as many labels as the sub-ring shows legibly.
const MAX_SHELLS: usize = 6;
/// Longer distro names are shortened to fit a sub-ring item.
const MAX_LABEL: usize = 12;

#[derive(Debug, Clone)]
enum Kind {
    /// `wt.exe -d <dir>`: Windows Terminal with the user's default profile.
    WindowsTerminal,
    /// A console program started in the folder (PowerShell, Command Prompt).
    Console,
    /// `wsl.exe -d <distro> --cd <dir>`.
    Wsl(OsString),
    /// Git for Windows' launcher: `git-bash.exe --cd=<dir>`.
    GitBash,
}

#[derive(Debug, Clone)]
pub(crate) struct Shell {
    pub label: String,
    exe: PathBuf,
    kind: Kind,
}

static SHELLS: OnceLock<Vec<Shell>> = OnceLock::new();

/// Everything found on this PC, best first. Detected on first use (a few file and
/// registry lookups, well under a millisecond).
pub(crate) fn all() -> &'static [Shell] {
    SHELLS.get_or_init(detect)
}

/// Sub-ring labels, in the same order as [`all`].
pub(crate) fn labels() -> Vec<String> {
    all().iter().map(|s| s.label.clone()).collect()
}

/// Open shell `index` (from the sub-ring; out of range means the default) in `dir`.
pub(crate) fn open(index: usize, dir: &Path) -> std::io::Result<()> {
    let shells = all();
    let shell = shells.get(index).or(shells.first()).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no terminal found on this PC")
    })?;
    let dir = plain(dir);
    let mut cmd = Command::new(&shell.exe);
    match &shell.kind {
        Kind::WindowsTerminal => {
            cmd.arg("-d").arg(wt_escape(dir.as_os_str()));
        }
        Kind::Console => {
            cmd.current_dir(&dir).creation_flags(CREATE_NEW_CONSOLE);
        }
        Kind::Wsl(distro) => {
            cmd.arg("-d")
                .arg(distro)
                .arg("--cd")
                .arg(&dir)
                .creation_flags(CREATE_NEW_CONSOLE);
        }
        Kind::GitBash => {
            // The same form as Git's own "Git Bash Here" menu entry.
            let mut cd = OsString::from("--cd=");
            cd.push(dir.as_os_str());
            cmd.arg(cd).current_dir(&dir);
        }
    }
    cmd.spawn().map(|_| ())
}

fn detect() -> Vec<Shell> {
    let system = env_dir("SystemRoot")
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("System32");
    let mut out = Vec::new();
    let mut add = |label: &str, exe: PathBuf, kind: Kind| {
        out.push(Shell {
            label: label.to_string(),
            exe,
            kind,
        })
    };

    let wt = env_dir("LOCALAPPDATA")
        .map(|d| d.join(r"Microsoft\WindowsApps\wt.exe"))
        .filter(|p| exists(p))
        .or_else(|| on_path("wt.exe"));
    if let Some(exe) = wt {
        add("Terminal", exe, Kind::WindowsTerminal);
    }

    let pwsh = env_dir("ProgramFiles")
        .map(|d| d.join(r"PowerShell\7\pwsh.exe"))
        .filter(|p| exists(p))
        .or_else(|| on_path("pwsh.exe"));
    let powershell = pwsh.or_else(|| {
        Some(system.join(r"WindowsPowerShell\v1.0\powershell.exe")).filter(|p| exists(p))
    });
    if let Some(exe) = powershell {
        add("PowerShell", exe, Kind::Console);
    }

    let cmd = system.join("cmd.exe");
    if exists(&cmd) {
        add("Cmd", cmd, Kind::Console);
    }

    let wsl = system.join("wsl.exe");
    let mut distros = if exists(&wsl) {
        wsl_distros()
    } else {
        Vec::new()
    }
    .into_iter();
    // The default distro comes before Git Bash; the others after it.
    if let Some(d) = distros.next() {
        add(&short(&d), wsl.clone(), Kind::Wsl(d));
    }
    if let Some(exe) = git_bash() {
        add("Git Bash", exe, Kind::GitBash);
    }
    for d in distros {
        add(&short(&d), wsl.clone(), Kind::Wsl(d));
    }

    out.truncate(MAX_SHELLS);
    out
}

/// Installed WSL distros for this user, default first; Docker Desktop's internal ones
/// are skipped. Read from the registry, so WSL itself isn't started.
fn wsl_distros() -> Vec<OsString> {
    let base = r"Software\Microsoft\Windows\CurrentVersion\Lxss";
    let default = reg_string(HKEY_CURRENT_USER, base, w!("DefaultDistribution"));
    let mut found: Vec<(bool, OsString)> = Vec::new();
    for guid in subkeys(HKEY_CURRENT_USER, base) {
        let path = format!("{base}\\{}", guid.to_string_lossy());
        let Some(name) = reg_string(HKEY_CURRENT_USER, &path, w!("DistributionName")) else {
            continue;
        };
        if name.to_string_lossy().starts_with("docker-desktop") {
            continue;
        }
        let is_default = default.as_ref().is_some_and(|d| {
            d.to_string_lossy()
                .eq_ignore_ascii_case(&guid.to_string_lossy())
        });
        found.push((is_default, name));
    }
    // Stable: default first, then registry order.
    found.sort_by_key(|(is_default, _)| !is_default);
    found.into_iter().map(|(_, n)| n).collect()
}

fn git_bash() -> Option<PathBuf> {
    let from_registry = reg_string(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\GitForWindows",
        w!("InstallPath"),
    )
    .map(|d| PathBuf::from(d).join("git-bash.exe"));
    let candidates = [
        from_registry,
        env_dir("ProgramFiles").map(|d| d.join(r"Git\git-bash.exe")),
        env_dir("LOCALAPPDATA").map(|d| d.join(r"Programs\Git\git-bash.exe")),
    ];
    candidates.into_iter().flatten().find(|p| exists(p))
}

fn short(name: &OsStr) -> String {
    let s = name.to_string_lossy();
    if s.chars().count() > MAX_LABEL {
        format!("{}…", s.chars().take(MAX_LABEL - 1).collect::<String>())
    } else {
        s.into_owned()
    }
}

/// App execution aliases (`WindowsApps\wt.exe`) are reparse points that can't be opened,
/// so look at the link itself rather than following it.
fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

fn env_dir(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn on_path(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(exe))
        .find(|p| exists(p))
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\srv\share` → `\\srv\share`: shells don't accept
/// verbatim paths as a working directory. Anything else is returned unchanged.
fn plain(dir: &Path) -> PathBuf {
    let w: Vec<u16> = dir.as_os_str().encode_wide().collect();
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let unc: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
    if w.starts_with(&unc) {
        let mut out: Vec<u16> = r"\\".encode_utf16().collect();
        out.extend_from_slice(&w[unc.len()..]);
        PathBuf::from(OsString::from_wide(&out))
    } else if w.starts_with(&verbatim) {
        PathBuf::from(OsString::from_wide(&w[verbatim.len()..]))
    } else {
        dir.to_path_buf()
    }
}

/// Windows Terminal splits its command line on `;`, so a folder named `a;b` must be
/// passed as `a\;b`.
fn wt_escape(dir: &OsStr) -> OsString {
    let mut out = Vec::new();
    for c in dir.encode_wide() {
        if c == u16::from(b';') {
            out.push(u16::from(b'\\'));
        }
        out.push(c);
    }
    OsString::from_wide(&out)
}

fn wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_string(root: HKEY, path: &str, value: PCWSTR) -> Option<OsString> {
    let path = wide_z(path);
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: reads one REG_SZ into a local buffer whose byte size is passed in.
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(path.as_ptr()),
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if r.is_err() {
        return None;
    }
    let n = (bytes as usize / 2).min(buf.len());
    let s = &buf[..n];
    let end = s.iter().position(|&c| c == 0).unwrap_or(n);
    Some(OsString::from_wide(&s[..end])).filter(|s| !s.is_empty())
}

fn subkeys(root: HKEY, path: &str) -> Vec<OsString> {
    let path = wide_z(path);
    let mut key = HKEY::default();
    // SAFETY: opens a key for reading; closed below.
    if unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), Some(0), KEY_READ, &mut key) }.is_err() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..256 {
        let mut name = [0u16; 256];
        let mut len = name.len() as u32;
        // SAFETY: `name` holds `len` UTF-16 units; the other out-parameters are unused.
        let r = unsafe {
            RegEnumKeyExW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if r.is_err() {
            break;
        }
        out.push(OsString::from_wide(&name[..(len as usize).min(name.len())]));
    }
    // SAFETY: closing the key opened above.
    unsafe {
        let _ = RegCloseKey(key);
    }
    out
}
