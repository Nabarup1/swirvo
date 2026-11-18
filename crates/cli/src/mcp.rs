//! `swirvo mcp`: a Model Context Protocol server on stdin/stdout, so AI assistants
//! (Claude Desktop, Claude Code, Cursor, VS Code and other MCP clients) can convert files,
//! run Swirvo's tools and read text out of documents, all on this computer.
//!
//! The transport is MCP's stdio transport: one JSON-RPC 2.0 message per line. stdout carries
//! protocol messages only; anything for a human goes to stderr. Like everything in Swirvo,
//! results are new files next to the originals (or in `out_dir`); originals are never
//! changed. Tool calls run one at a time, and a panic in an engine fails only that call.

use std::io::{BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use swirvo_core::{CancelToken, Category, Preset};
use swirvo_jobs::args::{FieldKind, ToolArgs};
use swirvo_jobs::tools::{PageOp, Tool, run_page_op, run_tool_with, tools_for};
use swirvo_jobs::{JobEvent, Options, Toolkit};

/// Protocol versions we speak, newest first. We answer with the client's version when we
/// know it, else with our newest (the client then decides whether it can continue).
const VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Longest text returned by `extract_text` per file, in characters.
const TEXT_LIMIT: usize = 200_000;

const INSTRUCTIONS: &str = "Swirvo converts and edits files locally (nothing is uploaded). \
Pass absolute file paths. Results are always written as new files next to the originals \
(or into out_dir); originals are never modified, so it is safe to retry. \
Use list_formats to see targets, list_tools to see which tools apply to given files and \
their options, and extract_text to read text from PDFs, Word files and images (OCR on Windows).";

/// Serve until stdin closes.
pub fn serve(tk: &Toolkit) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle(tk, m)).collect();
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            Ok(msg) => handle(tk, &msg),
            Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
        };
        if let Some(r) = reply {
            serde_json::to_writer(&mut stdout, &r)?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// One message in, at most one reply out (notifications get none).
fn handle(tk: &Toolkit, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str);
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let (Some(id), Some(method)) = (id, method) else {
        // A notification (no id) or a response to something we never sent: nothing to say.
        return None;
    };
    let result = match method {
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list() })),
        "tools/call" => call(tk, &params),
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, m)) => error(id, code, &m),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| VERSIONS.contains(v))
        .unwrap_or(VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "swirvo",
            "title": "Swirvo",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

fn paths_schema(what: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "description": what })
}

fn tool_list() -> Value {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let writes = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false });
    json!([
        {
            "name": "list_formats",
            "title": "List formats",
            "description": "List the file formats Swirvo knows, by category, or (with `from`) every format a given format can be converted to, and whether that conversion is lossless.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": { "type": "string", "description": "A format key or extension, e.g. \"pdf\", \"heic\", \"docx\"." }
                }
            },
            "annotations": read_only,
        },
        {
            "name": "detect_format",
            "title": "Detect file types",
            "description": "Detect each file's real format from its content (not its extension), and flag files whose extension lies.",
            "inputSchema": {
                "type": "object",
                "properties": { "paths": paths_schema("Absolute paths of the files.") },
                "required": ["paths"]
            },
            "annotations": read_only,
        },
        {
            "name": "convert_files",
            "title": "Convert files",
            "description": "Convert files to another format, e.g. PDF to DOCX (keeps the layout), DOCX/XLSX/PPTX to PDF, HEIC to JPG, PNG to WebP, CSV to XLSX, Markdown to HTML, JSON to YAML. Runs in parallel. Each result is a new file next to its original (or in out_dir); originals are never changed. Returns the output path of each file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": paths_schema("Absolute paths of the files to convert."),
                    "to": { "type": "string", "description": "Target format key or extension, e.g. \"pdf\", \"docx\", \"jpg\", \"webp\", \"xlsx\", \"md\"." },
                    "preset": { "type": "string", "enum": ["fastest", "balanced", "best", "smallest"], "default": "balanced" },
                    "out_dir": { "type": "string", "description": "Optional folder for the results (created if missing)." },
                    "pdf_profile": { "type": "string", "enum": ["standard", "tagged", "pdfa-2", "pdfa-3", "pdfua"], "default": "standard", "description": "For PDF output: tagged (accessible), PDF/A-2 or PDF/A-3 (archival; A-3 attaches the original), or PDF/UA." }
                },
                "required": ["paths", "to"]
            },
            "annotations": writes,
        },
        {
            "name": "list_tools",
            "title": "List tools for files",
            "description": "List the tools that apply to these files (for PDFs: merge, split, compress, watermark, page numbers, protect, unlock, crop, OCR, redact, sign, compare, PDF/A and more; for images, Word files, videos and archives their own tools), with each tool's options.",
            "inputSchema": {
                "type": "object",
                "properties": { "paths": paths_schema("Absolute paths of the files.") },
                "required": ["paths"]
            },
            "annotations": read_only,
        },
        {
            "name": "run_tool",
            "title": "Run a tool",
            "description": "Run one Swirvo tool on files, e.g. pdf-merge, pdf-compress, pdf-watermark, pdf-numbers, pdf-protect, pdf-redact, pdf-searchable, pdf-split-ranges, docx-replace, resize-half, compress, checksum. Use list_tools first to get the tool ids and option keys. Results are new files; originals are never changed.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "tool": { "type": "string", "description": "Tool id from list_tools, e.g. \"pdf-merge\"." },
                    "paths": paths_schema("Absolute paths of the input files (order matters for merge)."),
                    "options": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean"] }, "description": "Tool options by key, from list_tools, e.g. {\"text\": \"DRAFT\"} or {\"ranges\": \"1-3; 4-9\"}." },
                    "pages": { "type": "string", "description": "PDF page tools only (pdf-pages, pdf-split, pdf-rotate, pdf-to-png, pdf-to-jpg): which pages, e.g. \"1-3, 7, 10-\", \"odd\", \"even\". One PDF at a time." },
                    "page_action": { "type": "string", "enum": ["extract", "split", "rotate", "delete", "images-png", "images-jpg"], "description": "With `pages`: what to do with them (default: the tool's own action)." }
                },
                "required": ["tool", "paths"]
            },
            "annotations": writes,
        },
        {
            "name": "extract_text",
            "title": "Extract text",
            "description": "Read the text of PDFs, Word/ODT/RTF files, slides, spreadsheets, Markdown, HTML and images, and return it. Scanned PDFs and photos of text are read with the OCR engine built into Windows. Nothing is written to disk.",
            "inputSchema": {
                "type": "object",
                "properties": { "paths": paths_schema("Absolute paths of the files.") },
                "required": ["paths"]
            },
            "annotations": read_only,
        }
    ])
}

type CallResult = Result<Value, (i64, String)>;

fn call(tk: &Toolkit, params: &Value) -> CallResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((-32602, "tools/call needs a tool name".to_string()))?;
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let run = || -> Result<Value, String> {
        match name {
            "list_formats" => list_formats(tk, str_arg(&args, "from")),
            "detect_format" => detect(tk, &paths(&args)?),
            "convert_files" => convert(tk, &args),
            "list_tools" => list_tools(tk, &paths(&args)?),
            "run_tool" => run_one(tk, &args),
            "extract_text" => extract_text(tk, &paths(&args)?),
            other => Err(format!("unknown tool '{other}'")),
        }
    };
    if !matches!(
        name,
        "list_formats"
            | "detect_format"
            | "convert_files"
            | "list_tools"
            | "run_tool"
            | "extract_text"
    ) {
        return Err((-32602, format!("unknown tool: {name}")));
    }
    // An engine bug must not take the server (and the client's session) down with it.
    let outcome = catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|p| {
        let what = p
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown error".into());
        Err(format!(
            "Swirvo hit an internal error on this input: {what}"
        ))
    });
    Ok(match outcome {
        Ok(v) => json!({
            "content": [{ "type": "text", "text": v.as_str().map_or_else(|| pretty(&v), str::to_owned) }],
            "isError": false,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": e }],
            "isError": true,
        }),
    })
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
}

fn paths(args: &Value) -> Result<Vec<PathBuf>, String> {
    let list = args
        .get("paths")
        .and_then(Value::as_array)
        .ok_or("`paths` must be a list of file paths")?;
    let mut out = Vec::new();
    for p in list {
        let p = p.as_str().ok_or("every path must be a string")?;
        let path = PathBuf::from(p);
        if !path.exists() {
            return Err(format!("{p}: no such file"));
        }
        out.push(path);
    }
    if out.is_empty() {
        return Err("give at least one path".into());
    }
    Ok(out)
}

fn list_formats(tk: &Toolkit, from: Option<&str>) -> Result<Value, String> {
    if let Some(f) = from {
        let id = tk
            .registry
            .find(f)
            .ok_or_else(|| format!("unknown format '{f}'"))?;
        let targets: Vec<Value> = tk
            .graph
            .reach(id)
            .iter()
            .map(|t| {
                let r = tk.plan(id, t, Preset::Balanced, 1_000_000);
                json!({
                    "key": tk.registry.key(t),
                    "name": tk.registry.get(t).name,
                    "lossless": r.as_ref().is_some_and(|r| r.is_lossless()),
                    "steps": r.map_or(0, |r| r.edges.len()),
                })
            })
            .collect();
        return Ok(json!({ "from": tk.registry.key(id), "can_become": targets }));
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
    let mut by_cat = Map::new();
    for c in cats {
        let list: Vec<Value> = tk
            .registry
            .in_category(c)
            .map(|(id, d)| {
                json!({ "key": d.key, "name": d.name, "convertible": !tk.graph.reach(id).is_empty() })
            })
            .collect();
        if !list.is_empty() {
            by_cat.insert(c.name().to_string(), Value::Array(list));
        }
    }
    Ok(Value::Object(by_cat))
}

fn detect(tk: &Toolkit, files: &[PathBuf]) -> Result<Value, String> {
    let out: Vec<Value> = files
        .iter()
        .map(|f| match tk.detect(f) {
            Ok(Some(d)) => {
                let def = tk.registry.get(d.format);
                json!({
                    "path": f,
                    "format": def.key,
                    "name": def.name,
                    "category": def.category.name(),
                    "mime": def.mime,
                    "extension_says": d.ext_guess.map(|e| tk.registry.key(e)),
                    "extension_mismatch": d.mismatch,
                })
            }
            Ok(None) => json!({ "path": f, "format": null, "error": "unknown file type" }),
            Err(e) => json!({ "path": f, "format": null, "error": e.to_string() }),
        })
        .collect();
    Ok(Value::Array(out))
}

fn convert(tk: &Toolkit, args: &Value) -> Result<Value, String> {
    let files = paths(args)?;
    let to = str_arg(args, "to").ok_or("`to` is required, e.g. \"pdf\"")?;
    let to_id = tk
        .registry
        .find(to)
        .ok_or_else(|| format!("unknown format '{to}'. Use list_formats."))?;
    let preset = str_arg(args, "preset")
        .unwrap_or("balanced")
        .parse::<Preset>()?;
    let profile = str_arg(args, "pdf_profile").unwrap_or("standard");
    let pdf = match profile {
        "standard" => swirvo_core::PdfOptions::default(),
        "tagged" => swirvo_core::PdfOptions {
            tagged: true,
            ..Default::default()
        },
        "pdfa-2" => swirvo_core::PdfOptions {
            archival: true,
            ..Default::default()
        },
        "pdfa-3" => swirvo_core::PdfOptions {
            archival: true,
            attach_source: true,
            ..Default::default()
        },
        "pdfua" => swirvo_core::PdfOptions {
            tagged: true,
            accessible: true,
            ..Default::default()
        },
        other => return Err(format!("unknown pdf_profile '{other}'")),
    };
    let out_dir = str_arg(args, "out_dir").map(PathBuf::from);
    if let Some(o) = &out_dir {
        std::fs::create_dir_all(o).map_err(|e| format!("can't create {}: {e}", o.display()))?;
    }
    let opts = Options {
        preset,
        pdf,
        out_dir,
        ..Options::default()
    };
    let events = Mutex::new(Vec::new());
    let summary = swirvo_jobs::convert(tk, &files, to_id, &opts, &CancelToken::new(), &|ev| {
        let row = match ev {
            JobEvent::Done {
                input,
                output,
                bytes_in,
                bytes_out,
                lossless,
                note,
                ..
            } => json!({
                "input": input, "status": "done", "output": output,
                "bytes_in": bytes_in, "bytes_out": bytes_out, "lossless": lossless, "note": note,
            }),
            JobEvent::Failed { input, error, .. } => {
                json!({ "input": input, "status": "failed", "error": error })
            }
            JobEvent::Skipped { input, reason, .. } => {
                json!({ "input": input, "status": "skipped", "reason": reason })
            }
        };
        events.lock().unwrap_or_else(|p| p.into_inner()).push(row);
    });
    let results = events.into_inner().unwrap_or_else(|p| p.into_inner());
    if summary.done == 0 && summary.failed > 0 {
        return Err(pretty(
            &json!({ "error": "nothing was converted", "results": results }),
        ));
    }
    Ok(json!({
        "converted": summary.done,
        "failed": summary.failed,
        "skipped": summary.skipped,
        "results": results,
    }))
}

fn list_tools(tk: &Toolkit, files: &[PathBuf]) -> Result<Value, String> {
    let tools: Vec<Value> = tools_for(tk, files)
        .into_iter()
        .map(|t| {
            let (label, caption) = t.label();
            let options: Vec<Value> = t
                .fields()
                .iter()
                .map(|f| {
                    let (kind, choices) = match f.kind {
                        FieldKind::Choice(c) => {
                            ("choice", Some(c.iter().map(|o| o.0).collect::<Vec<_>>()))
                        }
                        FieldKind::Number { .. } => ("number", None),
                        FieldKind::Flag => ("true|false", None),
                        FieldKind::File => ("file path", None),
                        FieldKind::Secret => ("secret text", None),
                        FieldKind::Text => ("text", None),
                        FieldKind::Pages => ("pages, e.g. 1-3, 7", None),
                        FieldKind::PageGroups => ("page groups, e.g. 1-3, 5; 6-9", None),
                    };
                    json!({
                        "key": f.key, "label": f.label, "type": kind, "choices": choices,
                        "default": (!f.default.is_empty()).then_some(f.default),
                        "required": f.required,
                        "hint": (!f.hint.is_empty()).then_some(f.hint),
                    })
                })
                .collect();
            json!({
                "id": t.id(),
                "name": label,
                "what": caption,
                "page_tool": t.page_op().is_some(),
                "options": options,
            })
        })
        .collect();
    Ok(Value::Array(tools))
}

fn run_one(tk: &Toolkit, args: &Value) -> Result<Value, String> {
    let name = str_arg(args, "tool").ok_or("`tool` is required; see list_tools")?;
    let tool =
        Tool::from_id(name).ok_or_else(|| format!("unknown tool '{name}'; see list_tools"))?;
    let files = paths(args)?;
    let cancel = CancelToken::new();
    let result = if let Some(spec) = str_arg(args, "pages") {
        let [file] = files.as_slice() else {
            return Err("`pages` works on one PDF at a time".into());
        };
        let default = tool
            .page_op()
            .ok_or_else(|| format!("'{name}' isn't a PDF page tool; drop `pages`"))?;
        let op = match str_arg(args, "page_action") {
            None => default,
            Some("images-jpg") => PageOp::Images {
                png: false,
                dpi: 150,
            },
            Some("images-png") => PageOp::Images {
                png: true,
                dpi: 150,
            },
            Some("extract") => PageOp::Extract,
            Some("split") => PageOp::Split,
            Some("rotate") => PageOp::Rotate,
            Some("delete") => PageOp::Delete,
            Some(o) => return Err(format!("unknown page_action '{o}'")),
        };
        let count = swirvo_jobs::tools::pdf_page_count(file)
            .ok_or_else(|| format!("{}: can't read this PDF", file.display()))?;
        let pages = swirvo_jobs::pages::parse(spec, count)?;
        run_page_op(tk, file, op, &pages, &cancel).map_err(|e| e.to_string())?
    } else {
        let mut opts = ToolArgs::new();
        if let Some(o) = args.get("options").and_then(Value::as_object) {
            for (k, v) in o {
                let v = match v {
                    Value::String(s) => s.clone(),
                    Value::Null => continue,
                    other => other.to_string(),
                };
                opts.insert(k.clone(), v);
            }
        }
        run_tool_with(tk, tool, &files, &opts, &cancel).map_err(|e| e.to_string())?
    };
    let mut v = json!({
        "message": result.message,
        "outputs": result.outputs,
    });
    if let Some(c) = &result.clipboard {
        v["text"] = Value::String(c.clone());
    }
    if !result.failures.is_empty() {
        v["failures"] = json!(
            result
                .failures
                .iter()
                .map(|(f, e)| json!({ "path": f, "error": e }))
                .collect::<Vec<_>>()
        );
    }
    Ok(v)
}

fn extract_text(tk: &Toolkit, files: &[PathBuf]) -> Result<Value, String> {
    let mut parts = Vec::new();
    let mut any = false;
    for f in files {
        let r = run_tool_with(
            tk,
            Tool::CopyText,
            std::slice::from_ref(f),
            &ToolArgs::new(),
            &CancelToken::new(),
        );
        let head = format!("===== {} =====", f.display());
        match r {
            Ok(r) => match r.clipboard {
                Some(text) => {
                    any = true;
                    let mut text = text;
                    if text.chars().count() > TEXT_LIMIT {
                        text = text.chars().take(TEXT_LIMIT).collect();
                        text.push_str("\n[… cut: the text is longer than 200,000 characters]");
                    }
                    parts.push(format!("{head}\n{text}"));
                }
                None => parts.push(format!("{head}\n[no text: {}]", r.message)),
            },
            Err(e) => parts.push(format!("{head}\n[error: {e}]")),
        }
    }
    let text = parts.join("\n\n");
    if any {
        Ok(Value::String(text))
    } else {
        Err(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tk() -> Toolkit {
        Toolkit::new().expect("toolkit")
    }

    #[test]
    fn handshake_and_listing() {
        let tk = tk();
        let r = handle(
            &tk,
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        )
        .expect("reply");
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "swirvo");
        // Notifications get no reply.
        assert!(
            handle(
                &tk,
                &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
            )
            .is_none()
        );
        let r = handle(&tk, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).expect("reply");
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .map(|t| t["name"].as_str().expect("name"))
            .collect();
        assert!(names.contains(&"convert_files") && names.contains(&"extract_text"));
        let r = handle(&tk, &json!({"jsonrpc":"2.0","id":3,"method":"nope"})).expect("reply");
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn convert_and_text_through_the_protocol() {
        let tk = tk();
        let dir = tempfile::tempdir().expect("tempdir");
        let md = dir.path().join("notes.md");
        std::fs::write(&md, "# Hello\n\nSwirvo sample text.\n").expect("write");
        let r = handle(
            &tk,
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"convert_files","arguments":{"paths":[md],"to":"html"}}}),
        )
        .expect("reply");
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(dir.path().join("notes.html").is_file());
        assert!(md.is_file(), "the original stays");

        let r = handle(
            &tk,
            &json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"extract_text","arguments":{"paths":[md]}}}),
        )
        .expect("reply");
        let text = r["result"]["content"][0]["text"].as_str().expect("text");
        assert!(text.contains("Swirvo sample text"), "{text}");

        // A tool error is a result with isError, not a protocol error.
        let r = handle(
            &tk,
            &json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"convert_files","arguments":{"paths":["/no/such/file.png"],"to":"jpg"}}}),
        )
        .expect("reply");
        assert_eq!(r["result"]["isError"], true);
    }
}
