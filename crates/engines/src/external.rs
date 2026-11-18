//! Engine packs that are separate programs (docs/03 §12): FFmpeg ("Media") and LibreOffice
//! ("Office"). They're discovered on this PC at startup; when absent, their formats simply
//! don't appear in the wheel. They run as child processes with argument vectors only
//! (PROJECT_GUIDE.md rule 7), no console window, and are killed on cancel.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use swirvo_core::{CancelToken, Capability, Engine, EngineError, Preset, StepRequest};

const VIDEO: &[&str] = &[
    "mp4", "mov", "mkv", "webm", "avi", "wmv", "flv", "ts", "3gp", "ogv",
];
const AUDIO: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff", "wma", "amr", "ac3", "caf",
];
const VIDEO_OUT: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "gif"];
const AUDIO_OUT: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "aiff"];

/// Find an executable: explicit candidates first, then every PATH entry. Only native
/// programs count: under WSL, Windows `.exe`s on the shared PATH can't open Linux paths.
pub(crate) fn which(names: &[&str], extra: &[PathBuf]) -> Option<PathBuf> {
    let native =
        |p: &Path| cfg!(windows) == p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if let Some(p) = extra.iter().find(|p| native(p) && p.is_file()) {
        return Some(p.clone());
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for n in names.iter().filter(|n| native(Path::new(n))) {
            let p = dir.join(n);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn local_app_data() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

/// The FFmpeg program on this PC, if any: our engine-pack folder, winget's links, PATH.
pub fn ffmpeg_exe() -> Option<PathBuf> {
    let mut extra = Vec::new();
    if let Some(l) = local_app_data() {
        extra.push(
            l.join("Swirvo")
                .join("engines")
                .join("ffmpeg")
                .join("bin")
                .join("ffmpeg.exe"),
        );
        extra.push(
            l.join("Swirvo")
                .join("engines")
                .join("ffmpeg")
                .join("ffmpeg.exe"),
        );
        extra.push(
            l.join("Microsoft")
                .join("WinGet")
                .join("Links")
                .join("ffmpeg.exe"),
        );
    }
    which(&["ffmpeg.exe", "ffmpeg"], &extra)
}

/// Every engine pack found on this PC.
pub fn discover() -> Vec<Box<dyn Engine>> {
    discover_with(crate::Packs::default())
}

/// The engine packs found on this PC that `packs` allows.
pub fn discover_with(packs: crate::Packs) -> Vec<Box<dyn Engine>> {
    let mut out: Vec<Box<dyn Engine>> = Vec::new();
    if packs.ffmpeg
        && let Some(exe) = ffmpeg_exe()
    {
        out.push(Box::new(FfmpegEngine { exe }));
    }
    if packs.libreoffice
        && let Some(exe) = libreoffice_exe()
    {
        out.push(Box::new(OfficeEngine { exe }));
    }
    out
}

/// LibreOffice's `soffice` on this PC, if any.
pub fn libreoffice_exe() -> Option<PathBuf> {
    let mut lo_extra = Vec::new();
    for pf in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(d) = std::env::var_os(pf) {
            lo_extra.push(
                PathBuf::from(d)
                    .join("LibreOffice")
                    .join("program")
                    .join("soffice.exe"),
            );
        }
    }
    lo_extra.push(PathBuf::from("/usr/bin/soffice"));
    lo_extra.push(PathBuf::from("/usr/lib/libreoffice/program/soffice"));
    lo_extra.push(PathBuf::from("/opt/libreoffice/program/soffice"));
    which(&["soffice.exe", "soffice", "libreoffice"], &lo_extra)
}

/// Run a child to completion, polling `cancel`. Stderr's tail becomes the error message.
pub(crate) fn run(cmd: Command, cancel: &CancelToken, what: &str) -> Result<(), EngineError> {
    let (ok, err) = run_capture(cmd, cancel, what)?;
    if ok {
        return Ok(());
    }
    let tail: Vec<&str> = err
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(3)
        .collect();
    Err(EngineError::Encode(format!(
        "{what} failed: {}",
        tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
    )))
}

/// Run a child to completion, polling `cancel`: (exited successfully, everything it wrote
/// to stderr). For programs that report on stderr, like `ffmpeg -i` without an output.
pub(crate) fn run_capture(
    mut cmd: Command,
    cancel: &CancelToken,
    what: &str,
) -> Result<(bool, String), EngineError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| EngineError::Invalid(format!("couldn't start {what}: {e}")))?;
    let stderr = child.stderr.take();
    // Drain stderr on a thread so a chatty child never blocks on a full pipe.
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut e) = stderr {
            let _ = std::io::Read::read_to_string(&mut e, &mut s);
        }
        s
    });
    let status = loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(EngineError::Cancelled);
        }
        match child.try_wait()? {
            Some(s) => break s,
            None => std::thread::sleep(Duration::from_millis(40)),
        }
    };
    let err = reader.join().unwrap_or_default();
    Ok((status.success(), err))
}

// ---------------------------------------------------------------- FFmpeg

pub struct FfmpegEngine {
    exe: PathBuf,
}

fn muxer(to: &str) -> &'static str {
    match to {
        "mp4" => "mp4",
        "mov" => "mov",
        "mkv" => "matroska",
        "webm" => "webm",
        "avi" => "avi",
        "gif" => "gif",
        "mp3" => "mp3",
        "m4a" => "ipod",
        "aac" => "adts",
        "wav" => "wav",
        "flac" => "flac",
        "ogg" => "ogg",
        "opus" => "opus",
        _ => "aiff",
    }
}

impl Engine for FfmpegEngine {
    fn id(&self) -> &'static str {
        "ffmpeg"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        let lossless = |t: &str| matches!(t, "wav" | "flac" | "aiff");
        for &from in VIDEO {
            for &to in VIDEO_OUT.iter().chain(AUDIO_OUT) {
                if from == to {
                    continue;
                }
                let cap = Capability::new("ffmpeg", from, to).time(400, 900);
                caps.push(match to {
                    "gif" => cap.loss(40).note("animated GIF, 12 fps, up to 720 px wide"),
                    t if AUDIO_OUT.contains(&t) => cap
                        .loss(if lossless(t) { 1 } else { 10 })
                        .time(300, 150)
                        .note("extracts the audio track"),
                    _ => cap.loss(10).note("re-encoded with FFmpeg"),
                });
            }
        }
        for &from in AUDIO {
            for &to in AUDIO_OUT {
                if from != to {
                    caps.push(
                        Capability::new("ffmpeg", from, to)
                            .loss(if lossless(to) { 1 } else { 10 })
                            .time(250, 120)
                            .note("FFmpeg"),
                    );
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let mut cmd = Command::new(&self.exe);
        cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-i"])
            .arg(req.input);
        let q = |best: &'static str, bal: &'static str, small: &'static str| match req.preset {
            Preset::Best => best,
            Preset::Smallest => small,
            _ => bal,
        };
        let video_src = VIDEO.contains(&req.from);
        match req.to {
            "gif" => {
                cmd.args(["-vf", "fps=12,scale='min(720,iw)':-2:flags=lanczos,split[a][b];[a]palettegen[p];[b][p]paletteuse", "-loop", "0"]);
            }
            "mp4" | "mov" | "mkv" => {
                cmd.args(["-pix_fmt", "yuv420p", "-crf", q("18", "23", "28")]);
                if req.to != "mkv" {
                    cmd.args(["-movflags", "+faststart"]);
                }
            }
            "webm" => {
                cmd.args(["-b:v", "0", "-crf", q("24", "32", "40")]);
            }
            "avi" => {}
            t if AUDIO_OUT.contains(&t) => {
                if video_src {
                    cmd.arg("-vn");
                }
                match t {
                    "mp3" => cmd.args(["-q:a", q("0", "2", "5")]),
                    "m4a" | "aac" => cmd.args(["-c:a", "aac", "-b:a", q("256k", "192k", "96k")]),
                    "opus" => cmd.args(["-b:a", q("192k", "128k", "64k")]),
                    "ogg" => cmd.args(["-q:a", q("8", "5", "2")]),
                    _ => &mut cmd,
                };
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "ffmpeg engine can't write {other}"
                )));
            }
        }
        cmd.args(["-f", muxer(req.to)]).arg(req.output);
        run(cmd, req.cancel, "FFmpeg")
    }
}

// ---------------------------------------------------------------- LibreOffice

pub struct OfficeEngine {
    exe: PathBuf,
}

const WRITER: &[&str] = &["docx", "doc", "odt", "rtf"];
const IMPRESS: &[&str] = &["pptx", "ppt", "odp"];
const CALC: &[&str] = &["xlsx", "xls", "ods"];

fn filter(to: &str) -> &'static str {
    match to {
        "pdf" => "pdf",
        "docx" => "docx:MS Word 2007 XML",
        "doc" => "doc:MS Word 97",
        "odt" => "odt",
        "rtf" => "rtf",
        "pptx" => "pptx:Impress MS PowerPoint 2007 XML",
        "ppt" => "ppt:MS PowerPoint 97",
        "odp" => "odp",
        "xlsx" => "xlsx:Calc MS Excel 2007 XML",
        "xls" => "xls:MS Excel 97",
        _ => "ods",
    }
}

/// Where Swirvo keeps its private LibreOffice profile.
fn office_profile() -> Option<PathBuf> {
    let base = std::env::var_os("SWIRVO_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| local_app_data().map(|l| l.join("Swirvo")))
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(|c| PathBuf::from(c).join("swirvo")))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("swirvo"))
        })?;
    let dir = base.join("engines").join("libreoffice-profile");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// `file:///C:/Users/a%20b/...` for LibreOffice's -env:UserInstallation.
fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if s.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Engine for OfficeEngine {
    fn id(&self) -> &'static str {
        "office"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for group in [WRITER, IMPRESS, CALC] {
            for &from in group {
                caps.push(
                    Capability::new("office", from, "pdf")
                        .loss(1)
                        .time(2500, 300)
                        .note("LibreOffice on this PC: its own layout"),
                );
                for &to in group {
                    if from != to {
                        caps.push(
                            Capability::new("office", from, to)
                                .loss(2)
                                .time(2500, 300)
                                .note("LibreOffice"),
                        );
                    }
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        // One Office job at a time, with Swirvo' own persistent LibreOffice profile: the
        // first run builds it (~15–20 s), later runs start in a couple of seconds, and the
        // user's own LibreOffice profile is never touched.
        static OFFICE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one_at_a_time = OFFICE.lock().unwrap_or_else(|p| p.into_inner());
        let work = tempfile::tempdir()?;
        let profile = office_profile().unwrap_or_else(|| work.path().join("profile"));
        let outdir = work.path().join("out");
        std::fs::create_dir_all(&outdir)?;
        // LibreOffice names the output after the input; give it a stable input name.
        let ext = req.from;
        let input = work.path().join(format!("input.{ext}"));
        std::fs::copy(req.input, &input)?;
        let mut cmd = Command::new(&self.exe);
        cmd.arg(format!("-env:UserInstallation={}", file_url(&profile)))
            .args([
                "--headless",
                "--norestore",
                "--nologo",
                "--nodefault",
                "--nolockcheck",
                "--convert-to",
                filter(req.to),
                "--outdir",
            ])
            .arg(&outdir)
            .arg(&input);
        run(cmd, req.cancel, "LibreOffice")?;
        let produced = outdir.join(format!("input.{}", req.to));
        if !produced.is_file() {
            return Err(EngineError::Encode(
                "LibreOffice didn't produce a file".into(),
            ));
        }
        // req.output is our own temp path; replace it.
        let _ = std::fs::remove_file(req.output);
        if std::fs::rename(&produced, req.output).is_err() {
            std::fs::copy(&produced, req.output)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_capabilities() {
        assert_eq!(
            file_url(Path::new(r"C:\Users\a b\p")),
            "file:///C:/Users/a%20b/p"
        );
        assert_eq!(file_url(Path::new("/tmp/x y")), "file:///tmp/x%20y");
        let ff = FfmpegEngine {
            exe: PathBuf::from("ffmpeg"),
        };
        let caps = ff.capabilities();
        assert!(caps.iter().any(|c| c.from == "mp4" && c.to == "mp3"));
        assert!(
            caps.iter()
                .any(|c| c.from == "mov" && c.to == "gif" && c.loss > 0)
        );
        let lo = OfficeEngine {
            exe: PathBuf::from("soffice"),
        };
        assert!(
            lo.capabilities()
                .iter()
                .any(|c| c.from == "docx" && c.to == "pdf" && c.loss == 1)
        );
    }
}
