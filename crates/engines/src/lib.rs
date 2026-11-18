//! Built-in engines. Each one declares capabilities (graph edges) and runs single steps.
//! Heavier engines (libvips, FFmpeg, LibreOffice, Pandoc, AI) arrive as packs (docs/03 §12).

pub mod archive;
pub mod audio;
pub mod chart;
pub mod data;
pub mod doc;
pub mod docxlayout;
pub mod docxtools;
pub mod emf;
pub mod external;
pub mod font;
pub mod image;
pub mod markdown;
pub mod ocr;
mod opc;
pub mod os;
pub mod pdf_tools;
pub mod pdfedit;
pub mod pdflayout;
mod pdfw;
pub mod pptxlayout;
pub mod preview;
pub mod psd;
pub mod raw;
pub mod sheet;
pub mod subtitles;
pub mod svg;
mod text;
pub mod video;
pub mod xlsxlayout;
mod xmldom;
pub mod xmljson;

use swirvo_core::Engine;

/// Decode text bytes (UTF-8, UTF-16 with BOM, or Windows-1252) into a `String`.
pub fn text_from_bytes(bytes: &[u8]) -> String {
    text::from_bytes(bytes)
}

/// Every built-in engine, plus engine packs found on this PC (FFmpeg, LibreOffice) and the
/// engines Windows/Linux already ship (bsdtar, WIC codecs, OCR).
pub fn all() -> Vec<Box<dyn Engine>> {
    all_with(Packs::default())
}

/// Which engine packs found on this PC may be used (the user's Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packs {
    pub office: bool,
    pub libreoffice: bool,
    pub ffmpeg: bool,
}

impl Default for Packs {
    fn default() -> Self {
        Packs {
            office: true,
            libreoffice: true,
            ffmpeg: true,
        }
    }
}

/// [`all`], leaving out the packs the user turned off.
pub fn all_with(packs: Packs) -> Vec<Box<dyn Engine>> {
    let mut engines = builtin();
    engines.extend(external::discover_with(packs));
    engines.extend(os::discover_with(packs));
    engines
}

/// What this PC has, for the Settings window: (FFmpeg, LibreOffice) program paths and
/// whether Word, Excel or PowerPoint is installed. Opens no files; may take a moment
/// (Office is probed through COM), so call it off the UI thread.
pub fn found_packs() -> (Option<std::path::PathBuf>, Option<std::path::PathBuf>, bool) {
    (
        external::ffmpeg_exe(),
        external::libreoffice_exe(),
        os::office_installed(),
    )
}

/// Only the engines compiled into this binary (deterministic; used by tests).
pub fn builtin() -> Vec<Box<dyn Engine>> {
    vec![
        Box::new(image::ImageEngine),
        Box::new(data::DataEngine),
        Box::new(sheet::SheetEngine),
        Box::new(markdown::MarkdownEngine),
        Box::new(doc::DocEngine),
        Box::new(pdflayout::PdfLayoutEngine),
        Box::new(pdflayout::PdfTablesEngine),
        Box::new(docxlayout::DocxLayoutEngine),
        Box::new(pptxlayout::PptxLayoutEngine),
        Box::new(xlsxlayout::XlsxLayoutEngine),
        Box::new(svg::SvgEngine),
        Box::new(emf::MetafileEngine),
        Box::new(audio::AudioEngine),
        Box::new(subtitles::SubtitleEngine),
        Box::new(archive::ArchiveEngine),
        Box::new(psd::PsdEngine),
        Box::new(raw::RawEngine),
        Box::new(font::FontEngine),
        Box::new(xmljson::XmlJsonEngine),
    ]
}
