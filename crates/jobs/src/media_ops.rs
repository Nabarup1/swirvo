//! Running the video tools (docs/11 §2): a frame as a picture, frames every N seconds,
//! and compress to a size. New files go next to the video; nothing is replaced.

use std::path::{Path, PathBuf};

use swirvo_core::{CancelToken, EngineError};
use swirvo_engines::video::{self, Still};

use crate::Toolkit;
use crate::args::{Field, ToolArgs, Values};
use crate::tools::{Tool, ToolError, ToolResult, emit, into_folder, name_of, parent_of};

/// The options each video tool takes.
pub fn fields(tool: Tool) -> &'static [Field] {
    static FRAME: &[Field] = &[
        Field::text("at", "Time", "").hint("e.g. 1:23 or 40 %; empty: a good frame near the start"),
        Field::number("every", "Or one every N seconds", "0", 0.0, 3600.0)
            .hint("0: just one picture; more go in a new folder"),
        Field::choice(
            "format",
            "Format",
            &[("png", "PNG · exact"), ("jpg", "JPG · smaller")],
            "png",
        ),
    ];
    static COMPRESS: &[Field] = &[
        Field::choice(
            "size",
            "Target size",
            &[
                ("25", "Email · 25 MB"),
                ("10", "Chat · 10 MB"),
                ("8", "Small chat · 8 MB"),
                ("50", "Upload · 50 MB"),
                ("100", "Large upload · 100 MB"),
                ("other", "Other size"),
            ],
            "25",
        ),
        Field::number("mb", "Other size (MB)", "", 0.1, 100_000.0)
            .hint("Used with Other size; 1 MB = 1,000,000 bytes"),
    ];
    match tool {
        Tool::VideoFrame => FRAME,
        Tool::VideoCompress => COMPRESS,
        _ => &[],
    }
}

/// Tools whose options must be filled in before they run.
pub fn asks(tool: Tool) -> bool {
    tool == Tool::VideoCompress
}

fn engine(e: EngineError) -> ToolError {
    ToolError::Engine(e)
}

/// The target in bytes (decimal megabytes, so a "25 MB" limit is never crossed).
fn target(v: &Values<'_>) -> Result<u64, ToolError> {
    let mb = match v.str("size").trim() {
        "other" => v.num("mb").ok_or_else(|| {
            ToolError::Invalid("type the size you need under Other size (MB)".into())
        })?,
        s => match v.num("mb") {
            // A size typed without picking "Other size" still counts.
            Some(mb) => mb,
            None => s
                .parse::<f64>()
                .map_err(|_| ToolError::Invalid(format!("unknown size “{s}”")))?,
        },
    };
    Ok((mb * 1e6) as u64)
}

/// Run a video tool. Per-file tools keep going after a failure and report it.
pub fn run(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    args: &ToolArgs,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let v = Values::new(fields(tool), args).map_err(ToolError::Invalid)?;
    let mut r = ToolResult::default();
    let mut lines = Vec::new();
    let mut note = None;
    // Two videos at a time: encoders already use every core, so more only adds memory.
    let done =
        crate::tools::par_map(
            files,
            crate::tools::workers(files.len(), 2),
            cancel,
            |f| match tool {
                Tool::VideoFrame => frame(tk, f, &v, cancel),
                Tool::VideoCompress => compress(tk, f, &v, cancel),
                other => Err(ToolError::Invalid(format!("{} can't run here", other.id()))),
            },
        );
    cancel.check().map_err(engine)?;
    for (f, res) in files.iter().zip(done) {
        match res {
            Some(Ok((outs, msg, n))) => {
                r.outputs.extend(outs);
                lines.push(msg);
                note = note.or(n);
            }
            Some(Err(ToolError::Engine(EngineError::Cancelled))) => {
                return Err(ToolError::Engine(EngineError::Cancelled));
            }
            Some(Err(e)) => r.failures.push((f.clone(), e.to_string())),
            None => {}
        }
    }
    r.message = match lines.as_slice() {
        [only] => only.clone(),
        _ => {
            let (label, _) = tool.label();
            format!("{label}: {} of {} videos done", lines.len(), files.len())
        }
    };
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    Ok(r)
}

type Done = (Vec<PathBuf>, String, Option<String>);

fn frame(tk: &Toolkit, f: &Path, v: &Values<'_>, cancel: &CancelToken) -> Result<Done, ToolError> {
    let format = if v.str("format").eq_ignore_ascii_case("jpg") {
        Still::Jpg
    } else {
        Still::Png
    };
    let stem = tk.registry.stem(f);
    let every = v.num("every").unwrap_or(0.0);
    if every > 0.0 {
        let (dir, made, note) = into_folder(f, &format!("{stem} (frames)"), |d| {
            video::frames_every(f, every, d, &stem, format, cancel)
        })?;
        let msg = format!(
            "{} → {}",
            crate::tools::count(made.len(), "picture"),
            name_of(&dir)
        );
        return Ok((vec![dir], msg, note));
    }
    let at = video::parse_time(v.str("at")).map_err(ToolError::Invalid)?;
    let (t, img) = video::frame(f, at, cancel).map_err(engine)?;
    let (out, note) = emit(&parent_of(f), &frame_stem(&stem, t), format.ext(), |tmp| {
        video::save(&img, tmp, format)
    })?;
    let msg = format!("Frame at {} → {}", video::clock(t), name_of(&out));
    Ok((vec![out], msg, note))
}

/// `clip 00m12s`, `talk 1h02m03s`: the time the picture was taken from.
pub(crate) fn frame_stem(stem: &str, t: f64) -> String {
    format!("{stem} {}", video::label(t, t >= 3600.0, false))
}

fn compress(
    tk: &Toolkit,
    f: &Path,
    v: &Values<'_>,
    cancel: &CancelToken,
) -> Result<Done, ToolError> {
    let target = target(v)?;
    let size = std::fs::metadata(f)?.len();
    let label = format!("{} MB", video::mb(target));
    if size <= target {
        return Ok((
            Vec::new(),
            format!("Already under {label} ({} MB)", video::mb(size)),
            None,
        ));
    }
    let stem = tk.registry.stem(f);
    let mut made = None;
    let (out, note) = emit(&parent_of(f), &format!("{stem} ({label})"), "mp4", |tmp| {
        made = Some(video::compress_to(f, tmp, target, cancel)?);
        Ok(())
    })?;
    let Some(c) = made else {
        return Err(ToolError::Invalid("the video wasn't written".into()));
    };
    let mut msg = format!(
        "{} MB → {} MB{}: {}",
        video::mb(size),
        video::mb(c.bytes),
        if c.scaled {
            format!(", {}×{}", c.width, c.height)
        } else {
            String::new()
        },
        name_of(&out)
    );
    if c.over {
        msg.push_str(&format!(
            " (still over {label}; try a smaller size or a shorter clip)"
        ));
    }
    Ok((vec![out], msg, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)], tool: Tool) -> ToolArgs {
        let mut a = ToolArgs::new();
        for (k, val) in pairs {
            a.insert((*k).into(), (*val).into());
        }
        let _ = Values::new(fields(tool), &a).unwrap();
        a
    }

    #[test]
    fn sizes_from_presets_and_typed_values() {
        let a = values(&[], Tool::VideoCompress);
        let v = Values::new(fields(Tool::VideoCompress), &a).unwrap();
        assert_eq!(target(&v).unwrap(), 25_000_000);
        let a = values(&[("size", "8")], Tool::VideoCompress);
        let v = Values::new(fields(Tool::VideoCompress), &a).unwrap();
        assert_eq!(target(&v).unwrap(), 8_000_000);
        let a = values(&[("size", "other"), ("mb", "2,5")], Tool::VideoCompress);
        let v = Values::new(fields(Tool::VideoCompress), &a).unwrap();
        assert_eq!(target(&v).unwrap(), 2_500_000);
        let a = values(&[("size", "other")], Tool::VideoCompress);
        let v = Values::new(fields(Tool::VideoCompress), &a).unwrap();
        assert!(target(&v).is_err(), "Other size needs a number");
        let a = values(&[("mb", "3")], Tool::VideoCompress);
        let v = Values::new(fields(Tool::VideoCompress), &a).unwrap();
        assert_eq!(target(&v).unwrap(), 3_000_000, "a typed size wins");
        let mut bad = ToolArgs::new();
        bad.insert("size".into(), "7".into());
        assert!(Values::new(fields(Tool::VideoCompress), &bad).is_err());
    }

    #[test]
    fn offered_for_videos_when_an_engine_can_run_them() {
        let tk = Toolkit::with_builtin_engines().unwrap();
        let t = crate::tools::tools_for(&tk, &[PathBuf::from("a.mp4"), PathBuf::from("b.mkv")]);
        let can = video::available();
        assert_eq!(t.contains(&Tool::VideoFrame), can);
        assert_eq!(t.contains(&Tool::VideoCompress), can);
        let t = crate::tools::tools_for(&tk, &[PathBuf::from("a.mp4"), PathBuf::from("b.jpg")]);
        assert!(!t.contains(&Tool::VideoFrame), "not for a mix");
        assert!(Tool::VideoCompress.asks() && !Tool::VideoFrame.asks());
        assert_eq!(Tool::from_id("video-frame"), Some(Tool::VideoFrame));
    }

    /// The tools end to end on a real clip (`SWIRVO_TEST_VIDEO`; skipped without it):
    /// names, folders, and nothing replaced on a second run.
    #[test]
    fn real_clip_through_the_tools() {
        let Some(clip) = std::env::var_os("SWIRVO_TEST_VIDEO").map(PathBuf::from) else {
            return;
        };
        let tk = Toolkit::new().unwrap();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("clip.mp4");
        std::fs::copy(&clip, &f).unwrap();
        let before = std::fs::read(&f).unwrap();
        let c = CancelToken::new();
        let run1 = |tool, pairs: &[(&str, &str)]| {
            let mut a = ToolArgs::new();
            for (k, val) in pairs {
                a.insert((*k).into(), (*val).into());
            }
            crate::tools::run_tool_with(&tk, tool, std::slice::from_ref(&f), &a, &c).unwrap()
        };
        let r = run1(Tool::VideoFrame, &[]);
        eprintln!("{}", r.message);
        let name = name_of(&r.outputs[0]);
        assert!(
            name.starts_with("clip 00m0") && name.ends_with("s.png"),
            "{name}"
        );
        let r = run1(Tool::VideoFrame, &[("at", "50 %"), ("format", "jpg")]);
        assert_eq!(name_of(&r.outputs[0]), "clip 00m02s.jpg");
        let again = run1(Tool::VideoFrame, &[("at", "50 %"), ("format", "jpg")]);
        assert_eq!(
            name_of(&again.outputs[0]),
            "clip 00m02s (2).jpg",
            "never replaced"
        );
        let r = run1(Tool::VideoFrame, &[("every", "1")]);
        eprintln!("{}", r.message);
        assert_eq!(name_of(&r.outputs[0]), "clip (frames)");
        assert!(r.outputs[0].join("clip 00m01s.png").is_file());
        let r = run1(Tool::VideoCompress, &[("size", "other"), ("mb", "0.8")]);
        eprintln!("{}", r.message);
        assert_eq!(name_of(&r.outputs[0]), "clip (0.8 MB).mp4");
        assert!(std::fs::metadata(&r.outputs[0]).unwrap().len() <= 800_000);
        assert_eq!(
            std::fs::read(&f).unwrap(),
            before,
            "the original is untouched"
        );
        let leftovers: Vec<String> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(".swirvo-tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn frames_are_named_by_their_time() {
        assert_eq!(frame_stem("clip", 12.4), "clip 00m12s");
        assert_eq!(frame_stem("talk", 3723.0), "talk 1h02m03s");
    }

    #[test]
    fn a_video_already_under_the_target_is_left_alone() {
        let tk = Toolkit::with_builtin_engines().unwrap();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("small.mp4");
        std::fs::write(&f, vec![0u8; 1000]).unwrap();
        let r = run(
            &tk,
            Tool::VideoCompress,
            std::slice::from_ref(&f),
            &ToolArgs::new(),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(r.outputs.is_empty());
        assert!(
            r.message.starts_with("Already under 25 MB"),
            "{}",
            r.message
        );
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            1,
            "nothing written"
        );
    }
}
