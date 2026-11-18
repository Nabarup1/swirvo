//! The worker process for real: conversions and tools run in `swirvo --worker`, a crash
//! fails only the file it was on, and cancel stops the worker.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use swirvo_core::CancelToken;
use swirvo_jobs::tools::Tool;
use swirvo_jobs::worker::{self, CRASHED};
use swirvo_jobs::{JobEvent, Options, Toolkit};

fn png(path: &Path) {
    image::RgbaImage::from_fn(40, 30, |x, y| {
        image::Rgba([x as u8 * 6, y as u8 * 8, 90, 255])
    })
    .save_with_format(path, image::ImageFormat::Png)
    .expect("write png");
}

/// One test: the worker setting and the crash switch are process-wide.
#[test]
fn conversions_and_tools_run_in_the_worker() {
    worker::set_worker(Some(PathBuf::from(env!("CARGO_BIN_EXE_swirvo"))));
    let tk = Toolkit::with_builtin_engines().unwrap();
    let jpg = tk.registry.id("jpg").unwrap();
    let d = tempfile::tempdir().unwrap();
    let files: Vec<PathBuf> = ["a.png", "crash-me.png", "c.png"]
        .iter()
        .map(|n| {
            let p = d.path().join(n);
            png(&p);
            p
        })
        .collect();

    // A plain batch.
    let jobs: Vec<(PathBuf, _)> = files.iter().map(|f| (f.clone(), jpg)).collect();
    let events = Mutex::new(Vec::new());
    let one_at_a_time = Options {
        jobs: 1,
        ..Options::default()
    };
    let s = worker::convert_each(&tk, &jobs, &one_at_a_time, &CancelToken::new(), &|e| {
        events.lock().unwrap().push(e.clone())
    });
    assert_eq!(s.done, 3, "{:?}", events.lock().unwrap());
    for f in ["a.jpg", "crash-me.jpg", "c.jpg"] {
        assert!(d.path().join(f).exists(), "{f}");
    }

    // A worker that dies on one file: that file fails, the next one runs in a fresh
    // worker, and the batch carries on.
    // SAFETY: this is the only test in this binary, so nothing reads the environment
    // concurrently; the child workers inherit it.
    unsafe { std::env::set_var("SWIRVO_WORKER_CRASH_ON", "crash-me") };
    let events = Mutex::new(Vec::new());
    let s = worker::convert_each(&tk, &jobs, &one_at_a_time, &CancelToken::new(), &|e| {
        events.lock().unwrap().push(e.clone())
    });
    let ev = events.into_inner().unwrap();
    assert_eq!((s.done, s.failed), (2, 1), "{ev:?}");
    let failed: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            JobEvent::Failed { index, error, .. } => Some((*index, error.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, 1, "the index is the caller's");
    assert!(failed[0].1.starts_with(CRASHED), "{}", failed[0].1);
    assert!(d.path().join("a (2).jpg").exists() && d.path().join("c (2).jpg").exists());

    // Tools too: one that crashes says so, one that works returns its result.
    let r = worker::run_tool_with(
        &tk,
        Tool::Checksum,
        &[files[1].clone()],
        &Default::default(),
        &CancelToken::new(),
    );
    let e = r.expect_err("the crash is reported").to_string();
    assert!(e.contains("stopped unexpectedly"), "{e}");
    unsafe { std::env::remove_var("SWIRVO_WORKER_CRASH_ON") };
    let r = worker::run_tool_with(
        &tk,
        Tool::Checksum,
        &[files[0].clone()],
        &Default::default(),
        &CancelToken::new(),
    )
    .unwrap();
    assert!(r.clipboard.is_some_and(|c| c.len() >= 64), "{}", r.message);

    // Cancel: a long batch stops early, and quickly. Each job is a 2000 × 2000 picture, so
    // even a fast machine is still busy when the first file reports back.
    let big = d.path().join("big.png");
    image::RgbaImage::from_fn(2000, 2000, |x, y| {
        let n = x
            .wrapping_mul(2_654_435_761)
            .wrapping_add(y.wrapping_mul(40_503));
        image::Rgba([n as u8, (n >> 8) as u8, (n >> 16) as u8, 255])
    })
    .save_with_format(&big, image::ImageFormat::Png)
    .expect("write big png");
    let many: Vec<(PathBuf, _)> = (0..60).map(|_| (big.clone(), jpg)).collect();
    let cancel = CancelToken::new();
    let started = std::time::Instant::now();
    // Cancel as soon as the first file is done: on a fast machine a timer could fire
    // after all 60 tiny conversions have already finished.
    let s = worker::convert_each(&tk, &many, &one_at_a_time, &cancel, &|_| cancel.cancel());
    assert!(s.skipped + s.failed > 0, "{s:?}");
    assert_eq!(s.done + s.skipped + s.failed, 60);
    assert!(started.elapsed().as_secs() < 20);

    // Without a worker, the same calls run in-process.
    worker::set_worker(None);
    let s = worker::convert_each(
        &tk,
        &jobs[..1],
        &Options::default(),
        &CancelToken::new(),
        &|_| {},
    );
    assert_eq!(s.done, 1);
}
