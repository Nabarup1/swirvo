//! 7Z, RAR, ISO and CAB through the system's bsdtar (libarchive). Extraction refuses
//! absolute paths and `..` by default (no `-P`), and never follows symlinks outside.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, StepRequest};

/// Formats only libarchive reads (ours handle zip/tar/gz/bz2/xz/zst).
pub const FORMATS: &[&str] = &["7z", "rar", "iso", "cab"];

pub struct SysTar {
    exe: PathBuf,
}

fn find_exe() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let p = Path::new(&root).join("System32").join("tar.exe");
        if p.is_file() {
            return Some(p);
        }
        None
    }
    #[cfg(not(windows))]
    {
        crate::external::which(&["bsdtar"], &[])
    }
}

/// The bsdtar executable, looked up once.
pub fn exe() -> Option<&'static Path> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(find_exe).as_deref()
}

impl SysTar {
    pub fn find() -> Option<SysTar> {
        exe().map(|e| SysTar {
            exe: e.to_path_buf(),
        })
    }
}

/// Extract `archive` into `dir` (which must exist and be empty).
pub fn extract(archive: &Path, dir: &Path, cancel: &CancelToken) -> Result<(), EngineError> {
    let exe =
        exe().ok_or_else(|| EngineError::Invalid("no system tar (libarchive) found".into()))?;
    let mut cmd = Command::new(exe);
    cmd.arg("-x").arg("-f").arg(archive).arg("-C").arg(dir);
    crate::external::run(cmd, cancel, "tar")
}

impl Engine for SysTar {
    fn id(&self) -> &'static str {
        "systar"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut v = Vec::new();
        for f in FORMATS {
            v.push(
                Capability::new("systar", f, "zip")
                    .loss(0)
                    .time(300, 60)
                    .note("repacked by the system's libarchive"),
            );
            v.push(
                Capability::new("systar", f, "tar")
                    .loss(0)
                    .time(250, 40)
                    .note("repacked by the system's libarchive"),
            );
        }
        v
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let format = match req.to {
            "zip" => "zip",
            "tar" => "pax",
            other => return Err(EngineError::Invalid(format!("systar can't write {other}"))),
        };
        // `@archive` copies every entry of another archive (libarchive reads it).
        let mut src = std::ffi::OsString::from("@");
        src.push(req.input.as_os_str());
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--format")
            .arg(format)
            .arg("-c")
            .arg("-f")
            .arg(req.output)
            .arg(src);
        crate::external::run(cmd, req.cancel, "tar")
    }
}
