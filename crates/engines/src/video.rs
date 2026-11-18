//! Video tools (docs/11 §2, docs/17 §2): a frame as a picture, frames every N seconds, and
//! compress to a target size.
//!
//! Two backends, tried in turn: FFmpeg when it's installed (two-pass rate control, every
//! container), then Windows Media Foundation (always there on Windows, no download). A
//! backend that can't read the file hands over to the next. Everything that decides
//! (times, names, bitrates, sizes) is plain code here and unit-tested on any platform.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use swirvo_core::{CancelToken, EngineError};

/// What a video is, as far as the tools need to know.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    /// Seconds.
    pub duration: f64,
    /// Frame size as the backend decodes it.
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio: bool,
}

/// Which frames to decode.
#[derive(Debug, Clone, PartialEq)]
pub enum Times {
    /// These times (seconds, ascending).
    At(Vec<f64>),
    /// `count` frames from the start, one every `step` seconds.
    Every { step: f64, count: usize },
}

/// How to encode for a target size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SizePlan {
    pub video_bps: u32,
    /// 0 when the video has no sound.
    pub audio_bps: u32,
    pub width: u32,
    pub height: u32,
}

pub(crate) trait Backend {
    fn probe(&self, input: &Path, cancel: &CancelToken) -> Result<Probe, EngineError>;
    /// Decode the frames in `times`, calling `each(index, frame)` in order.
    fn frames(
        &self,
        input: &Path,
        times: &Times,
        cancel: &CancelToken,
        each: &mut dyn FnMut(usize, image::RgbaImage) -> Result<(), EngineError>,
    ) -> Result<(), EngineError>;
    /// H.264 + AAC in MP4, at the plan's bitrates and size.
    fn encode(
        &self,
        input: &Path,
        output: &Path,
        plan: &SizePlan,
        cancel: &CancelToken,
    ) -> Result<(), EngineError>;
    /// Audio bitrates the encoder takes, highest first.
    fn audio_rates(&self) -> &'static [u32];
}

fn ffmpeg() -> Option<&'static PathBuf> {
    static EXE: OnceLock<Option<PathBuf>> = OnceLock::new();
    EXE.get_or_init(crate::external::ffmpeg_exe).as_ref()
}

/// Can the video tools run on this PC? Cheap after the first call: nothing is started.
pub fn available() -> bool {
    cfg!(windows) || ffmpeg().is_some()
}

fn backends() -> Vec<Box<dyn Backend>> {
    let mut v: Vec<Box<dyn Backend>> = Vec::new();
    if let Some(exe) = ffmpeg() {
        v.push(Box::new(Ffmpeg { exe: exe.clone() }));
    }
    #[cfg(windows)]
    v.push(Box::new(crate::os::mediafoundation::MfVideo));
    v
}

/// Run `f` with each backend until one succeeds. Cancelling stops at once; otherwise the
/// first backend's error is the one reported.
fn attempt<T>(
    cancel: &CancelToken,
    mut f: impl FnMut(&dyn Backend) -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    let mut first = None;
    for b in backends() {
        cancel.check()?;
        match f(b.as_ref()) {
            Ok(v) => return Ok(v),
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    Err(first.unwrap_or_else(|| {
        EngineError::Invalid("video tools need Windows' media engine or FFmpeg".into())
    }))
}

// ---------------------------------------------------------------- times and names

/// A time the user typed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeSpec {
    Seconds(f64),
    Percent(f64),
}

/// `""` (pick a good frame), `83`, `83.5`, `1:23`, `1:02:03.5` or `40 %`.
pub fn parse_time(s: &str) -> Result<Option<TimeSpec>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    let bad = || format!("“{s}”: write a time like 1:23 or 83, or a share like 40 %");
    if let Some(p) = s.strip_suffix('%') {
        let v: f64 = p.trim().replace(',', ".").parse().map_err(|_| bad())?;
        return if (0.0..=100.0).contains(&v) {
            Ok(Some(TimeSpec::Percent(v)))
        } else {
            Err(bad())
        };
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return Err(bad());
    }
    let mut t = 0.0;
    for (i, p) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let v: f64 = p.trim().replace(',', ".").parse().map_err(|_| bad())?;
        if v < 0.0 || !v.is_finite() || (!last && v.fract() != 0.0) || (i > 0 && v >= 60.0) {
            return Err(bad());
        }
        t = t * 60.0 + v;
    }
    Ok(Some(TimeSpec::Seconds(t)))
}

/// The latest time that still has a frame.
fn last_time(duration: f64) -> f64 {
    (duration - 0.1).max(0.0)
}

/// A typed time on this video.
pub fn resolve(spec: TimeSpec, duration: f64) -> Result<f64, EngineError> {
    match spec {
        TimeSpec::Percent(p) => Ok((duration * p / 100.0).min(last_time(duration))),
        TimeSpec::Seconds(t) if t <= duration => Ok(t.min(last_time(duration))),
        TimeSpec::Seconds(_) => Err(EngineError::Invalid(format!(
            "the video is only {} long",
            clock(duration)
        ))),
    }
}

/// Where to look for a good frame when no time is given: 10 % in first (past fades and
/// titles), then further in, and only then near the start.
pub(crate) fn auto_candidates(duration: f64) -> Vec<f64> {
    let end = last_time(duration);
    let mut v: Vec<f64> = Vec::new();
    for f in [0.10, 0.25, 0.50, 0.75, 0.04] {
        let t = (duration * f).min(end);
        if !v.iter().any(|&x| (x - t).abs() < 0.05) {
            v.push(t);
        }
    }
    v
}

/// At most this many frames from "one every N seconds".
pub const MAX_FRAMES: usize = 1000;

/// How many frames "one every `step` seconds" gives on this video.
pub fn every_count(duration: f64, step: f64) -> Result<usize, EngineError> {
    if step.is_nan() || step < 0.1 {
        return Err(EngineError::Invalid(
            "pick an interval of at least 0.1 seconds".into(),
        ));
    }
    let n = (last_time(duration) / step + 1e-9).floor() as usize + 1;
    if n > MAX_FRAMES {
        return Err(EngineError::Invalid(format!(
            "that's {n} pictures; pick a longer interval (at most {MAX_FRAMES})"
        )));
    }
    Ok(n)
}

/// `0:05`, `12:40`, `1:02:03` for messages.
pub fn clock(t: f64) -> String {
    let s = t.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A time for a file name (no colons, sorts in order within one video): `00m05s`, with
/// hours (`1h02m03s`) when the video is an hour or longer, tenths when the interval has
/// them (`00m05.5s`).
pub fn label(t: f64, long: bool, tenths: bool) -> String {
    let ds = (t.max(0.0) * 10.0).round() as u64;
    let (whole, tenth) = (ds / 10, ds % 10);
    let (h, m, s) = (whole / 3600, whole / 60 % 60, whole % 60);
    let secs = if tenths {
        format!("{s:02}.{tenth}s")
    } else {
        format!("{s:02}s")
    };
    if long {
        format!("{h}h{m:02}m{secs}")
    } else {
        format!("{:02}m{secs}", whole / 60)
    }
}

/// How much the picture varies (standard deviation of luma, 0–255). Black, white and
/// single-colour frames (fades, title cards) are close to 0.
pub(crate) fn contrast(img: &image::RgbaImage) -> f32 {
    let (w, h) = img.dimensions();
    let step = ((w as u64 * h as u64 / 40_000) as f64).sqrt().max(1.0) as u32;
    let (mut n, mut sum, mut sq) = (0f64, 0f64, 0f64);
    for y in (0..h).step_by(step as usize) {
        for x in (0..w).step_by(step as usize) {
            let p = img.get_pixel(x, y);
            let l = 0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64;
            n += 1.0;
            sum += l;
            sq += l * l;
        }
    }
    if n == 0.0 {
        return 0.0;
    }
    let mean = sum / n;
    ((sq / n - mean * mean).max(0.0)).sqrt() as f32
}

const BLANK: f32 = 8.0;

// ---------------------------------------------------------------- size planning

/// Share of the target the encoders actually aim for: room for the container (about 1 %)
/// and for rate control running over.
const MARGIN: f64 = 0.94;
/// Bits per pixel per frame below which H.264 turns to mush: scale down instead.
const KEEP_BPP: f64 = 0.05;
/// Below this even at the smallest size, the target can't be met watchably.
const FLOOR_BPP: f64 = 0.012;
/// Never smaller than this many lines.
const MIN_HEIGHT: u32 = 144;
const MIN_VIDEO_BPS: f64 = 40_000.0;

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

/// The frame size for `video_bps`: the source size if it gets enough bits per pixel,
/// otherwise scaled down (never up) until it does, but not below [`MIN_HEIGHT`]. None
/// when even that is starved.
fn fit(video_bps: f64, p: &Probe) -> Option<(u32, u32)> {
    let fps = p.fps.clamp(1.0, 120.0);
    let (w, h) = (p.width.max(2) as f64, p.height.max(2) as f64);
    let bpp = |pixels: f64| video_bps / (pixels * fps);
    if bpp(w * h) >= KEEP_BPP {
        return Some((even(w), even(h)));
    }
    let k = (video_bps / (fps * KEEP_BPP) / (w * h)).sqrt();
    let min_k = (MIN_HEIGHT as f64 / h).min(1.0);
    let k = k.max(min_k).min(1.0);
    let (nw, nh) = (w * k, h * k);
    (bpp(nw * nh) >= FLOOR_BPP).then(|| (even(nw), even(nh)))
}

/// The smallest target, in MB (10⁶ bytes), that [`plan_size`] accepts for this video.
pub fn min_target_mb(p: &Probe, audio_rates: &[u32]) -> f64 {
    let fps = p.fps.clamp(1.0, 120.0);
    let h = p.height.max(2) as f64;
    let k = (MIN_HEIGHT as f64 / h).min(1.0);
    let pixels = p.width.max(2) as f64 * k * h * k;
    let video = (pixels * fps * FLOOR_BPP).max(MIN_VIDEO_BPS);
    let audio = if p.audio {
        audio_rates.last().copied().unwrap_or(0) as f64
    } else {
        0.0
    };
    ((video + audio) * p.duration / 8.0 / MARGIN / 1e6 * 10.0).ceil() / 10.0
}

/// Bitrates and size so the whole file lands under `target` bytes: video gets what's left
/// after the sound, the sound gets at most about 10 % of the budget.
pub fn plan_size(target: u64, p: &Probe, audio_rates: &[u32]) -> Result<SizePlan, EngineError> {
    if p.duration <= 0.0 {
        return Err(EngineError::Decode(
            "couldn't tell how long this video is".into(),
        ));
    }
    let total = target as f64 * 8.0 * MARGIN / p.duration;
    let audio_bps = if p.audio {
        audio_rates
            .iter()
            .copied()
            .find(|&r| r as f64 <= total * 0.10)
            .or_else(|| audio_rates.last().copied())
            .unwrap_or(0)
    } else {
        0
    };
    let video = total - audio_bps as f64;
    let too_small = || {
        EngineError::Invalid(format!(
            "{} MB is too small for {} of video; try at least {} MB",
            mb(target),
            clock(p.duration),
            min_target_mb(p, audio_rates)
        ))
    };
    if video < MIN_VIDEO_BPS {
        return Err(too_small());
    }
    let (width, height) = fit(video, p).ok_or_else(too_small)?;
    Ok(SizePlan {
        video_bps: video.min(u32::MAX as f64) as u32,
        audio_bps,
        width,
        height,
    })
}

/// A second plan after the first file came out at `got` bytes, over `target`: video
/// bitrate cut in proportion, plus a little more, and the size refitted.
pub fn retry(plan: &SizePlan, got: u64, target: u64, p: &Probe) -> SizePlan {
    let audio_bits = plan.audio_bps as f64 * p.duration;
    let video_bits = (got as f64 * 8.0 - audio_bits).max(1.0);
    let room = (target as f64 * 8.0 * MARGIN - audio_bits).max(0.0);
    let k = (room / video_bits).clamp(0.2, 0.97);
    let video = (plan.video_bps as f64 * k).max(MIN_VIDEO_BPS);
    let (width, height) = fit(video, p).unwrap_or((plan.width, plan.height));
    SizePlan {
        video_bps: video as u32,
        audio_bps: plan.audio_bps,
        width: width.min(plan.width),
        height: height.min(plan.height),
    }
}

/// `25`, `8.5` for messages and names (decimal megabytes).
pub fn mb(bytes: u64) -> String {
    let v = bytes as f64 / 1e6;
    if (v - v.round()).abs() < 0.05 {
        format!("{}", v.round())
    } else {
        format!("{v:.1}")
    }
}

// ---------------------------------------------------------------- the tools

/// Picture formats for frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Still {
    Png,
    Jpg,
}

impl Still {
    pub fn ext(self) -> &'static str {
        match self {
            Still::Png => "png",
            Still::Jpg => "jpg",
        }
    }
}

pub fn save(img: &image::RgbaImage, out: &Path, format: Still) -> Result<(), EngineError> {
    let file = std::io::BufWriter::new(std::fs::File::create(out)?);
    match format {
        Still::Png => img
            .write_with_encoder(image::codecs::png::PngEncoder::new(file))
            .map_err(EngineError::encode),
        Still::Jpg => image::DynamicImage::ImageRgba8(img.clone())
            .to_rgb8()
            .write_with_encoder(image::codecs::jpeg::JpegEncoder::new_with_quality(file, 92))
            .map_err(EngineError::encode),
    }
}

/// One frame: at `at`, or (None) the first good one of [`auto_candidates`]. Returns its
/// time in seconds and the picture.
pub fn frame(
    input: &Path,
    at: Option<TimeSpec>,
    cancel: &CancelToken,
) -> Result<(f64, image::RgbaImage), EngineError> {
    attempt(cancel, |b| {
        let p = b.probe(input, cancel)?;
        let times = match at {
            Some(spec) => vec![resolve(spec, p.duration)?],
            None => auto_candidates(p.duration),
        };
        let mut best: Option<(f32, f64, image::RgbaImage)> = None;
        for &t in &times {
            let mut got = None;
            b.frames(input, &Times::At(vec![t]), cancel, &mut |_, img| {
                got = Some(img);
                Ok(())
            })?;
            let Some(img) = got else { continue };
            if at.is_some() {
                return Ok((t, img));
            }
            let c = contrast(&img);
            if c >= BLANK {
                return Ok((t, img));
            }
            if best.as_ref().is_none_or(|(bc, _, _)| c > *bc) {
                best = Some((c, t, img));
            }
        }
        best.map(|(_, t, img)| (t, img))
            .ok_or_else(|| EngineError::Decode("no frame could be read from this video".into()))
    })
}

/// One picture every `step` seconds into `dir` (a folder of ours), named
/// `{stem} {label}.{ext}`. Returns the files in time order.
pub fn frames_every(
    input: &Path,
    step: f64,
    dir: &Path,
    stem: &str,
    format: Still,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    attempt(cancel, |b| {
        let p = b.probe(input, cancel)?;
        let count = every_count(p.duration, step)?;
        let long = p.duration >= 3600.0;
        let tenths = (step * 10.0).round() % 10.0 != 0.0;
        let mut written = Vec::with_capacity(count);
        let r = b.frames(
            input,
            &Times::Every { step, count },
            cancel,
            &mut |i, img| {
                let path = dir.join(format!(
                    "{stem} {}.{}",
                    label(i as f64 * step, long, tenths),
                    format.ext()
                ));
                save(&img, &path, format)?;
                written.push(path);
                Ok(())
            },
        );
        if let Err(e) = r {
            // The folder is ours; leave it as it was for the next backend.
            for f in &written {
                let _ = std::fs::remove_file(f);
            }
            return Err(e);
        }
        if written.is_empty() {
            return Err(EngineError::Decode(
                "no frame could be read from this video".into(),
            ));
        }
        Ok(written)
    })
}

/// What [`compress_to`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compressed {
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    /// Encodes it took (1, or 2 when the first came out over the target).
    pub tries: u32,
    /// Still over the target after the second try.
    pub over: bool,
    pub scaled: bool,
}

/// Re-encode `input` as MP4 (H.264 + AAC) into `out`, aiming under `target` bytes.
pub fn compress_to(
    input: &Path,
    out: &Path,
    target: u64,
    cancel: &CancelToken,
) -> Result<Compressed, EngineError> {
    attempt(cancel, |b| {
        let p = b.probe(input, cancel)?;
        let mut plan = plan_size(target, &p, b.audio_rates())?;
        b.encode(input, out, &plan, cancel)?;
        let mut bytes = std::fs::metadata(out)?.len();
        let mut tries = 1;
        if bytes > target {
            plan = retry(&plan, bytes, target, &p);
            b.encode(input, out, &plan, cancel)?;
            bytes = std::fs::metadata(out)?.len();
            tries = 2;
        }
        Ok(Compressed {
            bytes,
            width: plan.width,
            height: plan.height,
            tries,
            over: bytes > target,
            scaled: (plan.width, plan.height) != (even(p.width as f64), even(p.height as f64)),
        })
    })
}

// ---------------------------------------------------------------- FFmpeg

struct Ffmpeg {
    exe: PathBuf,
}

/// Read what `ffmpeg -i` prints about the input: duration, the first video stream's size
/// and frame rate (turned by its rotation, as FFmpeg turns it when encoding), and whether
/// there's sound.
pub(crate) fn parse_ffmpeg_info(text: &str) -> Option<Probe> {
    let dur = text.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("Duration:")?;
        let t = rest.split(',').next()?.trim();
        match parse_time(t) {
            Ok(Some(TimeSpec::Seconds(s))) => Some(s),
            _ => None,
        }
    })?;
    let mut video: Option<(u32, u32, f64)> = None;
    let mut audio = false;
    let mut rotated = false;
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with("Stream #") && l.contains(": Audio:") {
            audio = true;
        }
        if l.starts_with("Stream #") && l.contains(": Video:") && video.is_none() {
            let size = l
                .split([' ', ','])
                .filter_map(|tok| {
                    let (a, b) = tok.split_once('x')?;
                    let (w, h) = (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?);
                    (w >= 2 && h >= 2 && w < 100_000 && h < 100_000).then_some((w, h))
                })
                .next();
            let fps = l
                .split(',')
                .find_map(|part| part.trim().strip_suffix(" fps")?.trim().parse::<f64>().ok())
                .or_else(|| {
                    l.split(',').find_map(|part| {
                        part.trim().strip_suffix(" tbr")?.trim().parse::<f64>().ok()
                    })
                })
                .unwrap_or(30.0);
            if let Some((w, h)) = size {
                video = Some((w, h, fps));
            }
        }
        if video.is_some()
            && let Some(r) = l.split("rotation of ").nth(1)
        {
            let deg: f64 = r
                .split_whitespace()
                .next()
                .and_then(|d| d.parse().ok())
                .unwrap_or(0.0);
            rotated = (deg.abs().round() as i64) % 180 == 90;
        }
    }
    let (w, h, fps) = video?;
    let (width, height) = if rotated { (h, w) } else { (w, h) };
    Some(Probe {
        duration: dur,
        width,
        height,
        fps,
        audio,
    })
}

/// H.264 encoders to try, best first. Only libx264 takes two passes.
const H264: &[&str] = &["libx264", "libopenh264", "h264_mf"];

fn missing_encoder(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("unknown encoder") || e.contains("encoder not found")
}

impl Ffmpeg {
    fn cmd(&self) -> Command {
        let mut c = Command::new(&self.exe);
        c.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]);
        c
    }
}

/// The arguments of one encoding pass (after `-i input`). `pass` is 1 or 2 for libx264's
/// two passes, 0 for a single pass; pass 1 writes no file (`-f null -`).
pub(crate) fn encode_args(
    enc: &str,
    plan: &SizePlan,
    scale: bool,
    pass: u8,
    log: &Path,
    out: &Path,
) -> Vec<std::ffi::OsString> {
    let mut a: Vec<std::ffi::OsString> = Vec::new();
    let mut push = |s: &str| a.push(s.into());
    push("-map");
    push("0:v:0");
    if pass != 1 && plan.audio_bps > 0 {
        push("-map");
        push("0:a:0?");
    }
    if scale {
        push("-vf");
        push(&format!(
            "scale={}:{}:flags=lanczos",
            plan.width, plan.height
        ));
    }
    push("-c:v");
    push(enc);
    if enc == "libx264" {
        push("-preset");
        push("medium");
    }
    push("-b:v");
    push(&plan.video_bps.to_string());
    if pass == 0 {
        // One pass: cap the peaks so the average holds.
        push("-maxrate");
        push(&(plan.video_bps as u64 * 12 / 10).to_string());
        push("-bufsize");
        push(&(plan.video_bps as u64 * 2).to_string());
    }
    push("-pix_fmt");
    push("yuv420p");
    if pass != 0 {
        push("-pass");
        push(&pass.to_string());
        a.push("-passlogfile".into());
        a.push(log.as_os_str().to_owned());
    }
    let mut push = |s: &str| a.push(s.into());
    if pass == 1 || plan.audio_bps == 0 {
        push("-an");
    } else {
        push("-c:a");
        push("aac");
        push("-b:a");
        push(&plan.audio_bps.to_string());
        push("-ac");
        push("2");
    }
    if pass == 1 {
        push("-f");
        push("null");
        push("-");
    } else {
        push("-movflags");
        push("+faststart");
        push("-f");
        push("mp4");
        a.push(out.as_os_str().to_owned());
    }
    a
}

impl Backend for Ffmpeg {
    fn probe(&self, input: &Path, cancel: &CancelToken) -> Result<Probe, EngineError> {
        let mut c = Command::new(&self.exe);
        c.args(["-hide_banner", "-nostdin", "-i"]).arg(input);
        // Without an output FFmpeg exits with an error after describing the input.
        let (_, text) = crate::external::run_capture(c, cancel, "FFmpeg")?;
        parse_ffmpeg_info(&text)
            .ok_or_else(|| EngineError::Decode("FFmpeg found no video in this file".into()))
    }

    fn frames(
        &self,
        input: &Path,
        times: &Times,
        cancel: &CancelToken,
        each: &mut dyn FnMut(usize, image::RgbaImage) -> Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        let tmp = tempfile::tempdir()?;
        let read = |p: &Path| -> Result<image::RgbaImage, EngineError> {
            Ok(image::open(p).map_err(EngineError::decode)?.to_rgba8())
        };
        match times {
            Times::At(list) => {
                let f = tmp.path().join("frame.png");
                for (i, &t) in list.iter().enumerate() {
                    let mut c = self.cmd();
                    c.args(["-ss", &format!("{t:.3}"), "-i"]).arg(input).args([
                        "-frames:v",
                        "1",
                        "-an",
                        "-update",
                        "1",
                        "-f",
                        "image2",
                        "-c:v",
                        "png",
                    ]);
                    c.arg(&f);
                    crate::external::run(c, cancel, "FFmpeg")?;
                    each(i, read(&f)?)?;
                }
            }
            Times::Every { step, count } => {
                let mut c = self.cmd();
                c.arg("-i").arg(input).args([
                    "-vf",
                    &format!("fps={:.6}", 1.0 / step),
                    "-frames:v",
                    &count.to_string(),
                    "-an",
                    "-f",
                    "image2",
                    "-c:v",
                    "png",
                ]);
                c.arg(tmp.path().join("f%06d.png"));
                crate::external::run(c, cancel, "FFmpeg")?;
                for i in 0..*count {
                    cancel.check()?;
                    let f = tmp.path().join(format!("f{:06}.png", i + 1));
                    if !f.is_file() {
                        break;
                    }
                    each(i, read(&f)?)?;
                    let _ = std::fs::remove_file(&f);
                }
            }
        }
        Ok(())
    }

    fn encode(
        &self,
        input: &Path,
        output: &Path,
        plan: &SizePlan,
        cancel: &CancelToken,
    ) -> Result<(), EngineError> {
        let p = self.probe(input, cancel)?;
        let scale = (plan.width, plan.height) != (even(p.width as f64), even(p.height as f64));
        let tmp = tempfile::tempdir()?;
        let log = tmp.path().join("pass");
        let mut first_err = None;
        for &enc in H264 {
            let passes: &[u8] = if enc == "libx264" { &[1, 2] } else { &[0] };
            let mut failed = None;
            for &pass in passes {
                let mut c = self.cmd();
                c.arg("-i")
                    .arg(input)
                    .args(encode_args(enc, plan, scale, pass, &log, output));
                c.current_dir(tmp.path());
                let (ok, err) = crate::external::run_capture(c, cancel, "FFmpeg")?;
                if !ok {
                    failed = Some(err);
                    break;
                }
            }
            match failed {
                None => return Ok(()),
                Some(err) if missing_encoder(&err) => continue,
                Some(err) => {
                    let tail = err.lines().rev().find(|l| !l.trim().is_empty());
                    first_err.get_or_insert(EngineError::Encode(format!(
                        "FFmpeg failed: {}",
                        tail.unwrap_or("no details")
                    )));
                    break;
                }
            }
        }
        Err(first_err
            .unwrap_or_else(|| EngineError::Invalid("this FFmpeg has no H.264 encoder".into())))
    }

    fn audio_rates(&self) -> &'static [u32] {
        &[128_000, 96_000, 64_000, 48_000, 32_000]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(duration: f64, w: u32, h: u32, fps: f64, audio: bool) -> Probe {
        Probe {
            duration,
            width: w,
            height: h,
            fps,
            audio,
        }
    }

    #[test]
    fn reads_typed_times() {
        assert_eq!(parse_time(""), Ok(None));
        assert_eq!(parse_time("83"), Ok(Some(TimeSpec::Seconds(83.0))));
        assert_eq!(parse_time("1:23"), Ok(Some(TimeSpec::Seconds(83.0))));
        assert_eq!(parse_time("1:02:03.5"), Ok(Some(TimeSpec::Seconds(3723.5))));
        assert_eq!(parse_time(" 0:05,5 "), Ok(Some(TimeSpec::Seconds(5.5))));
        assert_eq!(parse_time("40 %"), Ok(Some(TimeSpec::Percent(40.0))));
        for bad in ["1:75", "a", "1:2:3:4", "-3", "150%", "1.5:20"] {
            assert!(parse_time(bad).is_err(), "{bad}");
        }
        assert_eq!(resolve(TimeSpec::Percent(50.0), 10.0).unwrap(), 5.0);
        assert!(resolve(TimeSpec::Seconds(10.0), 10.0).unwrap() < 10.0);
        let e = resolve(TimeSpec::Seconds(61.0), 42.0).unwrap_err();
        assert!(e.to_string().contains("0:42"), "{e}");
    }

    #[test]
    fn picks_frames_past_the_start() {
        let c = auto_candidates(60.0);
        assert!((c[0] - 6.0).abs() < 1e-9, "10 % in first: {c:?}");
        assert!(c.iter().all(|&t| t < 60.0));
        assert!(
            c.last().is_some_and(|&t| (t - 2.4).abs() < 1e-9),
            "near the start only as a last resort: {c:?}"
        );
        // A very short clip still gets valid times, without near-duplicates.
        let c = auto_candidates(0.3);
        assert!(c.iter().all(|&t| t <= 0.2), "{c:?}");
        for (i, a) in c.iter().enumerate() {
            assert!(c[i + 1..].iter().all(|b| (a - b).abs() >= 0.05), "{c:?}");
        }
    }

    #[test]
    fn counts_and_names_frames() {
        assert_eq!(
            every_count(10.0, 5.0).unwrap(),
            2,
            "0 s and 5 s; 10 s is the end"
        );
        assert_eq!(every_count(10.05, 1.0).unwrap(), 10);
        assert_eq!(every_count(3.96, 1.0).unwrap(), 4);
        assert!(every_count(10.0, 0.0).is_err());
        assert!(every_count(10.0, f64::NAN).is_err());
        let e = every_count(3600.0, 1.0).unwrap_err();
        assert!(e.to_string().contains("longer interval"), "{e}");
        assert_eq!(label(5.0, false, false), "00m05s");
        assert_eq!(label(65.0, false, false), "01m05s");
        assert_eq!(label(5.5, false, true), "00m05.5s");
        assert_eq!(label(3723.0, true, false), "1h02m03s");
        assert_eq!(label(83.0, true, false), "0h01m23s");
        // Names sort in time order within one video.
        let mut names: Vec<String> = (0..30)
            .map(|i| label(i as f64 * 7.0, false, false))
            .collect();
        let order = names.clone();
        names.sort();
        assert_eq!(names, order);
        assert_eq!(clock(42.4), "0:42");
        assert_eq!(clock(3723.0), "1:02:03");
    }

    #[test]
    fn blank_frames_have_no_contrast() {
        let black = image::RgbaImage::from_pixel(320, 180, image::Rgba([3, 3, 3, 255]));
        assert!(contrast(&black) < BLANK);
        let white = image::RgbaImage::from_pixel(320, 180, image::Rgba([250, 250, 250, 255]));
        assert!(contrast(&white) < BLANK);
        let scene = image::RgbaImage::from_fn(320, 180, |x, y| {
            image::Rgba([(x % 256) as u8, (y * 2 % 256) as u8, 90, 255])
        });
        assert!(contrast(&scene) > 30.0);
    }

    #[test]
    fn plans_fit_the_target() {
        let rates: &[u32] = &[192_000, 160_000, 128_000, 96_000];
        // 2 minutes of 1080p30 into 25 MB: about 1.6 Mbit/s in all, which needs a smaller
        // picture to stay watchable.
        let p = probe(120.0, 1920, 1080, 30.0, true);
        let plan = plan_size(25_000_000, &p, rates).unwrap();
        let bits = (plan.video_bps + plan.audio_bps) as f64 * p.duration;
        assert!(bits / 8.0 <= 25_000_000.0 * MARGIN + 1.0, "{plan:?}");
        assert!(
            bits / 8.0 >= 25_000_000.0 * 0.9,
            "uses the budget: {plan:?}"
        );
        assert_eq!(plan.audio_bps, 128_000, "sound gets at most ~10 %");
        assert!(plan.height < 1080 && plan.height >= MIN_HEIGHT, "{plan:?}");
        assert_eq!((plan.width % 2, plan.height % 2), (0, 0));
        let ratio = plan.width as f64 / plan.height as f64;
        assert!(
            (ratio - 16.0 / 9.0).abs() < 0.02,
            "keeps the shape: {plan:?}"
        );
        let bpp = plan.video_bps as f64 / (plan.width as f64 * plan.height as f64 * 30.0);
        assert!(bpp >= KEEP_BPP * 0.98, "{bpp}");

        // A short clip with a big budget keeps its size and the best sound.
        let p = probe(10.0, 1280, 720, 30.0, true);
        let plan = plan_size(25_000_000, &p, rates).unwrap();
        assert_eq!(
            (plan.width, plan.height, plan.audio_bps),
            (1280, 720, 192_000)
        );

        // No sound: everything goes to the picture.
        let p = probe(60.0, 1280, 720, 30.0, false);
        let plan = plan_size(10_000_000, &p, rates).unwrap();
        assert_eq!(plan.audio_bps, 0);
        assert!((plan.video_bps as f64 - 10e6 * 8.0 * MARGIN / 60.0).abs() < 2.0);

        // An hour into 8 MB can't work: say what would.
        let p = probe(3600.0, 1920, 1080, 30.0, true);
        let e = plan_size(8_000_000, &p, rates).unwrap_err().to_string();
        assert!(e.contains("too small") && e.contains("1:00:00"), "{e}");
        let min = min_target_mb(&p, rates);
        let ok = plan_size((min * 1e6) as u64 + 1, &p, rates);
        assert!(ok.is_ok(), "the suggested minimum works: {min} MB, {ok:?}");
    }

    #[test]
    fn a_retry_aims_lower() {
        let p = probe(60.0, 1280, 720, 30.0, true);
        let plan = plan_size(10_000_000, &p, &[128_000, 96_000]).unwrap();
        let again = retry(&plan, 11_000_000, 10_000_000, &p);
        assert!(again.video_bps < plan.video_bps);
        // The new video share is what fits: (target·margin − audio) over what was spent.
        let audio = plan.audio_bps as f64 * 60.0;
        let want = (10e6 * 8.0 * MARGIN - audio) / (11e6 * 8.0 - audio) * plan.video_bps as f64;
        assert!((again.video_bps as f64 - want).abs() < 2.0, "{again:?}");
        assert!(again.width <= plan.width && again.height <= plan.height);
        assert_eq!(mb(25_000_000), "25");
        assert_eq!(mb(8_450_000), "8.4");
    }

    /// End to end with a real clip and whatever backend this PC has. Runs only when
    /// `SWIRVO_TEST_VIDEO` names a video (CI has none; run it on Windows by hand).
    #[test]
    fn a_real_clip_gives_frames_and_a_smaller_file() {
        let Some(clip) = std::env::var_os("SWIRVO_TEST_VIDEO").map(PathBuf::from) else {
            return;
        };
        let cancel = CancelToken::new();
        let p = attempt(&cancel, |b| b.probe(&clip, &cancel)).unwrap();
        eprintln!("probe: {p:?}");
        assert!(p.duration > 0.5 && p.width > 0 && p.height > 0);

        let (t, img) = frame(&clip, None, &cancel).unwrap();
        eprintln!("auto frame at {t:.2} s, contrast {:.1}", contrast(&img));
        assert!(contrast(&img) >= BLANK, "not a blank frame");
        assert_eq!(img.dimensions(), (p.width, p.height));
        let (t2, _) = frame(&clip, Some(TimeSpec::Percent(50.0)), &cancel).unwrap();
        assert!((t2 - p.duration / 2.0).abs() < 0.01);
        assert!(frame(&clip, Some(TimeSpec::Seconds(p.duration + 5.0)), &cancel).is_err());

        let d = tempfile::tempdir().unwrap();
        let files = frames_every(&clip, 1.0, d.path(), "clip", Still::Jpg, &cancel).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        eprintln!("every 1 s: {names:?}");
        assert_eq!(files.len(), every_count(p.duration, 1.0).unwrap());
        assert_eq!(names[0], "clip 00m00s.jpg");
        let a = image::open(&files[0]).unwrap().to_rgba8();
        let b = image::open(files.last().unwrap()).unwrap().to_rgba8();
        assert_ne!(a.as_raw(), b.as_raw(), "different moments");

        let size = std::fs::metadata(&clip).unwrap().len();
        let target = size * 2 / 5;
        let out = d.path().join("small.mp4");
        let started = std::time::Instant::now();
        let c = compress_to(&clip, &out, target, &cancel).unwrap();
        eprintln!(
            "{size} bytes → {} (target {target}) in {:?}: {c:?}",
            c.bytes,
            started.elapsed()
        );
        assert!(
            !c.over && c.bytes <= target && c.bytes > target / 4,
            "{c:?}"
        );
        let q = attempt(&cancel, |b| b.probe(&out, &cancel)).unwrap();
        assert!((q.duration - p.duration).abs() < 0.3, "same length: {q:?}");
        assert_eq!(q.audio, p.audio);
    }

    #[test]
    fn reads_ffmpeg_stream_info() {
        let text = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'clip.mp4':
  Metadata:
    major_brand     : isom
  Duration: 00:01:03.96, start: 0.000000, bitrate: 3266 kb/s
  Stream #0:0[0x1](und): Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1920x1080 [SAR 1:1 DAR 16:9], 3130 kb/s, 29.97 fps, 29.97 tbr, 30k tbn (default)
      Side data:
        displaymatrix: rotation of -90.00 degrees
  Stream #0:1[0x2](und): Audio: aac (LC) (mp4a / 0x6134706d), 48000 Hz, stereo, fltp, 128 kb/s (default)
At least one output file must be specified";
        let p = parse_ffmpeg_info(text).unwrap();
        assert!((p.duration - 63.96).abs() < 1e-9);
        assert_eq!(
            (p.width, p.height),
            (1080, 1920),
            "turned as FFmpeg turns it"
        );
        assert!((p.fps - 29.97).abs() < 1e-9);
        assert!(p.audio);
        let silent = "  Duration: 00:00:04.00, start: 0.000000, bitrate: 100 kb/s
  Stream #0:0: Video: vp9 (Profile 0), yuv420p(tv), 640x360, SAR 1:1 DAR 16:9, 25 tbr, 1k tbn";
        let p = parse_ffmpeg_info(silent).unwrap();
        assert_eq!((p.width, p.height, p.audio), (640, 360, false));
        assert_eq!(p.fps, 25.0);
        assert!(parse_ffmpeg_info("tone.mp3: Duration: N/A").is_none());
    }

    #[test]
    fn ffmpeg_passes_are_argument_vectors() {
        let plan = SizePlan {
            video_bps: 1_500_000,
            audio_bps: 128_000,
            width: 1280,
            height: 720,
        };
        let s = |v: Vec<std::ffi::OsString>| {
            v.iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        };
        let log = Path::new("/t/pass");
        let out = Path::new("/o/a b.mp4");
        let one = s(encode_args("libx264", &plan, true, 1, log, out));
        assert!(
            one.contains("-pass 1") && one.ends_with("-an -f null -"),
            "{one}"
        );
        assert!(!one.contains("0:a"), "pass 1 reads no sound: {one}");
        assert!(one.contains("scale=1280:720"));
        let two = encode_args("libx264", &plan, false, 2, log, out);
        assert_eq!(
            two.last().map(|a| a.as_os_str()),
            Some(out.as_os_str()),
            "the path is one argument"
        );
        let two = s(two);
        assert!(
            two.contains("-map 0:a:0? ") && two.contains("-b:a 128000"),
            "{two}"
        );
        assert!(!two.contains("scale="));
        let single = s(encode_args("h264_mf", &plan, false, 0, log, out));
        assert!(
            single.contains("-maxrate 1800000") && !single.contains("-pass"),
            "{single}"
        );
        assert!(missing_encoder("Unknown encoder 'libx264'"));
        assert!(!missing_encoder("Invalid data found when processing input"));
    }
}
