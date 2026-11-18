//! Page-one previews of Office and OpenDocument files (Board thumbnails, docs/12 §3.4):
//! the first page is laid out by our own engines into a one-page PDF in memory and
//! rendered like any PDF page. Nothing is written to disk.

use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use swirvo_core::{CancelToken, EngineError};

use crate::pdf_tools::PageRenderer;

/// Format keys with a page-one preview.
pub const KINDS: &[&str] = &["docx", "odt", "rtf", "pptx", "xlsx"];

/// Files bigger than this aren't previewed: laying out a huge document for a thumbnail
/// isn't worth the time.
const MAX_BYTES: u64 = 40 << 20;

/// The first page of a `kind` file (one of [`KINDS`]) as a one-page PDF.
pub fn first_page_pdf(
    bytes: &[u8],
    kind: &str,
    cancel: &CancelToken,
) -> Result<Vec<u8>, EngineError> {
    match kind {
        "docx" | "odt" | "rtf" => crate::docxlayout::first_page(bytes, kind, cancel),
        "pptx" => crate::pptxlayout::first_page(bytes, cancel),
        "xlsx" => crate::xlsxlayout::first_page(bytes, cancel),
        other => Err(EngineError::Invalid(format!(
            "no preview for {other} files"
        ))),
    }
}

/// Page one of `path` rendered to fit `max` × `max` pixels, or an error after `limit`.
/// The layout runs on its own thread: when it takes too long the caller gets an error
/// at once, and the thread is cancelled (engines that check the token stop early;
/// the rest finish in the background and their result is dropped).
pub fn first_page(
    path: &Path,
    kind: &str,
    max: u32,
    limit: Duration,
) -> Result<image::RgbaImage, EngineError> {
    if std::fs::metadata(path)?.len() > MAX_BYTES {
        return Err(EngineError::Invalid("too big to preview".into()));
    }
    let bytes = std::fs::read(path)?;
    let kind = kind.to_string();
    let cancel = Arc::new(CancelToken::new());
    let (tx, rx) = std::sync::mpsc::channel();
    let c = cancel.clone();
    std::thread::Builder::new()
        .name("preview".into())
        .spawn(move || {
            let r = first_page_pdf(&bytes, &kind, &c).and_then(|pdf| {
                let r = PageRenderer::from_bytes(pdf)?;
                r.session(|s| s.render_fit(0, max, max))
            });
            let _ = tx.send(r);
        })?;
    match rx.recv_timeout(limit) {
        Ok(r) => r,
        Err(RecvTimeoutError::Timeout) => {
            cancel.cancel();
            Err(EngineError::Invalid("the preview took too long".into()))
        }
        Err(RecvTimeoutError::Disconnected) => {
            Err(EngineError::Encode("couldn't preview this file".into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_page_one_of_a_word_document() {
        if !crate::docxlayout::fonts::Fonts::available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let docx = d.path().join("a.docx");
        std::fs::write(&docx, crate::docxlayout::tests::sample_docx()).unwrap();
        let img = first_page(&docx, "docx", 200, Duration::from_secs(60)).unwrap();
        let (w, h) = img.dimensions();
        assert!(w.max(h) == 200 && h > w, "a portrait page: {w}×{h}");
        let ink = img.pixels().filter(|p| p[0] < 128).count();
        assert!(ink > 50, "something is drawn on it ({ink} dark pixels)");
        // Not a document we preview, and a cap that can't be met.
        assert!(first_page(&docx, "md", 200, Duration::from_secs(5)).is_err());
        assert!(first_page(&docx, "docx", 200, Duration::from_nanos(1)).is_err());
    }
}
