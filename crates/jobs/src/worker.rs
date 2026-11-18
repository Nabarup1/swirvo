//! The worker process (docs/03 §2, PROJECT_GUIDE.md rule 3): conversions and tools run in a
//! separate, restricted child process, so a malformed file that crashes or hangs a decoder
//! takes down that child, not the app.
//!
//! **Protocol.** The parent starts `<exe> --worker`, writes one request as a JSON line on
//! its stdin, and reads JSON lines back from its stdout: `started` when a job begins, each
//! job's event as it finishes, then one final reply. A later `cancel` line, or stdin
//! closing (the parent went away), cancels the run. One worker per run: starting one
//! (engine discovery included) costs about 0.1 s, and every run gets a fresh process.
//!
//! **Crashes.** When the worker's output ends without a final reply, the jobs it had
//! started fail with "the converter stopped unexpectedly on this file", and the jobs it
//! hadn't reached run again in a fresh worker (up to [`MAX_RESTARTS`] times), so one bad
//! file costs only itself.
//!
//! **Sandbox (Windows).** The worker runs in a Job Object that kills it with the app
//! (`KILL_ON_JOB_CLOSE`), caps each process's memory (the lesser of 4 GB and ¾ of RAM)
//! and suppresses crash dialogs, at below-normal priority, with no console window. At
//! start it turns on process mitigations: no DLLs from network shares or low-integrity
//! files, System32 first, no legacy extension points (AppInit DLLs, global hooks), and
//! strict handle checks. LibreOffice, FFmpeg and Office run as its children and inherit
//! the job (and its memory cap), not the mitigations.
//!
//! **Not (yet) isolated:** the network and the file system. The worker runs as the user,
//! so it *could* open other files or connect out; our engines never do. Closing that
//! needs an AppContainer with brokered file handles (docs/03 §9, level L2).
//!
//! **Fallback.** Without a worker executable (tests, the CLI) everything runs in-process,
//! exactly as before. Hosts call [`set_worker`] at start-up.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use swirvo_core::{CancelToken, FormatId, PdfOptions, Preset};
use swirvo_engines::pdfedit::annotate::Comment;
use swirvo_engines::pdfedit::assemble::PageSrc;
use swirvo_engines::pdfedit::organise::{Mark, Sheet};

use crate::args::ToolArgs;
use crate::tools::{PageOp, Tool, ToolError, ToolResult};
use crate::{JobEvent, Options, Summary, Toolkit};

/// The argument that makes the app's executable serve one request as a worker.
pub const WORKER_ARG: &str = "--worker";
/// Fresh workers started after crashes in one run, for the jobs not yet reached.
pub const MAX_RESTARTS: usize = 3;
/// After a cancel, how long the worker gets to stop by itself before it is killed.
const CANCEL_GRACE: Duration = Duration::from_secs(3);
/// What a job that was running when its worker died fails with.
pub const CRASHED: &str = "the converter stopped unexpectedly on this file";

/// The worker executable and its arguments (None: run in-process).
static WORKER: RwLock<Option<(PathBuf, Vec<OsString>)>> = RwLock::new(None);

/// Run conversions and tools in `<exe> --worker` from now on (None: in-process).
pub fn set_worker(exe: Option<PathBuf>) {
    let v = exe.map(|e| (e, vec![OsString::from(WORKER_ARG)]));
    *WORKER.write().unwrap_or_else(|p| p.into_inner()) = v;
}

/// The worker executable in use, if any.
pub fn worker_exe() -> Option<PathBuf> {
    WORKER
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .map(|w| w.0.clone())
}

// ---------------------------------------------------------------------------------------
// Wire types. Kept apart from the engine types so those need no serde.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct WireOptions {
    preset: String,
    archival: bool,
    tagged: bool,
    attach_source: bool,
    accessible: bool,
    out_dir: Option<PathBuf>,
    jobs: usize,
}

impl WireOptions {
    fn from(o: &Options) -> WireOptions {
        WireOptions {
            preset: match o.preset {
                Preset::Fastest => "fastest",
                Preset::Balanced => "balanced",
                Preset::Best => "best",
                Preset::Smallest => "smallest",
            }
            .into(),
            archival: o.pdf.archival,
            tagged: o.pdf.tagged,
            attach_source: o.pdf.attach_source,
            accessible: o.pdf.accessible,
            out_dir: o.out_dir.clone(),
            jobs: o.jobs,
        }
    }

    fn options(&self) -> Options {
        Options {
            preset: match self.preset.as_str() {
                "fastest" => Preset::Fastest,
                "best" => Preset::Best,
                "smallest" => Preset::Smallest,
                _ => Preset::Balanced,
            },
            pdf: PdfOptions {
                archival: self.archival,
                tagged: self.tagged,
                attach_source: self.attach_source,
                accessible: self.accessible,
            },
            out_dir: self.out_dir.clone(),
            jobs: self.jobs.max(1),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireSrc {
    Page { file: usize, page: usize },
    Blank { w: f32, h: f32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct WireSheet {
    src: WireSrc,
    turn: i64,
    crop: Option<[f32; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireComment {
    Highlight {
        rect: [f32; 4],
        color: [u8; 3],
    },
    Note {
        at: [f32; 2],
        text: String,
        color: [u8; 3],
    },
    Ink {
        strokes: Vec<Vec<[f32; 2]>>,
        color: [u8; 3],
        width: f32,
    },
    Box {
        rect: [f32; 4],
        color: [u8; 3],
        width: f32,
    },
    Text {
        rect: [f32; 4],
        text: String,
        size: f32,
        color: [u8; 3],
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireMark {
    Redact {
        page: usize,
        rect: [f32; 4],
    },
    Picture {
        page: usize,
        rect: [f32; 4],
        image: Vec<u8>,
    },
    Comment {
        page: usize,
        comment: WireComment,
    },
}

impl WireSheet {
    fn from(s: &Sheet) -> WireSheet {
        WireSheet {
            src: match s.src {
                PageSrc::Page { file, page } => WireSrc::Page { file, page },
                PageSrc::Blank { w, h } => WireSrc::Blank { w, h },
            },
            turn: s.turn,
            crop: s.crop,
        }
    }

    fn sheet(&self) -> Sheet {
        Sheet {
            src: match self.src {
                WireSrc::Page { file, page } => PageSrc::Page { file, page },
                WireSrc::Blank { w, h } => PageSrc::Blank { w, h },
            },
            turn: self.turn,
            crop: self.crop,
        }
    }
}

impl WireComment {
    fn from(c: &Comment) -> WireComment {
        match c.clone() {
            Comment::Highlight { rect, color } => WireComment::Highlight { rect, color },
            Comment::Note { at, text, color } => WireComment::Note { at, text, color },
            Comment::Ink {
                strokes,
                color,
                width,
            } => WireComment::Ink {
                strokes,
                color,
                width,
            },
            Comment::Box { rect, color, width } => WireComment::Box { rect, color, width },
            Comment::Text {
                rect,
                text,
                size,
                color,
            } => WireComment::Text {
                rect,
                text,
                size,
                color,
            },
        }
    }

    fn comment(self) -> Comment {
        match self {
            WireComment::Highlight { rect, color } => Comment::Highlight { rect, color },
            WireComment::Note { at, text, color } => Comment::Note { at, text, color },
            WireComment::Ink {
                strokes,
                color,
                width,
            } => Comment::Ink {
                strokes,
                color,
                width,
            },
            WireComment::Box { rect, color, width } => Comment::Box { rect, color, width },
            WireComment::Text {
                rect,
                text,
                size,
                color,
            } => Comment::Text {
                rect,
                text,
                size,
                color,
            },
        }
    }
}

impl WireMark {
    fn from(m: &Mark) -> WireMark {
        match m {
            Mark::Redact { page, rect } => WireMark::Redact {
                page: *page,
                rect: *rect,
            },
            Mark::Picture { page, rect, image } => WireMark::Picture {
                page: *page,
                rect: *rect,
                image: image.as_ref().clone(),
            },
            Mark::Comment { page, comment } => WireMark::Comment {
                page: *page,
                comment: WireComment::from(comment),
            },
        }
    }

    fn mark(self) -> Mark {
        match self {
            WireMark::Redact { page, rect } => Mark::Redact { page, rect },
            WireMark::Picture { page, rect, image } => Mark::Picture {
                page,
                rect,
                image: Arc::new(image),
            },
            WireMark::Comment { page, comment } => Mark::Comment {
                page,
                comment: comment.comment(),
            },
        }
    }
}

/// What the parent asks for (one per worker, plus `Cancel`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "req", rename_all = "snake_case")]
enum Request {
    /// Each input with its target format's key ("pdf", "jpg").
    Convert {
        jobs: Vec<(PathBuf, String)>,
        options: WireOptions,
    },
    Tool {
        tool: String,
        files: Vec<PathBuf>,
        args: ToolArgs,
    },
    PageOp {
        file: PathBuf,
        op: PageOp,
        pages: Vec<usize>,
    },
    Organise {
        inputs: Vec<PathBuf>,
        sheets: Vec<WireSheet>,
        marks: Vec<WireMark>,
    },
    Cancel,
}

/// What the worker says back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
enum Reply {
    /// Job `index` began (so a crash can be pinned on it).
    Started {
        index: usize,
    },
    Job {
        event: JobEvent,
    },
    Converted {
        summary: Summary,
    },
    /// A tool, page operation or organise run finished (Err: the tool's message).
    Finished {
        result: Result<ToolResult, String>,
    },
    /// The request couldn't run at all.
    Error {
        message: String,
    },
}

// ---------------------------------------------------------------------------------------
// The worker side.

/// Serve one request from stdin on stdout, then return the process exit code. Called by
/// the app's executable when started with [`WORKER_ARG`].
pub fn serve() -> i32 {
    sandbox::enter();
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let first = match lines.next() {
        Some(Ok(l)) => l,
        _ => return 2,
    };
    let out = Mutex::new(std::io::stdout());
    let send = |r: &Reply| {
        if let Ok(line) = serde_json::to_string(r) {
            let mut o = out.lock().unwrap_or_else(|p| p.into_inner());
            let _ = writeln!(o, "{line}");
            let _ = o.flush();
        }
    };
    let req: Request = match serde_json::from_str(&first) {
        Ok(r) => r,
        Err(e) => {
            send(&Reply::Error {
                message: format!("bad request: {e}"),
            });
            return 2;
        }
    };
    // Cancel on a `cancel` line, or when the parent closes our stdin (it went away).
    let cancel = Arc::new(CancelToken::new());
    let c = cancel.clone();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if matches!(serde_json::from_str(&l), Ok(Request::Cancel)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        c.cancel();
    });
    drop(lines);
    let tk = match Toolkit::new() {
        Ok(t) => t,
        Err(e) => {
            send(&Reply::Error {
                message: format!("couldn't start the converter: {e}"),
            });
            return 1;
        }
    };
    run_request(&tk, req, &cancel, &send);
    0
}

fn run_request(tk: &Toolkit, req: Request, cancel: &CancelToken, send: &(dyn Fn(&Reply) + Sync)) {
    let finished = |r: Result<ToolResult, ToolError>| {
        send(&Reply::Finished {
            result: r.map_err(|e| e.to_string()),
        })
    };
    match req {
        Request::Convert { jobs, options } => {
            let mut resolved = Vec::with_capacity(jobs.len());
            for (path, key) in jobs {
                match tk.registry.id(&key) {
                    Some(id) => resolved.push((path, id)),
                    None => {
                        send(&Reply::Error {
                            message: format!("unknown format “{key}”"),
                        });
                        return;
                    }
                }
            }
            let crash_on = crash_hook();
            let summary = crate::convert_each_observed(
                tk,
                &resolved,
                &options.options(),
                cancel,
                &|i| {
                    send(&Reply::Started { index: i });
                    if let Some(name) = &crash_on
                        && resolved[i].0.to_string_lossy().contains(name.as_str())
                    {
                        std::process::abort();
                    }
                },
                &|ev| send(&Reply::Job { event: ev.clone() }),
            );
            send(&Reply::Converted { summary });
        }
        Request::Tool { tool, files, args } => match Tool::from_id(&tool) {
            Some(t) => {
                if let Some(name) = crash_hook()
                    && files
                        .iter()
                        .any(|f| f.to_string_lossy().contains(name.as_str()))
                {
                    std::process::abort();
                }
                finished(crate::tools::run_tool_with(tk, t, &files, &args, cancel))
            }
            None => send(&Reply::Error {
                message: format!("unknown tool “{tool}”"),
            }),
        },
        Request::PageOp { file, op, pages } => {
            finished(crate::tools::run_page_op(tk, &file, op, &pages, cancel))
        }
        Request::Organise {
            inputs,
            sheets,
            marks,
        } => {
            let sheets: Vec<Sheet> = sheets.iter().map(WireSheet::sheet).collect();
            let marks: Vec<Mark> = marks.into_iter().map(WireMark::mark).collect();
            finished(crate::organise::run_organise(
                tk, &inputs, &sheets, &marks, cancel,
            ))
        }
        Request::Cancel => {}
    }
}

/// Test builds only: `SWIRVO_WORKER_CRASH_ON=<part of a file name>` makes the worker
/// abort when it reaches that file, to exercise crash recovery.
fn crash_hook() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var("SWIRVO_WORKER_CRASH_ON")
            .ok()
            .filter(|s| !s.is_empty())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------------------
// The parent side.

/// A running worker's process and stdin (for `cancel`); its replies come from [`Replies`].
struct Control {
    child: Mutex<Child>,
    stdin: Mutex<Option<ChildStdin>>,
    done: AtomicBool,
}

/// A worker's stdout as replies, until its output ends.
struct Replies(std::io::Lines<BufReader<std::process::ChildStdout>>);

impl Iterator for Replies {
    type Item = Reply;

    fn next(&mut self) -> Option<Reply> {
        loop {
            match self.0.next()? {
                Ok(l) => {
                    if let Ok(r) = serde_json::from_str(&l) {
                        return Some(r);
                    }
                }
                Err(_) => return None,
            }
        }
    }
}

fn start(exe: &Path, args: &[OsString], req: &Request) -> std::io::Result<(Control, Replies)> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    sandbox::configure(&mut cmd);
    let mut child = cmd.spawn()?;
    sandbox::contain(&child);
    let mut stdin = child.stdin.take();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("the worker has no output"))?;
    if let Some(i) = stdin.as_mut() {
        let line = serde_json::to_string(req).map_err(std::io::Error::other)?;
        writeln!(i, "{line}")?;
        i.flush()?;
    }
    Ok((
        Control {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            done: AtomicBool::new(false),
        },
        Replies(BufReader::new(stdout).lines()),
    ))
}

impl Control {
    /// Until the run is over: on cancel, ask the worker to stop, and kill it after the
    /// grace period.
    fn watch(&self, cancel: &CancelToken) {
        let mut asked: Option<Instant> = None;
        while !self.done.load(Ordering::Relaxed) {
            if cancel.is_cancelled() {
                match asked {
                    None => {
                        if let Some(i) = self
                            .stdin
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .as_mut()
                        {
                            let _ = writeln!(i, "{{\"req\":\"cancel\"}}");
                            let _ = i.flush();
                        }
                        asked = Some(Instant::now());
                    }
                    Some(t) if t.elapsed() > CANCEL_GRACE => {
                        let _ = self.child.lock().unwrap_or_else(|p| p.into_inner()).kill();
                        return;
                    }
                    Some(_) => {}
                }
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }

    /// Wait for the worker to exit; a description of an abnormal exit.
    fn finish(self) -> Option<String> {
        drop(self.stdin.into_inner().unwrap_or_else(|p| p.into_inner()));
        let mut child = self.child.into_inner().unwrap_or_else(|p| p.into_inner());
        match child.wait() {
            Ok(s) if s.success() => None,
            Ok(s) => Some(match s.code() {
                Some(c) => format!("exit code {:#x}", c as u32),
                None => "killed".into(),
            }),
            Err(e) => Some(e.to_string()),
        }
    }
}

/// Run `req` in a worker, handing each reply to `on_reply`; then the abnormal exit, if
/// any. Cancelling `cancel` stops the worker.
fn exchange(
    exe: &Path,
    args: &[OsString],
    req: &Request,
    cancel: &CancelToken,
    on_reply: &mut dyn FnMut(Reply),
) -> std::io::Result<Option<String>> {
    let (ctl, replies) = start(exe, args, req)?;
    std::thread::scope(|s| {
        s.spawn(|| ctl.watch(cancel));
        for r in replies {
            on_reply(r);
        }
        ctl.done.store(true, Ordering::Relaxed);
    });
    Ok(ctl.finish())
}

fn configured() -> Option<(PathBuf, Vec<OsString>)> {
    WORKER.read().unwrap_or_else(|p| p.into_inner()).clone()
}

fn reindex(ev: &JobEvent, to: usize) -> JobEvent {
    let mut e = ev.clone();
    match &mut e {
        JobEvent::Done { index, .. }
        | JobEvent::Failed { index, .. }
        | JobEvent::Skipped { index, .. } => *index = to,
    }
    e
}

fn index_of(ev: &JobEvent) -> usize {
    match ev {
        JobEvent::Done { index, .. }
        | JobEvent::Failed { index, .. }
        | JobEvent::Skipped { index, .. } => *index,
    }
}

fn tally(s: &mut Summary, ev: &JobEvent) {
    match ev {
        JobEvent::Done {
            bytes_in,
            bytes_out,
            ..
        } => {
            s.done += 1;
            s.bytes_in += bytes_in;
            s.bytes_out += bytes_out;
        }
        JobEvent::Failed { .. } => s.failed += 1,
        JobEvent::Skipped { .. } => s.skipped += 1,
    }
}

/// [`crate::convert_each`] in a worker when one is configured, else in-process. Events
/// carry indices into `jobs`, whichever worker ran them.
pub fn convert_each(
    tk: &Toolkit,
    jobs: &[(PathBuf, FormatId)],
    opts: &Options,
    cancel: &CancelToken,
    on_event: &(dyn Fn(&JobEvent) + Sync),
) -> Summary {
    let Some((exe, args)) = configured() else {
        return crate::convert_each(tk, jobs, opts, cancel, on_event);
    };
    let mut summary = Summary::default();
    let mut emit = |ev: JobEvent| {
        tally(&mut summary, &ev);
        on_event(&ev);
    };
    // Jobs (indices into `jobs`) the next worker gets.
    let mut pending: Vec<usize> = (0..jobs.len()).collect();
    let mut restarts = 0;
    while !pending.is_empty() {
        if cancel.is_cancelled() {
            for &i in &pending {
                emit(JobEvent::Skipped {
                    index: i,
                    input: jobs[i].0.clone(),
                    reason: "cancelled".into(),
                });
            }
            break;
        }
        let req = Request::Convert {
            jobs: pending
                .iter()
                .map(|&i| (jobs[i].0.clone(), tk.registry.key(jobs[i].1).to_string()))
                .collect(),
            options: WireOptions::from(opts),
        };
        let mut started = HashSet::new();
        let mut finished = HashSet::new();
        let mut complete = false;
        let mut refused = None;
        let exit = match exchange(&exe, &args, &req, cancel, &mut |reply| match reply {
            Reply::Started { index } => {
                started.insert(index);
            }
            Reply::Job { event } => {
                let local = index_of(&event);
                if let Some(&orig) = pending.get(local) {
                    finished.insert(local);
                    emit(reindex(&event, orig));
                }
            }
            Reply::Converted { .. } => complete = true,
            Reply::Error { message } => refused = Some(message),
            Reply::Finished { .. } => {}
        }) {
            Ok(x) => x,
            Err(e) => {
                for &i in &pending {
                    emit(JobEvent::Failed {
                        index: i,
                        input: jobs[i].0.clone(),
                        error: format!("couldn't start the converter: {e}"),
                    });
                }
                break;
            }
        };
        if complete {
            break;
        }
        if let Some(m) = refused {
            for (local, &i) in pending.iter().enumerate() {
                if !finished.contains(&local) {
                    emit(JobEvent::Failed {
                        index: i,
                        input: jobs[i].0.clone(),
                        error: m.clone(),
                    });
                }
            }
            break;
        }
        // The worker died: the files it was on fail; the ones it hadn't reached go again.
        let mut next = Vec::new();
        for (local, &i) in pending.iter().enumerate() {
            if finished.contains(&local) {
                continue;
            }
            if started.contains(&local) || restarts >= MAX_RESTARTS {
                let why = if started.contains(&local) {
                    CRASHED.to_string()
                } else {
                    "the converter kept stopping; try fewer files at once".to_string()
                };
                emit(JobEvent::Failed {
                    index: i,
                    input: jobs[i].0.clone(),
                    error: match (&exit, cancel.is_cancelled()) {
                        (_, true) => "cancelled".into(),
                        (Some(x), false) if started.contains(&local) => format!("{why} ({x})"),
                        _ => why,
                    },
                });
            } else {
                next.push(i);
            }
        }
        pending = next;
        restarts += 1;
    }
    summary
}

/// Send a tool-like request to a worker; None when no worker is configured.
fn remote(req: Request, cancel: &CancelToken) -> Option<Result<ToolResult, ToolError>> {
    let (exe, args) = configured()?;
    let mut result = None;
    let exit = exchange(&exe, &args, &req, cancel, &mut |reply| match reply {
        Reply::Finished { result: r } => result = Some(r.map_err(ToolError::Invalid)),
        Reply::Error { message } => result = Some(Err(ToolError::Invalid(message))),
        _ => {}
    });
    let exit = match exit {
        Ok(x) => x,
        Err(e) => {
            return Some(Err(ToolError::Invalid(format!(
                "couldn't start the converter: {e}"
            ))));
        }
    };
    Some(result.unwrap_or_else(|| {
        Err(ToolError::Invalid(if cancel.is_cancelled() {
            "cancelled".into()
        } else {
            format!(
                "the tool stopped unexpectedly ({})",
                exit.unwrap_or_else(|| "no reply".into())
            )
        }))
    }))
}

/// [`crate::tools::run_tool_with`] in a worker when one is configured.
pub fn run_tool_with(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    args: &ToolArgs,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let req = Request::Tool {
        tool: tool.id().to_string(),
        files: files.to_vec(),
        args: args.clone(),
    };
    remote(req, cancel)
        .unwrap_or_else(|| crate::tools::run_tool_with(tk, tool, files, args, cancel))
}

/// [`crate::tools::run_page_op`] in a worker when one is configured.
pub fn run_page_op(
    tk: &Toolkit,
    file: &Path,
    op: PageOp,
    pages: &[usize],
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let req = Request::PageOp {
        file: file.to_path_buf(),
        op,
        pages: pages.to_vec(),
    };
    remote(req, cancel).unwrap_or_else(|| crate::tools::run_page_op(tk, file, op, pages, cancel))
}

/// [`crate::organise::run_organise`] in a worker when one is configured.
pub fn run_organise(
    tk: &Toolkit,
    inputs: &[PathBuf],
    sheets: &[Sheet],
    marks: &[Mark],
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let req = Request::Organise {
        inputs: inputs.to_vec(),
        sheets: sheets.iter().map(WireSheet::from).collect(),
        marks: marks.iter().map(WireMark::from).collect(),
    };
    remote(req, cancel)
        .unwrap_or_else(|| crate::organise::run_organise(tk, inputs, sheets, marks, cancel))
}

// ---------------------------------------------------------------------------------------
// Sandbox.

#[cfg(windows)]
mod sandbox {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::OnceLock;

    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::SystemServices::{
        PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY, PROCESS_MITIGATION_IMAGE_LOAD_POLICY,
        PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY,
    };
    use windows::Win32::System::Threading::{
        ProcessExtensionPointDisablePolicy, ProcessImageLoadPolicy, ProcessStrictHandleCheckPolicy,
        SetProcessMitigationPolicy,
    };

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

    /// Raw job handle (kept open for the app's life: closing it kills the workers).
    static JOB: OnceLock<Option<isize>> = OnceLock::new();

    fn memory_cap() -> usize {
        let mut m = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: the struct is sized as the API requires.
        let total = unsafe { GlobalMemoryStatusEx(&mut m) }.map_or(8 << 30, |_| m.ullTotalPhys);
        (total / 4 * 3).min(4 << 30) as usize
    }

    fn job() -> Option<HANDLE> {
        let raw = JOB.get_or_init(|| {
            // SAFETY: an unnamed job object, configured once with a correctly sized struct;
            // the handle is deliberately never closed (it lives as long as the app).
            unsafe {
                let job = CreateJobObjectW(None, None).ok()?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
                    | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = memory_cap();
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .ok()?;
                Some(job.0 as isize)
            }
        });
        raw.map(|h| HANDLE(h as *mut core::ffi::c_void))
    }

    pub fn configure(cmd: &mut Command) {
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }

    pub fn contain(child: &Child) {
        if let Some(job) = job() {
            // SAFETY: both handles are valid for the call; the child is still alive (it
            // waits for its request on stdin).
            unsafe {
                let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()));
            }
        }
    }

    /// In the worker: mitigations that don't get in the way of our engines. Failures are
    /// ignored (older Windows); the job's limits apply regardless.
    pub fn enter() {
        // SAFETY: each call passes a policy struct of the matching type and size.
        unsafe {
            let mut img = PROCESS_MITIGATION_IMAGE_LOAD_POLICY::default();
            // NoRemoteImages | NoLowMandatoryLabelImages | PreferSystem32Images.
            img.Anonymous.Flags = 0b111;
            let _ = SetProcessMitigationPolicy(
                ProcessImageLoadPolicy,
                (&img as *const PROCESS_MITIGATION_IMAGE_LOAD_POLICY).cast(),
                size_of::<PROCESS_MITIGATION_IMAGE_LOAD_POLICY>(),
            );
            let mut ext = PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY::default();
            ext.Anonymous.Flags = 1;
            let _ = SetProcessMitigationPolicy(
                ProcessExtensionPointDisablePolicy,
                (&ext as *const PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY).cast(),
                size_of::<PROCESS_MITIGATION_EXTENSION_POINT_DISABLE_POLICY>(),
            );
            let mut strict = PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY::default();
            // RaiseExceptionOnInvalidHandleReference | HandleExceptionsPermanentlyEnabled.
            strict.Anonymous.Flags = 0b11;
            let _ = SetProcessMitigationPolicy(
                ProcessStrictHandleCheckPolicy,
                (&strict as *const PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY).cast(),
                size_of::<PROCESS_MITIGATION_STRICT_HANDLE_CHECK_POLICY>(),
            );
        }
    }
}

#[cfg(not(windows))]
mod sandbox {
    use std::process::{Child, Command};

    pub fn configure(_cmd: &mut Command) {}
    pub fn contain(_child: &Child) {}
    pub fn enter() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_replies_round_trip() {
        let sheets = vec![
            WireSheet {
                src: WireSrc::Page { file: 0, page: 2 },
                turn: 90,
                crop: Some([1.0, 2.0, 3.0, 4.0]),
            },
            WireSheet {
                src: WireSrc::Blank { w: 595.0, h: 842.0 },
                turn: 0,
                crop: None,
            },
        ];
        let marks = vec![
            WireMark::Redact {
                page: 0,
                rect: [0.0, 0.0, 9.0, 9.0],
            },
            WireMark::Picture {
                page: 1,
                rect: [1.0, 1.0, 2.0, 2.0],
                image: vec![137, 80, 78, 71],
            },
            WireMark::Comment {
                page: 0,
                comment: WireComment::Ink {
                    strokes: vec![vec![[1.0, 2.0], [3.0, 4.0]]],
                    color: [1, 2, 3],
                    width: 2.0,
                },
            },
        ];
        let reqs = vec![
            Request::Convert {
                jobs: vec![(PathBuf::from("a b/ü.png"), "jpg".into())],
                options: WireOptions::from(&Options::default()),
            },
            Request::Tool {
                tool: "pdf-split-ranges".into(),
                files: vec![PathBuf::from("x.pdf")],
                args: [("ranges".to_string(), "1-2; 3".to_string())].into(),
            },
            Request::PageOp {
                file: PathBuf::from("x.pdf"),
                op: PageOp::Images {
                    png: true,
                    dpi: 300,
                },
                pages: vec![0, 2],
            },
            Request::Organise {
                inputs: vec![PathBuf::from("x.pdf")],
                sheets,
                marks,
            },
            Request::Cancel,
        ];
        for r in reqs {
            let line = serde_json::to_string(&r).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<Request>(&line).unwrap(), r);
        }
        let ev = JobEvent::Done {
            index: 3,
            input: "a".into(),
            output: "b".into(),
            bytes_in: 1,
            bytes_out: 2,
            ms: 3,
            route: "x".into(),
            lossless: true,
            note: None,
        };
        let line = serde_json::to_string(&Reply::Job { event: ev }).unwrap();
        let Reply::Job { event } = serde_json::from_str(&line).unwrap() else {
            panic!("{line}")
        };
        assert_eq!(index_of(&reindex(&event, 7)), 7);
        let line = serde_json::to_string(&Reply::Finished {
            result: Err("nope".into()),
        })
        .unwrap();
        assert!(matches!(
            serde_json::from_str(&line).unwrap(),
            Reply::Finished { result: Err(_) }
        ));
    }

    #[test]
    fn options_and_marks_survive_the_trip() {
        let o = Options {
            preset: Preset::Smallest,
            pdf: PdfOptions {
                archival: true,
                tagged: true,
                attach_source: false,
                accessible: false,
            },
            out_dir: Some(PathBuf::from("out")),
            jobs: 3,
        };
        let back = WireOptions::from(&o).options();
        assert_eq!(back.preset, Preset::Smallest);
        assert_eq!(back.pdf, o.pdf);
        assert_eq!((back.out_dir, back.jobs), (o.out_dir, 3));
        let m = Mark::Comment {
            page: 4,
            comment: Comment::Note {
                at: [1.0, 2.0],
                text: "hi".into(),
                color: [9, 9, 9],
            },
        };
        assert_eq!(WireMark::from(&m).mark(), m);
        let s = Sheet::page(1, 2);
        assert_eq!(WireSheet::from(&s).sheet(), s);
    }
}
