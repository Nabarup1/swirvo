//! Running the Word tools (docs/17 §8) on files: options in, new files out next to the
//! originals (never replacing anything).

use std::path::{Path, PathBuf};

use swirvo_core::{CancelToken, EngineError, PdfOptions};
use swirvo_engines::docxtools;

use crate::Toolkit;
use crate::args::{Field, ToolArgs, Values};
use crate::tools::{Tool, ToolError, ToolResult, emit, human, into_folder, name_of, parent_of};

/// Data files a mail merge reads.
pub(crate) const DATA: &[&str] = &["csv", "tsv", "xlsx", "xls", "ods"];

/// The options each Word tool takes.
pub fn fields(tool: Tool) -> &'static [Field] {
    static COMPRESS: &[Field] = &[Field::choice(
        "level",
        "Quality",
        &[
            ("balanced", "Screen · 150 dpi"),
            ("light", "Print · 220 dpi"),
            ("strong", "Email · 100 dpi"),
        ],
        "balanced",
    )];
    static CLEAN: &[Field] = &[Field::flag("accept", "Accept tracked changes", true)
        .hint("Off: keep tracked changes; author, comments and ids still go")];
    static REPLACE: &[Field] = &[
        Field::text("find", "Find", "").required(),
        Field::text("with", "Replace with", "").hint("Empty: delete what's found"),
        Field::flag("match_case", "Match case", false),
        Field::flag("whole_word", "Whole words only", false),
    ];
    static MERGE: &[Field] = &[
        Field::file("data", "Data (CSV or spreadsheet)")
            .hint("Or drop it together with the letter"),
        Field::text("name_field", "Name files by column", "").hint("e.g. Name (empty: numbered)"),
        Field::flag("pdf", "Also make PDFs", false),
    ];
    static PDFA: &[Field] = &[
        Field::choice(
            "standard",
            "Standard",
            &[
                ("a2", "PDF/A-2 (archival)"),
                ("a3", "PDF/A-3 (archival, original attached)"),
                ("ua", "PDF/UA (accessible)"),
            ],
            "a2",
        )
        .hint("PDF/A-3 keeps the original file inside the PDF. Checked against the standard as written; not a certification."),
        Field::flag("accessible", "Tagged for screen readers", true)
            .hint("Headings, lists, tables and picture descriptions (PDF/A-2a, 3a)"),
    ];
    match tool {
        Tool::OfficePdfA => PDFA,
        Tool::DocxCompress => COMPRESS,
        Tool::DocxClean => CLEAN,
        Tool::DocxReplace => REPLACE,
        Tool::DocxMailMerge => MERGE,
        _ => &[],
    }
}

/// Tools whose options must be filled in before they run.
pub fn asks(tool: Tool) -> bool {
    matches!(tool, Tool::DocxReplace | Tool::DocxMailMerge)
}

fn engine(e: EngineError) -> ToolError {
    ToolError::Engine(e)
}

fn key_of(tk: &Toolkit, p: &Path) -> Option<&'static str> {
    tk.registry.from_path(p).map(|id| tk.registry.key(id))
}

/// Run a Word tool. Per-file tools keep going after a failure and report it.
pub fn run(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    args: &ToolArgs,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let v = Values::new(fields(tool), args).map_err(ToolError::Invalid)?;
    if tool == Tool::DocxMailMerge {
        return merge(tk, files, &v, cancel);
    }
    let mut r = ToolResult::default();
    let mut lines = Vec::new();
    let mut note = None;
    let done = crate::tools::par_map(files, crate::tools::workers(files.len(), 6), cancel, |f| {
        one(tk, tool, f, &v, cancel)
    });
    cancel.check().map_err(engine)?;
    for (f, res) in files.iter().zip(done) {
        match res {
            Some(Ok((outs, msg, n))) => {
                r.outputs.extend(outs);
                lines.push(msg);
                note = note.or(n);
            }
            Some(Err(e)) => r.failures.push((f.clone(), e.to_string())),
            None => {}
        }
    }
    r.message = match lines.as_slice() {
        [only] => only.clone(),
        _ => {
            let (label, _) = tool.label();
            format!("{label}: {} of {} documents done", lines.len(), files.len())
        }
    };
    if tool == Tool::DocxStats && !lines.is_empty() {
        r.clipboard = Some(lines.join("\r\n"));
    }
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    Ok(r)
}

fn merge(
    tk: &Toolkit,
    files: &[PathBuf],
    v: &Values<'_>,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let tpl = files
        .iter()
        .find(|f| key_of(tk, f) == Some("docx"))
        .ok_or_else(|| ToolError::Invalid("drop the Word letter (.docx) to merge into".into()))?;
    let data: PathBuf = match v.opt("data") {
        Some(d) => PathBuf::from(d),
        None => files
            .iter()
            .find(|f| key_of(tk, f).is_some_and(|k| DATA.contains(&k)))
            .cloned()
            .ok_or_else(|| {
                ToolError::Invalid(
                    "pick the data file (CSV or spreadsheet) with a row per letter".into(),
                )
            })?,
    };
    let rows = docxtools::data_rows(&data).map_err(engine)?;
    let stem = tk.registry.stem(tpl);
    let (dir, made, note) = into_folder(tpl, &format!("{stem} (merged)"), |d| {
        docxtools::mail_merge(
            tpl,
            &rows,
            d,
            &stem,
            v.opt("name_field"),
            v.flag("pdf"),
            cancel,
        )
    })?;
    let docs = made
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "docx"))
        .count();
    let mut r = ToolResult {
        message: format!("{docs} letters → {}", name_of(&dir)),
        outputs: vec![dir],
        ..ToolResult::default()
    };
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    Ok(r)
}

type One = (Vec<PathBuf>, String, Option<String>);

/// One output file `stem (suffix).docx` written by `f`.
fn single(
    file: &Path,
    stem: &str,
    suffix: &str,
    f: impl FnOnce(&Path) -> Result<(), EngineError>,
) -> Result<(PathBuf, Option<String>), ToolError> {
    emit(&parent_of(file), &format!("{stem} ({suffix})"), "docx", f)
}

fn one(
    tk: &Toolkit,
    tool: Tool,
    file: &Path,
    v: &Values<'_>,
    cancel: &CancelToken,
) -> Result<One, ToolError> {
    let stem = tk.registry.stem(file);
    match tool {
        Tool::OfficePdfA => {
            let standard = v.str("standard");
            let ua = standard == "ua";
            let opts = PdfOptions {
                archival: !ua,
                // PDF/UA is always tagged.
                tagged: ua || v.flag("accessible"),
                attach_source: standard == "a3",
                accessible: ua,
            };
            let from = key_of(tk, file).unwrap_or("docx");
            let suffix = if ua { "PDF-UA" } else { "PDF-A" };
            let (out, n) = emit(
                &parent_of(file),
                &format!("{stem} ({suffix})"),
                "pdf",
                |tmp| {
                    let bytes = std::fs::read(file)?;
                    match from {
                        "pptx" => {
                            swirvo_engines::pptxlayout::convert_as(&bytes, &stem, tmp, opts, cancel)
                        }
                        "xlsx" => {
                            swirvo_engines::xlsxlayout::convert_as(&bytes, &stem, tmp, opts, cancel)
                        }
                        other => swirvo_engines::docxlayout::convert_as(
                            &bytes, other, &stem, tmp, opts, cancel,
                        ),
                    }
                    .map(|_| ())
                },
            )?;
            let written = std::fs::read(&out).ok();
            let level = written
                .as_deref()
                .and_then(swirvo_engines::docxlayout::pdfa_level);
            let attached = if opts.attach_source {
                ", original attached"
            } else {
                ""
            };
            let msg = match level.as_deref() {
                _ if ua => match written
                    .as_deref()
                    .and_then(swirvo_engines::docxlayout::pdfua_part)
                {
                    Some(p) => format!("PDF/UA-{p}, accessible → {}", name_of(&out)),
                    None => format!("PDF/UA → {}", name_of(&out)),
                },
                Some(l @ ("2A" | "3A")) => format!(
                    "PDF/A-{}, accessible{attached} → {}",
                    l.to_lowercase(),
                    name_of(&out)
                ),
                Some(l @ ("2B" | "3B")) if opts.tagged => format!(
                    "PDF/A-{}, tagged (the document doesn't say its language, so not {}a){attached} → {}",
                    l.to_lowercase(),
                    &l[..1],
                    name_of(&out)
                ),
                Some(l) => format!("PDF/A-{}{attached} → {}", l.to_lowercase(), name_of(&out)),
                None => format!("PDF/A → {}", name_of(&out)),
            };
            Ok((vec![out], msg, n))
        }
        Tool::DocxCompress => {
            let (dpi, q) = match v.str("level") {
                "light" => (220.0, 85),
                "strong" => (100.0, 60),
                _ => (150.0, 75),
            };
            let mut rep = docxtools::Shrunk::default();
            let (out, n) = single(file, &stem, "compressed", |tmp| {
                rep = docxtools::compress(file, tmp, dpi, q, cancel)?;
                if rep.after as f64 > rep.before as f64 * 0.98 {
                    return Err(EngineError::Invalid(
                        "it's already compact; a smaller copy would only look worse".into(),
                    ));
                }
                Ok(())
            })?;
            let pct = 100.0 - rep.after as f64 * 100.0 / rep.before.max(1) as f64;
            Ok((
                vec![out.clone()],
                format!(
                    "{} → {} (−{pct:.0}%) · {}",
                    human(rep.before),
                    human(rep.after),
                    name_of(&out)
                ),
                n,
            ))
        }
        Tool::DocxClean => {
            let mut done = docxtools::Cleaned::default();
            let accept = v.flag("accept");
            let (out, n) = single(file, &stem, "clean", |tmp| {
                done = docxtools::clean(file, tmp, accept)?;
                Ok(())
            })?;
            let what = if done.items.is_empty() {
                "nothing personal was in it".to_string()
            } else {
                format!("removed {}", done.items.join(", "))
            };
            Ok((
                vec![out.clone()],
                format!("Cleaned ({what}) → {}", name_of(&out)),
                n,
            ))
        }
        Tool::DocxReplace => {
            let mut count = 0;
            let (out, n) = single(file, &stem, "edited", |tmp| {
                count = docxtools::replace(
                    file,
                    tmp,
                    v.str("find"),
                    v.str("with"),
                    v.flag("match_case"),
                    v.flag("whole_word"),
                )?;
                if count == 0 {
                    return Err(EngineError::Invalid(format!(
                        "“{}” isn't in the document",
                        v.str("find")
                    )));
                }
                Ok(())
            })?;
            let times = if count == 1 { "once" } else { "times" };
            let msg = if count == 1 {
                format!("Replaced {times} → {}", name_of(&out))
            } else {
                format!("Replaced {count} {times} → {}", name_of(&out))
            };
            Ok((vec![out], msg, n))
        }
        Tool::DocxPictures => {
            let (dir, made, n) = into_folder(file, &format!("{stem} (pictures)"), |d| {
                let got = docxtools::extract_images(file, d, &stem)?;
                if got.is_empty() {
                    return Err(EngineError::Invalid("it has no pictures".into()));
                }
                Ok(got)
            })?;
            Ok((
                vec![dir.clone()],
                format!("{} pictures → {}", made.len(), name_of(&dir)),
                n,
            ))
        }
        Tool::DocxStats => {
            let s = docxtools::stats(file).map_err(engine)?;
            Ok((
                Vec::new(),
                format!(
                    "{}: {} words · {} characters ({} without spaces) · {} paragraphs · {} pages",
                    name_of(file),
                    s.words,
                    s.chars,
                    s.chars_no_spaces,
                    s.paragraphs,
                    s.pages
                ),
                None,
            ))
        }
        other => Err(ToolError::Invalid(format!(
            "{} isn't a DOCX tool",
            other.id()
        ))),
    }
}
