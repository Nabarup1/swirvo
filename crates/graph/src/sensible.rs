//! Which conversions make sense to offer.
//!
//! The graph can chain engines freely, which finds good multi-step routes (HEIC → PNG →
//! WebP) but also absurd ones: a video's first GIF frame run through OCR into a Word
//! document, or JSON typeset as a PDF. This table says which *final* targets are worth
//! offering for a source, whatever the route. Intermediate steps are unrestricted, so
//! HEIC → PNG → JPG still works.

use swirvo_core::Category as C;
use swirvo_formats::FormatDef;

/// Photos and screenshots that can contain text worth recognising (OCR).
const OCR_SOURCES: &[&str] = &[
    "jpg", "png", "webp", "bmp", "tiff", "gif", "heic", "avif", "jxl",
];

/// Is `to` a sensible thing to turn `from` into?
pub fn sensible(from: &FormatDef, to: &FormatDef) -> bool {
    let k = to.key;
    match from.category {
        // Images become images or a PDF; photos of pages can also become text.
        C::Image => {
            matches!(to.category, C::Image | C::Pdf)
                || (OCR_SOURCES.contains(&from.key) && matches!(k, "txt" | "docx"))
        }
        // Video becomes video, its soundtrack, or an animated GIF. Stills are a tool.
        C::Video => matches!(to.category, C::Video | C::Audio) || k == "gif",
        C::Audio => to.category == C::Audio,
        // A PDF becomes an editable document, an e-book, its text, or its tables.
        C::Pdf => {
            matches!(
                to.category,
                C::Document | C::Ebook | C::Text | C::Spreadsheet
            ) || k == "csv"
        }
        C::Document | C::Ebook | C::Text => {
            matches!(to.category, C::Document | C::Pdf | C::Ebook | C::Text)
        }
        // Slides become slides, a PDF (handouts) or their text.
        C::Slides => matches!(to.category, C::Slides | C::Pdf | C::Text),
        // A table becomes another table format, data, a PDF, or a document with a table.
        C::Spreadsheet => {
            matches!(to.category, C::Spreadsheet | C::Data | C::Pdf | C::Text)
                || (to.category == C::Document && matches!(k, "docx" | "odt" | "html" | "md"))
        }
        C::Data => matches!(to.category, C::Data | C::Spreadsheet),
        // Subtitles become other subtitles or a plain transcript.
        C::Subtitle => to.category == C::Subtitle || k == "txt",
        C::Archive => to.category == C::Archive,
        C::Font => to.category == C::Font,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_formats::registry;

    fn ok(a: &str, b: &str) -> bool {
        let r = registry();
        sensible(r.get(r.id(a).unwrap()), r.get(r.id(b).unwrap()))
    }

    #[test]
    fn offers_what_people_mean() {
        for (a, b) in [
            ("heic", "jpg"),
            ("png", "pdf"),
            ("jpg", "txt"),
            ("png", "docx"),
            ("mp4", "mp3"),
            ("mp4", "gif"),
            ("mov", "mp4"),
            ("pdf", "docx"),
            ("pdf", "txt"),
            ("pdf", "xlsx"),
            ("docx", "pdf"),
            ("md", "epub"),
            ("pptx", "pdf"),
            ("xlsx", "pdf"),
            ("xlsx", "docx"),
            ("csv", "json"),
            ("json", "xlsx"),
            ("srt", "txt"),
            ("7z", "zip"),
            ("ttf", "woff2"),
            ("txt", "docx"),
        ] {
            assert!(ok(a, b), "{a} → {b} should be offered");
        }
    }

    #[test]
    fn refuses_nonsense() {
        for (a, b) in [
            ("mp4", "docx"),
            ("mp4", "pdf"),
            ("mp4", "png"),
            ("wav", "txt"),
            ("svg", "docx"),
            ("dng", "txt"),
            ("jpg", "epub"),
            ("png", "md"),
            ("json", "pdf"),
            ("xml", "docx"),
            ("srt", "pdf"),
            ("pptx", "docx"),
            ("pptx", "epub"),
            ("xlsx", "epub"),
            ("zip", "pdf"),
            ("pdf", "png"),
        ] {
            assert!(!ok(a, b), "{a} → {b} should not be offered");
        }
    }
}
