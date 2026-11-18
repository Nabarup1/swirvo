//! The page picker's Organise mode as a job: a page plan with turns, crops, redaction
//! boxes, signatures and comments becomes one new PDF next to the original.

use std::path::PathBuf;

use swirvo_core::CancelToken;
use swirvo_engines::pdfedit::assemble::PageSrc;
use swirvo_engines::pdfedit::organise::{self, Mark, Sheet};
use swirvo_engines::pdfedit::secure::{self, Need};

use crate::Toolkit;
use crate::tools::{ToolError, ToolResult, count, emit, name_of, parent_of};

/// Write `inputs[0]` organised as `sheets` with `marks` to "<name> (organised).pdf".
pub fn run_organise(
    tk: &Toolkit,
    inputs: &[PathBuf],
    sheets: &[Sheet],
    marks: &[Mark],
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let first = inputs
        .first()
        .ok_or_else(|| ToolError::Invalid("no PDF to organise".into()))?;
    let n0 = swirvo_engines::pdf_tools::page_count(first)?;
    let in_place = sheets.len() == n0
        && sheets
            .iter()
            .enumerate()
            .all(|(i, s)| s.src == PageSrc::Page { file: 0, page: i });
    // Re-embedding can't carry protection over; editing in place keeps it (docs/17 §4).
    let redacts = marks.iter().any(|m| matches!(m, Mark::Redact { .. }));
    for (k, f) in inputs.iter().enumerate() {
        let used = sheets
            .iter()
            .any(|s| matches!(s.src, PageSrc::Page { file, .. } if file == k));
        if !used {
            continue;
        }
        let need = if in_place {
            Need::Edit
        } else {
            Need::Everything
        };
        secure::permits(f, need)?;
        if redacts {
            // Redaction rebuilds pages from pixels: a copy of their content.
            secure::permits(f, Need::Copy)?;
        }
    }
    let stem = tk.registry.stem(first);
    let mut rep = organise::Organised::default();
    let (out, note) = emit(
        &parent_of(first),
        &format!("{stem} (organised)"),
        "pdf",
        |tmp| {
            rep = organise::organise(inputs, sheets, marks, tmp, cancel)?;
            Ok(())
        },
    )?;
    let mut parts = vec![count(rep.pages, "page")];
    if rep.redacted > 0 {
        parts.push(format!("{} redacted", count(rep.redacted, "page")));
    }
    if rep.pictures > 0 {
        parts.push(count(rep.pictures, "picture"));
    }
    let comments = rep.comments + rep.stamped_text;
    if comments > 0 {
        parts.push(count(comments, "comment"));
    }
    let mut message = format!("{} → {}", parts.join(", "), name_of(&out));
    if rep.rebuilt {
        message.push_str(" (links and form fields aren't kept when pages are rearranged)");
    }
    if let Some(n) = note {
        message = format!("{message} ({n})");
    }
    Ok(ToolResult {
        outputs: vec![out],
        message,
        ..ToolResult::default()
    })
}
