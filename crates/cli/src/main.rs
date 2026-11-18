//! `swirvo`: the command line (docs/01 §10). Runs standalone; once the agent exists it
//! will forward jobs to it over IPC and fall back to standalone when the agent isn't running.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

mod mcp;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use swirvo_core::{CancelToken, Category, FormatId, Preset};
use swirvo_jobs::{JobEvent, Options, Toolkit, load_frecency, save_frecency, wheel_items};

/// `println!` that exits quietly when stdout is closed (`swirvo formats | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        use std::io::Write as _;
        if let Err(e) = writeln!(std::io::stdout().lock(), $($t)*) {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        }
    }};
}

#[derive(Parser)]
#[command(
    name = "swirvo",
    version,
    about = "Zero-click file conversion. Offline, next to the original, never overwriting."
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert files; outputs go next to each source unless --out is given.
    Convert {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Target format: a key or extension (jpg, webp, pdf, yaml, tar.zst, …).
        #[arg(long, short)]
        to: String,
        #[arg(long, short, default_value = "balanced")]
        preset: String,
        /// Write outputs into this folder instead.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Parallel jobs (default: number of CPU threads).
        #[arg(long, short)]
        jobs: Option<usize>,
        /// JSON Lines events on stdout, for scripts.
        #[arg(long)]
        json: bool,
        /// PDFs we write are PDF/A-2 (archival): PDF/A-2a with --tagged, else PDF/A-2b.
        #[arg(long)]
        pdfa: bool,
        /// PDFs we write are tagged (accessible): headings, lists, tables, alt text.
        #[arg(long)]
        tagged: bool,
        /// PDFs we write are PDF/A-3 with the original document attached inside
        /// (PDF/A-3a with --tagged, else PDF/A-3b).
        #[arg(long, conflicts_with = "pdfua")]
        pdfa3: bool,
        /// PDFs we write are PDF/UA-1 (universal accessibility): tagged, titled, with an
        /// outline and picture descriptions. Not combined with --pdfa / --pdfa3.
        #[arg(long, conflicts_with = "pdfa")]
        pdfua: bool,
    },
    /// Show how a file (or format) would be converted.
    Plan {
        /// A file path, or a format key such as "png".
        input: String,
        #[arg(long, short)]
        to: String,
        #[arg(long, short, default_value = "balanced")]
        preset: String,
    },
    /// Extract archives into a new folder next to each one (never merging into an existing one).
    Extract {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Run a tool (resize, compress, pdf-merge, pdf-split, checksum, zip…) on files.
    /// `swirvo tools FILE…` lists the ones that apply.
    Tool {
        /// Tool id, e.g. resize-half, fit-1920, rotate-right, flip, compress, strip-metadata,
        /// images-to-pdf, pdf-merge, pdf-split, pdf-rotate, pdf-to-png, pdf-to-jpg, extract,
        /// test-archive, checksum, zip, copy-text, pdf-pages.
        name: String,
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// PDF page tools only: which pages, e.g. "1-3, 7, 10-", "odd", "even".
        #[arg(long)]
        pages: Option<String>,
        /// With --pages: what to do (images-jpg, images-png, extract, split, rotate,
        /// delete). Defaults to the tool's own operation.
        #[arg(long)]
        op: Option<String>,
        /// With --pages and an images operation: resolution.
        #[arg(long, default_value_t = 150)]
        dpi: u32,
        /// A tool option, e.g. --set password=… --set paper=letter (repeat for more).
        /// `swirvo tools FILE` lists each tool's options.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// List the tools that apply to these files, with their options.
    Tools {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Detect file types from content, flagging extensions that lie.
    Detect {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// List formats, or what one format can become.
    Formats {
        /// Show targets reachable from this format.
        #[arg(long)]
        from: Option<String>,
        /// Only formats the installed engines can produce or read.
        #[arg(long)]
        supported: bool,
    },
    /// Run as an MCP server on stdin/stdout, so AI assistants (Claude Desktop, Claude Code,
    /// Cursor, VS Code…) can convert files and run tools locally.
    Mcp,
    /// Show the Convert wheel Swirvo would open for these files; optionally render it.
    Wheel {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Save a PNG of the wheel.
        #[arg(long)]
        png: Option<PathBuf>,
        /// Highlight this wedge in the PNG (0 = 12 o'clock).
        #[arg(long, default_value_t = 0)]
        hover: usize,
        #[arg(long, value_enum, default_value_t = ThemeArg::Light)]
        theme: ThemeArg,
        #[arg(long, default_value_t = 2.0)]
        scale: f32,
        /// Also draw the preset sub-ring outside the hovered wedge.
        #[arg(long)]
        presets: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ThemeArg {
    Light,
    Dark,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn human(bytes: u64) -> String {
    let b = bytes as f64;
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.0} KB", b / 1e3),
        1_000_000..10_000_000 => format!("{:.1} MB", b / 1e6),
        10_000_000..1_000_000_000 => format!("{:.0} MB", b / 1e6),
        _ => format!("{:.2} GB", b / 1e9),
    }
}

fn target(tk: &Toolkit, name: &str) -> Result<FormatId> {
    tk.registry
        .find(name)
        .with_context(|| format!("unknown format '{name}'. Try `swirvo formats`."))
}

fn main() -> ExitCode {
    // `swirvo --worker`: serve one request from the app (crates/jobs/src/worker.rs).
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == swirvo_jobs::worker::WORKER_ARG)
    {
        return ExitCode::from(swirvo_jobs::worker::serve().clamp(0, 255) as u8);
    }
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("swirvo: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let tk = Toolkit::new()?;
    match cli.cmd {
        Cmd::Convert {
            files,
            to,
            preset,
            out,
            jobs,
            json,
            pdfa,
            tagged,
            pdfa3,
            pdfua,
        } => {
            let mut opts = Options {
                preset: preset.parse().map_err(anyhow::Error::msg)?,
                pdf: swirvo_core::PdfOptions {
                    archival: pdfa || pdfa3,
                    tagged: tagged || pdfua,
                    attach_source: pdfa3,
                    accessible: pdfua,
                },
                out_dir: out,
                ..Options::default()
            };
            if let Some(j) = jobs {
                opts.jobs = j.max(1);
            }
            convert(&tk, files, &to, opts, json)
        }
        Cmd::Plan { input, to, preset } => plan(&tk, &input, &to, &preset),
        Cmd::Extract { files, json } => extract(&tk, &files, json),
        Cmd::Tool {
            name,
            files,
            pages: Some(spec),
            op,
            dpi,
            json,
            ..
        } => page_tool(&tk, &name, &files, &spec, op.as_deref(), dpi, json),
        Cmd::Tool {
            name,
            files,
            set,
            json,
            ..
        } => tool(&tk, &name, &files, &set, json),
        Cmd::Tools { files } => {
            use swirvo_jobs::args::FieldKind;
            for t in swirvo_jobs::tools::tools_for(&tk, &files) {
                let (label, caption) = t.label();
                out!("{:<20} {label} · {caption}", t.id());
                for f in t.fields() {
                    let what = match f.kind {
                        FieldKind::Choice(opts) => {
                            opts.iter().map(|o| o.0).collect::<Vec<_>>().join("|")
                        }
                        FieldKind::Number { .. } => "number".into(),
                        FieldKind::Flag => "true|false".into(),
                        FieldKind::File => "path".into(),
                        FieldKind::Secret => "secret".into(),
                        FieldKind::Text => "text".into(),
                        FieldKind::Pages => "pages, e.g. 1-3,7".into(),
                        FieldKind::PageGroups => "pages;pages…".into(),
                    };
                    let def = if f.default.is_empty() {
                        String::new()
                    } else {
                        format!(" (default {})", f.default)
                    };
                    out!("{:<20}   --set {}=<{what}>  {}{def}", "", f.key, f.label);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Detect { files, json } => detect(&tk, &files, json),
        Cmd::Mcp => {
            mcp::serve(&tk)?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Formats { from, supported } => formats(&tk, from.as_deref(), supported),
        Cmd::Wheel {
            files,
            png,
            hover,
            theme,
            scale,
            presets,
        } => wheel(&tk, &files, png, hover, theme, scale, presets),
    }
}

#[allow(clippy::too_many_arguments)]
fn page_tool(
    tk: &Toolkit,
    name: &str,
    files: &[PathBuf],
    spec: &str,
    op: Option<&str>,
    dpi: u32,
    json: bool,
) -> Result<ExitCode> {
    use swirvo_jobs::tools::{PageOp, Tool, run_page_op};
    let [file] = files else {
        bail!("--pages works on one PDF at a time");
    };
    let default = Tool::from_id(name)
        .and_then(Tool::page_op)
        .ok_or_else(|| anyhow::anyhow!("'{name}' isn't a PDF page tool"))?;
    let op = match op {
        None => default,
        Some("images-jpg") => PageOp::Images { png: false, dpi },
        Some("images-png") => PageOp::Images { png: true, dpi },
        Some("extract") => PageOp::Extract,
        Some("split") => PageOp::Split,
        Some("rotate") => PageOp::Rotate,
        Some("delete") => PageOp::Delete,
        Some(o) => bail!("unknown --op '{o}'"),
    };
    let op = match op {
        PageOp::Images { png, .. } => PageOp::Images { png, dpi },
        o => o,
    };
    let count = swirvo_jobs::tools::pdf_page_count(file)
        .ok_or_else(|| anyhow::anyhow!("{}: can't read this PDF", file.display()))?;
    let pages = swirvo_jobs::pages::parse(spec, count).map_err(|e| anyhow::anyhow!(e))?;
    let r = run_page_op(tk, file, op, &pages, &CancelToken::new())?;
    if json {
        out!(
            "{}",
            serde_json::json!({"event": "done", "tool": name, "op": op, "result": r})
        );
    } else {
        out!("{}", r.message);
        for o in &r.outputs {
            out!("  {}", o.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn tool(
    tk: &Toolkit,
    name: &str,
    files: &[PathBuf],
    set: &[String],
    json: bool,
) -> Result<ExitCode> {
    use swirvo_jobs::tools::{Tool, run_tool_with};
    let Some(t) = Tool::from_id(name) else {
        let ids: Vec<&str> = Tool::ALL.iter().map(|t| t.id()).collect();
        bail!("unknown tool '{name}'. Tools: {}", ids.join(", "));
    };
    let args = swirvo_jobs::args::parse_pairs(set).map_err(|e| anyhow::anyhow!(e))?;
    match run_tool_with(tk, t, files, &args, &CancelToken::new()) {
        Ok(r) if json => {
            out!(
                "{}",
                serde_json::json!({"event": "done", "tool": t.id(), "result": r})
            );
            Ok(if r.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Ok(r) => {
            out!("{}", r.message);
            for o in &r.outputs {
                out!("  → {}", o.display());
            }
            // The CLI prints what the app would copy to the clipboard.
            if let Some(c) = &r.clipboard
                && t != Tool::CopyText
            {
                out!("{c}");
            }
            for (f, e) in &r.failures {
                eprintln!("{}: {e}", f.display());
            }
            Ok(if r.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Err(e) if json => {
            out!(
                "{}",
                serde_json::json!({"event": "failed", "tool": t.id(), "error": e.to_string()})
            );
            Ok(ExitCode::FAILURE)
        }
        Err(e) => {
            eprintln!("{}: {e}", t.id());
            Ok(ExitCode::FAILURE)
        }
    }
}

fn extract(tk: &Toolkit, files: &[PathBuf], json: bool) -> Result<ExitCode> {
    let mut failed = 0;
    for f in files {
        match swirvo_jobs::tools::extract_here(tk, f, &CancelToken::new()) {
            Ok(x) if json => out!(
                "{}",
                serde_json::json!({"event": "done", "input": f, "result": x})
            ),
            Ok(x) => {
                out!(
                    "{} → {} ({} files, {})",
                    f.display(),
                    x.dir.display(),
                    x.files,
                    human(x.bytes)
                );
                if let Some(n) = x.note {
                    out!("  note: {n}");
                }
            }
            Err(e) => {
                failed += 1;
                if json {
                    out!(
                        "{}",
                        serde_json::json!({"event": "failed", "input": f, "error": e.to_string()})
                    );
                } else {
                    eprintln!("{}: {e}", f.display());
                }
            }
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn convert(
    tk: &Toolkit,
    files: Vec<PathBuf>,
    to: &str,
    opts: Options,
    json: bool,
) -> Result<ExitCode> {
    let to_id = target(tk, to)?;
    if let Some(o) = &opts.out_dir {
        std::fs::create_dir_all(o).with_context(|| format!("can't create {}", o.display()))?;
    }
    let started = std::time::Instant::now();
    let stdout = std::sync::Mutex::new(std::io::stdout());
    let print = |ev: &JobEvent| {
        let mut o = stdout.lock().unwrap_or_else(|p| p.into_inner());
        if json {
            let _ = serde_json::to_writer(&mut *o, ev);
            let _ = writeln!(o);
            return;
        }
        let _ = match ev {
            JobEvent::Done {
                input,
                output,
                bytes_in,
                bytes_out,
                ms,
                lossless,
                note,
                ..
            } => {
                let delta = (*bytes_out as f64 / (*bytes_in).max(1) as f64 - 1.0) * 100.0;
                let tag = if *lossless { " · lossless" } else { "" };
                let line = writeln!(
                    o,
                    "✓ {} → {}  {} ({:+.0} %) · {} ms{tag}",
                    name(input),
                    name(output),
                    human(*bytes_out),
                    delta,
                    ms
                );
                if let Some(n) = note {
                    let _ = writeln!(o, "  {n}");
                }
                line
            }
            JobEvent::Failed { input, error, .. } => writeln!(o, "✗ {}: {error}", name(input)),
            JobEvent::Skipped { input, reason, .. } => writeln!(o, "– {}: {reason}", name(input)),
        };
    };

    // Remember what people convert to, so the wheel learns (docs/04 §3).
    let frecency = std::sync::Mutex::new(load_frecency());
    let record = |ev: &JobEvent| {
        if let JobEvent::Done { input, .. } = ev
            && let Ok(Some(d)) = tk.detect(input)
        {
            frecency.lock().unwrap_or_else(|p| p.into_inner()).bump(
                tk.registry.key(d.format),
                tk.registry.key(to_id),
                now_ms(),
            );
        }
    };
    // Detection runs again in `record`, but only after success and only on 4 KiB.
    let s = swirvo_jobs::convert(tk, &files, to_id, &opts, &CancelToken::new(), &|ev| {
        print(ev);
        record(ev);
    });
    save_frecency(&frecency.into_inner().unwrap_or_else(|p| p.into_inner()));

    if json {
        out!(
            "{}",
            serde_json::json!({"event": "summary", "done": s.done, "failed": s.failed, "skipped": s.skipped, "bytes_in": s.bytes_in, "bytes_out": s.bytes_out})
        );
    } else if files.len() > 1 {
        out!(
            "{} converted, {} failed, {} skipped in {:.1} s · {} → {}",
            s.done,
            s.failed,
            s.skipped,
            started.elapsed().as_secs_f64(),
            human(s.bytes_in),
            human(s.bytes_out)
        );
    }
    Ok(if s.failed > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn plan(tk: &Toolkit, input: &str, to: &str, preset: &str) -> Result<ExitCode> {
    let preset: Preset = preset.parse().map_err(anyhow::Error::msg)?;
    let to_id = target(tk, to)?;
    let path = PathBuf::from(input);
    let (from, bytes) = if path.is_file() {
        let d = tk.detect(&path)?.context("unrecognized file type")?;
        (d.format, std::fs::metadata(&path)?.len())
    } else {
        (target(tk, input)?, 1_000_000)
    };
    let Some(route) = tk.plan(from, to_id, preset, bytes) else {
        bail!(
            "no way to convert {} to {} yet",
            tk.registry.get(from).name,
            tk.registry.get(to_id).name
        );
    };
    out!("{}", tk.graph.explain(&route));
    out!(
        "preset {preset} · {} step{} · {} · est. {} ms",
        route.edges.len(),
        if route.edges.len() == 1 { "" } else { "s" },
        if route.is_lossless() {
            "lossless".to_string()
        } else {
            format!("quality cost {}", route.cost.loss)
        },
        route.cost.time_ms
    );
    Ok(ExitCode::SUCCESS)
}

fn detect(tk: &Toolkit, files: &[PathBuf], json: bool) -> Result<ExitCode> {
    let mut code = ExitCode::SUCCESS;
    for f in files {
        match tk.detect(f) {
            Ok(Some(d)) => {
                let def = tk.registry.get(d.format);
                let ext = d.ext_guess.map(|e| tk.registry.key(e));
                if json {
                    out!(
                        "{}",
                        serde_json::json!({"file": f, "format": def.key, "name": def.name, "category": def.category.name(), "mime": def.mime, "confidence": format!("{:?}", d.confidence).to_lowercase(), "extension_says": ext, "mismatch": d.mismatch})
                    );
                } else {
                    let warn = if d.mismatch {
                        format!("  ⚠ extension says {}", ext.unwrap_or("?"))
                    } else {
                        String::new()
                    };
                    out!(
                        "{}: {} ({}, {:?}){warn}",
                        name(f),
                        def.name,
                        def.category,
                        d.confidence
                    );
                }
            }
            Ok(None) => {
                out!("{}: unknown", name(f));
                code = ExitCode::from(1);
            }
            Err(e) => {
                out!("{}: {e}", name(f));
                code = ExitCode::from(1);
            }
        }
    }
    Ok(code)
}

fn formats(tk: &Toolkit, from: Option<&str>, supported: bool) -> Result<ExitCode> {
    if let Some(f) = from {
        let id = target(tk, f)?;
        let reach = tk.graph.reach(id);
        out!(
            "{} can become {} formats:",
            tk.registry.get(id).name,
            reach.len()
        );
        for t in reach.iter() {
            let r = tk.plan(id, t, Preset::Balanced, 1_000_000);
            let lossless = r.as_ref().is_some_and(|r| r.is_lossless());
            let hops = r.map_or(0, |r| r.edges.len());
            out!(
                "  {:<8} {:<22} {}{}",
                tk.registry.primary_ext(t),
                tk.registry.get(t).name,
                if lossless { "lossless" } else { "lossy" },
                if hops > 1 {
                    format!(", {hops} steps")
                } else {
                    String::new()
                }
            );
        }
        return Ok(ExitCode::SUCCESS);
    }
    let cats = [
        Category::Image,
        Category::Video,
        Category::Audio,
        Category::Pdf,
        Category::Document,
        Category::Slides,
        Category::Spreadsheet,
        Category::Data,
        Category::Ebook,
        Category::Subtitle,
        Category::Archive,
        Category::Font,
        Category::Text,
    ];
    let mut pairs = 0;
    for c in cats {
        let list: Vec<String> = tk
            .registry
            .in_category(c)
            .filter(|(id, _)| !supported || !tk.graph.reach(*id).is_empty())
            .map(|(id, d)| {
                pairs += tk.graph.reach(id).len();
                if tk.graph.reach(id).is_empty() {
                    format!("{}*", d.key)
                } else {
                    d.key.to_string()
                }
            })
            .collect();
        if !list.is_empty() {
            out!("{:<12} {}", c.name(), list.join(" "));
        }
    }
    out!(
        "\n{pairs} conversion options with the installed engines. * = detected, but needs an engine pack to convert."
    );
    Ok(ExitCode::SUCCESS)
}

fn wheel(
    tk: &Toolkit,
    files: &[PathBuf],
    png: Option<PathBuf>,
    hover: usize,
    theme: ThemeArg,
    scale: f32,
    presets: bool,
) -> Result<ExitCode> {
    let mut formats = Vec::new();
    let mut total = 0u64;
    for f in files {
        let d = tk
            .detect(f)?
            .with_context(|| format!("{}: unrecognized file type", f.display()))?;
        if !formats.contains(&d.format) {
            formats.push(d.format);
        }
        total += std::fs::metadata(f).map_or(0, |m| m.len());
    }
    let frecency = load_frecency();
    let (items, more) = wheel_items(tk, &formats, &frecency, now_ms());
    if items.is_empty() {
        out!(
            "No conversions for these files with the installed engines (Tools and Actions would still be available)."
        );
        return Ok(ExitCode::SUCCESS);
    }
    let hours = ["12", "1:30", "3", "4:30", "6", "7:30", "9", "10:30"];
    let n = items.len() + usize::from(more > 0);
    for (i, it) in items.iter().enumerate() {
        let pos = if n == 8 { hours[i] } else { "" };
        out!(
            "  slot {i} {pos:>5}  {:<8} {:<10} {}",
            it.label,
            it.caption,
            it.route
        );
    }
    if more > 0 {
        out!("  slot 7 10:30  More…   {more} more");
    }
    if let Some(path) = png {
        let mut wedges: Vec<swirvo_render::Wedge> = items
            .iter()
            .map(|i| swirvo_render::Wedge {
                label: i.label.clone(),
                caption: i.caption.clone(),
            })
            .collect();
        if more > 0 {
            wedges.push(swirvo_render::Wedge {
                label: "More…".into(),
                caption: format!("{more} more"),
            });
        }
        let title = if files.len() == 1 {
            name(&files[0])
        } else {
            format!("{} files", files.len())
        };
        let title = if title.chars().count() > 14 {
            format!("{}…", title.chars().take(12).collect::<String>())
        } else {
            title
        };
        let scene = swirvo_render::Scene {
            wedges,
            hover: Some(hover.min(n - 1)),
            wheel: 0,
            hub_title: title,
            hub_subtitle: human(total),
            theme: match theme {
                ThemeArg::Light => swirvo_render::Theme::Light,
                ThemeArg::Dark => swirvo_render::Theme::Dark,
            },
            scale,
            sub: presets.then(|| swirvo_render::Sub {
                parent: hover.min(n - 1),
                items: ["Best", "Balanced", "Smallest"].map(String::from).to_vec(),
                hover: Some(1),
            }),
            hit_radius: 0.0,
        };
        let font = swirvo_render::load_ui_font();
        if font.is_none() {
            eprintln!("note: no UI font found (set SWIRVO_FONT); drawing shapes only");
        }
        let pm =
            swirvo_render::render(&scene, font.as_ref()).context("couldn't allocate the canvas")?;
        pm.save_png(&path)
            .with_context(|| format!("couldn't write {}", path.display()))?;
        out!("saved {}", path.display());
    }
    Ok(ExitCode::SUCCESS)
}
