//! Batch runner (docs/12 §3.3): one coordinator thread per run; events go to the UI
//! through a channel, and the waker repaints the window. The UI thread never blocks.
//! The work itself runs in a worker process when the host configured one
//! (`swirvo_jobs::worker`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use swirvo_core::{CancelToken, FormatId};
use swirvo_jobs::tools::{Tool, ToolResult};
use swirvo_jobs::{JobEvent, Options, Toolkit};
use swirvo_ui::Waker;

#[derive(Debug)]
pub enum RunEvent {
    Done {
        item: u64,
        output: PathBuf,
        bytes_out: u64,
        /// Something the user should know: saved elsewhere, or a fallback engine was used.
        note: Option<String>,
    },
    Failed {
        item: u64,
        error: String,
    },
    Skipped {
        item: u64,
        reason: String,
    },
    Tool(Result<ToolResult, String>),
    Finished {
        ms: u64,
    },
}

pub struct Run {
    pub rx: Receiver<RunEvent>,
    pub cancel: Arc<CancelToken>,
    pub total: usize,
}

/// Convert each group of (item id, path) to its target.
pub fn convert(
    tk: Arc<Toolkit>,
    groups: Vec<(FormatId, Vec<(u64, PathBuf)>)>,
    opts: Options,
    waker: Waker,
) -> Run {
    let (tx, rx) = channel();
    let cancel = Arc::new(CancelToken::new());
    let total = groups.iter().map(|g| g.1.len()).sum();
    let c = cancel.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        // One pool for the whole batch, whatever each file becomes.
        let mut jobs = Vec::with_capacity(total);
        let mut ids = Vec::with_capacity(total);
        for (to, items) in groups {
            for (item, path) in items {
                jobs.push((path, to));
                ids.push(item);
            }
        }
        let tx2 = std::sync::Mutex::new(tx.clone());
        let w2 = waker.clone();
        swirvo_jobs::worker::convert_each(&tk, &jobs, &opts, &c, &|ev| {
            let msg = match ev {
                JobEvent::Done {
                    index,
                    output,
                    bytes_out,
                    note,
                    ..
                } => RunEvent::Done {
                    item: ids[*index],
                    output: output.clone(),
                    bytes_out: *bytes_out,
                    note: note.clone(),
                },
                JobEvent::Failed { index, error, .. } => RunEvent::Failed {
                    item: ids[*index],
                    error: error.clone(),
                },
                JobEvent::Skipped { index, reason, .. } => RunEvent::Skipped {
                    item: ids[*index],
                    reason: reason.clone(),
                },
            };
            let _ = tx2.lock().unwrap_or_else(|p| p.into_inner()).send(msg);
            w2();
        });
        let _ = tx.send(RunEvent::Finished {
            ms: started.elapsed().as_millis() as u64,
        });
        waker();
    });
    Run { rx, cancel, total }
}

/// Run a tool on `files` with its options.
pub fn tool(
    tk: Arc<Toolkit>,
    tool: Tool,
    files: Vec<PathBuf>,
    args: swirvo_jobs::args::ToolArgs,
    waker: Waker,
) -> Run {
    let (tx, rx) = channel();
    let cancel = Arc::new(CancelToken::new());
    let c = cancel.clone();
    let total = files.len();
    std::thread::spawn(move || {
        let started = Instant::now();
        let r = swirvo_jobs::worker::run_tool_with(&tk, tool, &files, &args, &c)
            .map_err(|e| e.to_string());
        let _ = tx.send(RunEvent::Tool(r));
        let _ = tx.send(RunEvent::Finished {
            ms: started.elapsed().as_millis() as u64,
        });
        waker();
    });
    Run { rx, cancel, total }
}
