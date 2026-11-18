//! Running conversions (docs/03 §7).
//!
//! Stages per input: detect → plan → reserve output name → run steps in a temp folder inside
//! the destination → verify → atomic no-replace commit. Originals are never opened for
//! writing, and an existing file is never replaced.
//!
//! In the shipping app these steps run in sandboxed `swirvo-worker` processes; the CLI's
//! standalone mode runs them in-process with the same code.

pub mod args;
mod commit;
mod doc_ops;
mod media_ops;
pub mod organise;
pub mod pages;
pub mod pdf_ops;
pub mod settings;
mod store;
pub mod tools;
mod wheel_items;
pub mod worker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use swirvo_core::{CancelToken, Engine, EngineError, FormatId, PdfOptions, Preset, StepRequest};
use swirvo_formats::{Detection, Registry, detect_path, registry};
use swirvo_graph::{Graph, GraphError, Route};

pub use commit::{commit_no_replace, fallback_dir};
pub use store::{data_dir, load_frecency, save_frecency};
pub use wheel_items::{WheelItem, ranked_targets, wheel_items};

/// Other routes tried after the planned one fails on a file.
const MAX_FALLBACKS: usize = 2;

/// Registry + graph + engines: everything needed to plan and run conversions.
pub struct Toolkit {
    pub registry: &'static Registry,
    pub graph: Graph,
    engines: HashMap<&'static str, Box<dyn Engine>>,
}

impl Toolkit {
    /// Built-in engines plus the engine packs found on this PC (FFmpeg, LibreOffice,
    /// Microsoft Office) that the user's settings allow.
    pub fn new() -> Result<Toolkit, GraphError> {
        let d = settings::current().discovery();
        Self::with_engines(swirvo_engines::all_with(swirvo_engines::Packs {
            office: d.office,
            libreoffice: d.libreoffice,
            ffmpeg: d.ffmpeg,
        }))
    }

    /// Only the engines compiled in: the same answers on every machine (tests, CI).
    pub fn with_builtin_engines() -> Result<Toolkit, GraphError> {
        Self::with_engines(swirvo_engines::builtin())
    }

    fn with_engines(list: Vec<Box<dyn Engine>>) -> Result<Toolkit, GraphError> {
        let engines: HashMap<&'static str, Box<dyn Engine>> =
            list.into_iter().map(|e| (e.id(), e)).collect();
        let caps = engines.values().flat_map(|e| e.capabilities());
        let graph = Graph::build(registry(), caps, &[])?;
        Ok(Toolkit {
            registry: registry(),
            graph,
            engines,
        })
    }

    pub fn detect(&self, path: &Path) -> std::io::Result<Option<Detection>> {
        detect_path(path)
    }

    pub fn plan(&self, from: FormatId, to: FormatId, preset: Preset, bytes: u64) -> Option<Route> {
        self.graph.route(from, to, preset, bytes as f64 / 1e6)
    }

    /// Ids of the engines in use (built-in and discovered packs).
    pub fn engine_ids(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.engines.keys().copied().collect();
        v.sort_unstable();
        v
    }

    /// Run a planned route from `input` to `output`. Intermediates live in `scratch`.
    pub fn run_route(
        &self,
        route: &Route,
        input: &Path,
        output: &Path,
        scratch: &Path,
        opts: &Options,
        cancel: &CancelToken,
    ) -> Result<(), EngineError> {
        self.run_steps(route, input, output, scratch, opts, cancel)
            .map_err(|(_, e)| e)
    }

    /// [`Self::run_route`], also naming the engine of the step that failed.
    fn run_steps(
        &self,
        route: &Route,
        input: &Path,
        output: &Path,
        scratch: &Path,
        opts: &Options,
        cancel: &CancelToken,
    ) -> Result<(), (&'static str, EngineError)> {
        let mut current = input.to_path_buf();
        for (i, &e) in route.edges.iter().enumerate() {
            let edge = self.graph.edge(e);
            let fail = |err| (edge.cap.engine, err);
            cancel.check().map_err(fail)?;
            let last = i + 1 == route.edges.len();
            let to_key = self.registry.key(edge.to);
            let next = if last {
                output.to_path_buf()
            } else {
                scratch.join(format!("step{i}.{}", self.registry.primary_ext(edge.to)))
            };
            let engine = self.engines.get(edge.cap.engine).ok_or_else(|| {
                fail(EngineError::Invalid(format!(
                    "engine '{}' is not installed",
                    edge.cap.engine
                )))
            })?;
            engine
                .run(&StepRequest {
                    input: &current,
                    output: &next,
                    from: self.registry.key(edge.from),
                    to: to_key,
                    preset: opts.preset,
                    pdf: opts.pdf,
                    cancel,
                })
                .map_err(fail)?;
            current = next;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub preset: Preset,
    /// PDF/A and tagging for PDFs our engines write.
    pub pdf: PdfOptions,
    /// None = next to each source (the default).
    pub out_dir: Option<PathBuf>,
    pub jobs: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            preset: Preset::Balanced,
            pdf: PdfOptions::default(),
            out_dir: None,
            jobs: std::thread::available_parallelism().map_or(4, |n| n.get()),
        }
    }
}

impl Options {
    /// The defaults the user chose in Settings: quality preset, save location, and how many
    /// files at once.
    pub fn from_settings() -> Options {
        let s = settings::current();
        Options {
            preset: s.preset.preset(),
            out_dir: s.out_dir.clone(),
            jobs: s.jobs(),
            ..Options::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JobEvent {
    Done {
        index: usize,
        input: PathBuf,
        output: PathBuf,
        bytes_in: u64,
        bytes_out: u64,
        ms: u64,
        route: String,
        lossless: bool,
        /// Set when the source folder wasn't writable and the output went elsewhere.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    Failed {
        index: usize,
        input: PathBuf,
        error: String,
    },
    Skipped {
        index: usize,
        input: PathBuf,
        reason: String,
    },
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct Summary {
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

/// Output names already claimed by running jobs, so two parallel jobs never pick the same
/// "photo.jpg". The filesystem's no-replace commit still guards against other programs.
#[derive(Default)]
struct Reservations(Mutex<std::collections::HashSet<PathBuf>>);

impl Reservations {
    fn key(p: &Path) -> PathBuf {
        // Windows and macOS file systems are case-insensitive; be conservative everywhere.
        PathBuf::from(p.to_string_lossy().to_lowercase())
    }

    /// `dir/stem.ext`, then `dir/stem (2).ext`, … : the first name that is free on disk and
    /// not reserved by another job.
    fn claim(&self, dir: &Path, stem: &str, ext: &str) -> PathBuf {
        let mut set = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for k in 1usize.. {
            let name = if k == 1 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem} ({k}).{ext}")
            };
            let p = dir.join(name);
            if !p.exists() && set.insert(Self::key(&p)) {
                return p;
            }
        }
        unreachable!("1.. is unbounded")
    }

    fn release(&self, p: &Path) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&Self::key(p));
    }
}

/// Convert every input to `to`, in parallel. Events stream to `on_event` as jobs finish.
pub fn convert(
    tk: &Toolkit,
    inputs: &[PathBuf],
    to: FormatId,
    opts: &Options,
    cancel: &CancelToken,
    on_event: &(dyn Fn(&JobEvent) + Sync),
) -> Summary {
    convert_to(tk, inputs, &|_| to, opts, cancel, &|_| {}, on_event)
}

/// Convert each (input, target) pair, all in one pool of workers: a batch with several
/// targets (some files to PDF, others to JPG) keeps every worker busy instead of
/// finishing one target before starting the next. Event indices are positions in `jobs`.
pub fn convert_each(
    tk: &Toolkit,
    jobs: &[(PathBuf, FormatId)],
    opts: &Options,
    cancel: &CancelToken,
    on_event: &(dyn Fn(&JobEvent) + Sync),
) -> Summary {
    let inputs: Vec<PathBuf> = jobs.iter().map(|j| j.0.clone()).collect();
    convert_to(tk, &inputs, &|i| jobs[i].1, opts, cancel, &|_| {}, on_event)
}

/// [`convert_each`], also telling `on_start` when each job begins (the worker reports
/// it, so a crash can be pinned on the files that were being converted).
pub(crate) fn convert_each_observed(
    tk: &Toolkit,
    jobs: &[(PathBuf, FormatId)],
    opts: &Options,
    cancel: &CancelToken,
    on_start: &(dyn Fn(usize) + Sync),
    on_event: &(dyn Fn(&JobEvent) + Sync),
) -> Summary {
    let inputs: Vec<PathBuf> = jobs.iter().map(|j| j.0.clone()).collect();
    convert_to(
        tk,
        &inputs,
        &|i| jobs[i].1,
        opts,
        cancel,
        on_start,
        on_event,
    )
}

fn convert_to(
    tk: &Toolkit,
    inputs: &[PathBuf],
    target: &(dyn Fn(usize) -> FormatId + Sync),
    opts: &Options,
    cancel: &CancelToken,
    on_start: &(dyn Fn(usize) + Sync),
    on_event: &(dyn Fn(&JobEvent) + Sync),
) -> Summary {
    let next = AtomicUsize::new(0);
    let reservations = Reservations::default();
    let summary = Mutex::new(Summary::default());
    let workers = opts.jobs.clamp(1, inputs.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(input) = inputs.get(i) else { break };
                    let ev = if cancel.is_cancelled() {
                        JobEvent::Skipped {
                            index: i,
                            input: input.clone(),
                            reason: "cancelled".into(),
                        }
                    } else {
                        on_start(i);
                        convert_one(tk, i, input, target(i), opts, cancel, &reservations)
                    };
                    {
                        let mut sm = summary.lock().unwrap_or_else(|p| p.into_inner());
                        match &ev {
                            JobEvent::Done {
                                bytes_in,
                                bytes_out,
                                ..
                            } => {
                                sm.done += 1;
                                sm.bytes_in += bytes_in;
                                sm.bytes_out += bytes_out;
                            }
                            JobEvent::Failed { .. } => sm.failed += 1,
                            JobEvent::Skipped { .. } => sm.skipped += 1,
                        }
                    }
                    on_event(&ev);
                }
            });
        }
    });
    summary.into_inner().unwrap_or_else(|p| p.into_inner())
}

fn convert_one(
    tk: &Toolkit,
    index: usize,
    input: &Path,
    to: FormatId,
    opts: &Options,
    cancel: &CancelToken,
    res: &Reservations,
) -> JobEvent {
    let fail = |error: String| JobEvent::Failed {
        index,
        input: input.to_path_buf(),
        error,
    };
    let reg = tk.registry;
    let started = Instant::now();

    let meta = match std::fs::metadata(input) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return fail("not a file".into()),
        Err(e) => return fail(format!("can't open it: {e}")),
    };
    let det = match tk.detect(input) {
        Ok(Some(d)) => d,
        Ok(None) => return fail("unrecognized file type".into()),
        Err(e) => return fail(format!("can't read it: {e}")),
    };
    let from = det.format;
    if from == to {
        return JobEvent::Skipped {
            index,
            input: input.to_path_buf(),
            reason: format!("already {}", reg.get(to).name),
        };
    }
    let mb = meta.len() as f64 / 1e6;
    let Some(route) = tk.plan(from, to, opts.preset, meta.len()) else {
        return fail(format!(
            "no way to convert {} to {} yet",
            reg.get(from).name,
            reg.get(to).name
        ));
    };

    let mut dest_dir = match &opts.out_dir {
        Some(d) => d.clone(),
        None => input
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    };
    // Scratch space on the destination volume, so the final commit is a cheap link.
    let mut note = None;
    let scratch = match tempfile::Builder::new()
        .prefix(".swirvo-tmp-")
        .tempdir_in(&dest_dir)
    {
        Ok(t) => t,
        Err(e) if opts.out_dir.is_none() => match fallback_dir().and_then(|d| {
            tempfile::Builder::new()
                .prefix(".swirvo-tmp-")
                .tempdir_in(&d)
                .map(|t| (d, t))
        }) {
            Ok((d, t)) => {
                note = Some(format!(
                    "Couldn't save next to the original ({e}). Saved to {} instead.",
                    d.display()
                ));
                dest_dir = d;
                t
            }
            Err(e2) => {
                return fail(format!(
                    "can't write next to the original ({e}) or to Downloads ({e2})"
                ));
            }
        },
        Err(e) => return fail(format!("can't write to {}: {e}", dest_dir.display())),
    };

    let stem = reg.stem(input);
    let ext = reg.primary_ext(to);
    let mut output = res.claim(&dest_dir, &stem, ext);
    let tmp_out = scratch.path().join(format!("out.{ext}"));
    // When an engine fails on this file, plan again without it: a Word document our layout
    // engine can't read still converts with LibreOffice if present, or the typesetter.
    let mut route = route;
    let mut avoid: Vec<&'static str> = Vec::new();
    let mut first_error = None;
    while let Err((engine, e)) = tk.run_steps(&route, input, &tmp_out, scratch.path(), opts, cancel)
    {
        let cancelled = matches!(e, EngineError::Cancelled) || cancel.check().is_err();
        avoid.push(engine);
        let next = (!cancelled && avoid.len() <= MAX_FALLBACKS)
            .then(|| tk.graph.route_avoiding(from, to, opts.preset, mb, &avoid))
            .flatten();
        let first = first_error.get_or_insert(e);
        match next {
            Some(r) => {
                // Our own scratch files from the failed attempt, never a user file.
                let _ = std::fs::remove_file(&tmp_out);
                route = r;
            }
            None => {
                res.release(&output);
                return fail(first.to_string());
            }
        }
    }
    if let Some(e) = &first_error {
        // The route (shown by the CLI and logged) names the engine that did the work.
        let msg = format!(
            "The best engine couldn't convert this file ({e}), so a fallback was used instead. Check the result."
        );
        note = Some(match note {
            Some(n) => format!("{n} {msg}"),
            None => msg,
        });
    }
    let bytes_out = match std::fs::metadata(&tmp_out) {
        Ok(m) if m.len() > 0 => m.len(),
        _ => {
            res.release(&output);
            return fail("the engine produced an empty file".into());
        }
    };
    // Commit; if another program grabbed the name meanwhile, take the next free one.
    let mut attempts = 0;
    loop {
        match commit_no_replace(&tmp_out, &output) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 5 => {
                attempts += 1;
                let taken = output.clone();
                output = res.claim(&dest_dir, &stem, ext);
                res.release(&taken);
            }
            Err(e) => {
                res.release(&output);
                return fail(format!("couldn't save {}: {e}", output.display()));
            }
        }
    }
    JobEvent::Done {
        index,
        input: input.to_path_buf(),
        output,
        bytes_in: meta.len(),
        bytes_out,
        ms: started.elapsed().as_millis() as u64,
        route: tk.graph.explain(&route),
        lossless: route.is_lossless(),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Word file the layout engine refuses (nested past its limit) still converts: the
    /// job falls back to the next engine and says so.
    #[test]
    fn falls_back_to_another_engine_when_one_fails() {
        use std::io::Write as _;
        let tk = Toolkit::with_builtin_engines().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("odd.docx");
        let w = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;
        let n = 300;
        let body = format!(
            "{}<w:p><w:r><w:t>still here</w:t></w:r></w:p>{}",
            "<w:sdt><w:sdtContent>".repeat(n),
            "</w:sdtContent></w:sdt>".repeat(n)
        );
        let doc = format!("<w:document {w}><w:body>{body}</w:body></w:document>");
        let rels = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("[Content_Types].xml", o).unwrap();
            z.write_all(br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#).unwrap();
            z.start_file("_rels/.rels", o).unwrap();
            z.write_all(rels.as_bytes()).unwrap();
            z.start_file("word/document.xml", o).unwrap();
            z.write_all(doc.as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let pdf = tk.registry.id("pdf").unwrap();
        let docx = tk.registry.id("docx").unwrap();
        let first = tk.plan(docx, pdf, Preset::Balanced, 1000).unwrap();
        assert_eq!(tk.graph.edge(first.edges[0]).cap.engine, "docx-layout");
        let events = Mutex::new(Vec::new());
        let sum = convert(
            &tk,
            std::slice::from_ref(&src),
            pdf,
            &Options::default(),
            &CancelToken::new(),
            &|e| events.lock().unwrap().push(e.clone()),
        );
        let events = events.into_inner().unwrap();
        assert_eq!(sum.done, 1, "{events:?}");
        let note = events
            .iter()
            .find_map(|e| match e {
                JobEvent::Done { note, route, .. } => Some((note.clone(), route.clone())),
                _ => None,
            })
            .unwrap();
        assert!(!note.1.contains("docx-layout"), "{note:?}");
        assert!(
            note.0.as_deref().is_some_and(|n| n.contains("instead")),
            "{note:?}"
        );
    }

    fn png(path: &Path) {
        image::RgbImage::from_fn(16, 12, |x, y| image::Rgb([x as u8 * 10, y as u8 * 10, 90]))
            .save(path)
            .unwrap();
    }

    #[test]
    fn a_mixed_batch_runs_in_one_pool() {
        let tk = Toolkit::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.png"), dir.path().join("b.png"));
        png(&a);
        png(&b);
        let (jpg, webp) = (
            tk.registry.id("jpg").unwrap(),
            tk.registry.id("webp").unwrap(),
        );
        let jobs = vec![(a.clone(), jpg), (b.clone(), webp), (a, webp)];
        let sum = convert_each(
            &tk,
            &jobs,
            &Options::default(),
            &CancelToken::new(),
            &|_| {},
        );
        assert_eq!(sum.done, 3);
        for f in ["a.jpg", "b.webp", "a.webp"] {
            assert!(dir.path().join(f).exists(), "{f}");
        }
    }

    #[test]
    fn converts_next_to_source_and_never_overwrites() {
        let tk = Toolkit::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("shot.png");
        png(&src);
        std::fs::write(dir.path().join("shot.jpg"), b"existing file, must survive").unwrap();
        let jpg = tk.registry.id("jpg").unwrap();
        let events = Mutex::new(Vec::new());
        let sum = convert(
            &tk,
            &[src.clone(), src.clone()],
            jpg,
            &Options::default(),
            &CancelToken::new(),
            &|e| events.lock().unwrap().push(e.clone()),
        );
        assert_eq!(sum.done, 2, "{:?}", events.lock().unwrap());
        assert_eq!(
            std::fs::read(dir.path().join("shot.jpg")).unwrap(),
            b"existing file, must survive"
        );
        let mut outs: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                JobEvent::Done { output, .. } => {
                    Some(output.file_name().unwrap().to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect();
        outs.sort();
        assert_eq!(outs, vec!["shot (2).jpg", "shot (3).jpg"]);
        // No scratch folders left behind.
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".swirvo-tmp")
        }));
    }

    #[test]
    fn multi_hop_route_and_skip_same_format() {
        let tk = Toolkit::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let csv = dir.path().join("t.csv");
        std::fs::write(&csv, "a,b\n1,x\n").unwrap();
        let yaml = tk.registry.id("yaml").unwrap();
        let ev = Mutex::new(None);
        convert(
            &tk,
            std::slice::from_ref(&csv),
            yaml,
            &Options::default(),
            &CancelToken::new(),
            &|e| *ev.lock().unwrap() = Some(e.clone()),
        );
        let Some(JobEvent::Done { output, .. }) = ev.into_inner().unwrap() else {
            panic!("expected success")
        };
        assert!(std::fs::read_to_string(output).unwrap().contains("a: 1"));

        let csv_id = tk.registry.id("csv").unwrap();
        let s = convert(
            &tk,
            &[csv],
            csv_id,
            &Options::default(),
            &CancelToken::new(),
            &|_| {},
        );
        assert_eq!(s.skipped, 1);
    }
}
