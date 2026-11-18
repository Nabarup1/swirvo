//! Tools and actions that aren't format conversions (docs/11). Same rules as conversions:
//! originals are never touched, outputs get a fresh name next to the source, files are
//! written to a temp name and committed atomically without replacing anything.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use serde::Serialize;
use sha2::Digest;
use swirvo_core::{CancelToken, Category, EngineError, Preset};
use swirvo_engines::{archive, image as img, pdf_tools};

use crate::args::{Field, ToolArgs};
use crate::{Toolkit, commit_no_replace, fallback_dir};
use swirvo_engines::pdfedit::secure::{self, Need};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    ResizeHalf,
    Fit1920,
    RotateRight,
    Flip,
    Compress,
    StripMetadata,
    ImagesToPdf,
    PdfMerge,
    PdfSplit,
    PdfRotate,
    PdfToPng,
    PdfToJpg,
    /// Opens the page picker (extract, delete, export a selection of pages).
    PdfPages,
    ExtractHere,
    TestArchive,
    Checksum,
    ZipFiles,
    CopyText,
    // PDF editing (docs/17).
    PdfCompress,
    PdfNumbers,
    PdfHeaderFooter,
    PdfBates,
    PdfWatermark,
    PdfProtect,
    PdfUnlock,
    PdfTrim,
    PdfCrop,
    PdfResize,
    PdfNup,
    PdfBooklet,
    PdfSplitEvery,
    /// Several PDFs from typed page groups: "1-15, 16; 25-40, 42".
    PdfSplitRanges,
    PdfSplitChapters,
    PdfSplitSize,
    PdfClean,
    PdfInfo,
    PdfRepair,
    PdfBookmarks,
    PdfImages,
    PdfAttachments,
    PdfAttach,
    PdfSearchable,
    PdfRedact,
    PdfGray,
    PdfFormFields,
    PdfFill,
    PdfFlatten,
    PdfCompare,
    /// Sign with a certificate from the Windows certificate store.
    PdfSign,
    /// Check the signatures in a PDF.
    PdfVerify,
    // Word documents (docs/17 §8).
    DocxCompress,
    DocxClean,
    DocxReplace,
    DocxPictures,
    DocxStats,
    DocxMailMerge,
    // Video (docs/11 §2).
    VideoFrame,
    VideoCompress,
    /// Word, ODT, RTF, PowerPoint or Excel → archival PDF/A, tagged for screen readers.
    OfficePdfA,
}

/// Which wheel a tool lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Tools,
    Actions,
}

impl Tool {
    pub const ALL: &'static [Tool] = &[
        Tool::ResizeHalf,
        Tool::Fit1920,
        Tool::RotateRight,
        Tool::Flip,
        Tool::Compress,
        Tool::StripMetadata,
        Tool::ImagesToPdf,
        Tool::PdfMerge,
        Tool::PdfSplit,
        Tool::PdfRotate,
        Tool::PdfToPng,
        Tool::PdfToJpg,
        Tool::PdfPages,
        Tool::ExtractHere,
        Tool::TestArchive,
        Tool::Checksum,
        Tool::ZipFiles,
        Tool::CopyText,
        Tool::PdfCompress,
        Tool::PdfNumbers,
        Tool::PdfHeaderFooter,
        Tool::PdfBates,
        Tool::PdfWatermark,
        Tool::PdfProtect,
        Tool::PdfUnlock,
        Tool::PdfTrim,
        Tool::PdfCrop,
        Tool::PdfResize,
        Tool::PdfNup,
        Tool::PdfBooklet,
        Tool::PdfSplitEvery,
        Tool::PdfSplitRanges,
        Tool::PdfSplitChapters,
        Tool::PdfSplitSize,
        Tool::PdfClean,
        Tool::PdfInfo,
        Tool::PdfRepair,
        Tool::PdfBookmarks,
        Tool::PdfImages,
        Tool::PdfAttachments,
        Tool::PdfAttach,
        Tool::PdfSearchable,
        Tool::PdfRedact,
        Tool::PdfGray,
        Tool::PdfFormFields,
        Tool::PdfFill,
        Tool::PdfFlatten,
        Tool::PdfCompare,
        Tool::PdfSign,
        Tool::PdfVerify,
        Tool::DocxCompress,
        Tool::DocxClean,
        Tool::DocxReplace,
        Tool::DocxPictures,
        Tool::DocxStats,
        Tool::DocxMailMerge,
        Tool::VideoFrame,
        Tool::VideoCompress,
        Tool::OfficePdfA,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Tool::ResizeHalf => "resize-half",
            Tool::Fit1920 => "fit-1920",
            Tool::RotateRight => "rotate-right",
            Tool::Flip => "flip",
            Tool::Compress => "compress",
            Tool::StripMetadata => "strip-metadata",
            Tool::ImagesToPdf => "images-to-pdf",
            Tool::PdfMerge => "pdf-merge",
            Tool::PdfSplit => "pdf-split",
            Tool::PdfRotate => "pdf-rotate",
            Tool::PdfToPng => "pdf-to-png",
            Tool::PdfToJpg => "pdf-to-jpg",
            Tool::PdfPages => "pdf-pages",
            Tool::ExtractHere => "extract",
            Tool::TestArchive => "test-archive",
            Tool::Checksum => "checksum",
            Tool::ZipFiles => "zip",
            Tool::CopyText => "copy-text",
            Tool::PdfCompress => "pdf-compress",
            Tool::PdfNumbers => "pdf-numbers",
            Tool::PdfHeaderFooter => "pdf-header-footer",
            Tool::PdfBates => "pdf-bates",
            Tool::PdfWatermark => "pdf-watermark",
            Tool::PdfProtect => "pdf-protect",
            Tool::PdfUnlock => "pdf-unlock",
            Tool::PdfTrim => "pdf-trim",
            Tool::PdfCrop => "pdf-crop",
            Tool::PdfResize => "pdf-resize",
            Tool::PdfNup => "pdf-nup",
            Tool::PdfBooklet => "pdf-booklet",
            Tool::PdfSplitEvery => "pdf-split-every",
            Tool::PdfSplitRanges => "pdf-split-ranges",
            Tool::PdfSplitChapters => "pdf-split-chapters",
            Tool::PdfSplitSize => "pdf-split-size",
            Tool::PdfClean => "pdf-clean",
            Tool::PdfInfo => "pdf-info",
            Tool::PdfRepair => "pdf-repair",
            Tool::PdfBookmarks => "pdf-bookmarks",
            Tool::PdfImages => "pdf-images",
            Tool::PdfAttachments => "pdf-attachments",
            Tool::PdfAttach => "pdf-attach",
            Tool::PdfSearchable => "pdf-searchable",
            Tool::PdfRedact => "pdf-redact",
            Tool::PdfGray => "pdf-gray",
            Tool::PdfFormFields => "pdf-form-fields",
            Tool::PdfFill => "pdf-fill",
            Tool::PdfFlatten => "pdf-flatten",
            Tool::PdfCompare => "pdf-compare",
            Tool::PdfSign => "pdf-sign",
            Tool::PdfVerify => "pdf-verify",
            Tool::DocxCompress => "docx-compress",
            Tool::DocxClean => "docx-clean",
            Tool::DocxReplace => "docx-replace",
            Tool::DocxPictures => "docx-pictures",
            Tool::DocxStats => "docx-stats",
            Tool::DocxMailMerge => "docx-mail-merge",
            Tool::VideoFrame => "video-frame",
            Tool::VideoCompress => "video-compress",
            Tool::OfficePdfA => "pdfa",
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.id() == id)
    }

    /// Wedge label (short) and caption (what it does).
    pub fn label(self) -> (&'static str, &'static str) {
        match self {
            Tool::ResizeHalf => ("Resize", "50 %"),
            Tool::Fit1920 => ("Fit 1920", "Longest side"),
            Tool::RotateRight => ("Rotate", "90° right"),
            Tool::Flip => ("Flip", "Mirror"),
            Tool::Compress => ("Compress", "Smaller file"),
            Tool::StripMetadata => ("Strip info", "GPS, camera"),
            Tool::ImagesToPdf => ("To one PDF", "Page each"),
            Tool::PdfMerge => ("Merge", "One PDF"),
            Tool::PdfSplit => ("Split", "Page each"),
            Tool::PdfRotate => ("Rotate", "90° right"),
            Tool::PdfToPng => ("Pages PNG", "Image each"),
            Tool::PdfToJpg => ("Pages JPG", "Image each"),
            Tool::PdfPages => ("Pages…", "Pick pages"),
            Tool::ExtractHere => ("Extract", "New folder"),
            Tool::TestArchive => ("Test", "Integrity"),
            Tool::Checksum => ("Checksum", "SHA-256"),
            Tool::ZipFiles => ("To ZIP", "Compress"),
            Tool::CopyText => ("Copy text", "Clipboard"),
            Tool::PdfCompress => ("Compress", "Smaller PDF"),
            Tool::PdfNumbers => ("Numbers", "Page 1 of 9"),
            Tool::PdfHeaderFooter => ("Header", "Text on pages"),
            Tool::PdfBates => ("Bates", "Legal numbering"),
            Tool::PdfWatermark => ("Watermark", "Text or logo"),
            Tool::PdfProtect => ("Protect", "Password"),
            Tool::PdfUnlock => ("Unlock", "Remove password"),
            Tool::PdfTrim => ("Trim", "White margins"),
            Tool::PdfCrop => ("Crop", "By margins"),
            Tool::PdfResize => ("Resize", "A4, Letter…"),
            Tool::PdfNup => ("N-up", "Pages per sheet"),
            Tool::PdfBooklet => ("Booklet", "Fold & staple"),
            Tool::PdfSplitEvery => ("Split by", "Every N pages"),
            Tool::PdfSplitRanges => ("Split ranges", "Your own groups"),
            Tool::PdfSplitChapters => ("Chapters", "Split at bookmarks"),
            Tool::PdfSplitSize => ("Split size", "Email-sized parts"),
            Tool::PdfClean => ("Clean", "Hidden data"),
            Tool::PdfInfo => ("Properties", "Title, author"),
            Tool::PdfRepair => ("Repair", "Damaged PDF"),
            Tool::PdfBookmarks => ("Bookmarks", "From headings"),
            Tool::PdfImages => ("Pictures", "Save images"),
            Tool::PdfAttachments => ("Attachments", "Save files"),
            Tool::PdfAttach => ("Attach", "Files into PDF"),
            Tool::PdfSearchable => ("Searchable", "OCR text layer"),
            Tool::PdfRedact => ("Redact", "Remove found text"),
            Tool::PdfGray => ("Grayscale", "Black & white"),
            Tool::PdfFormFields => ("Form fields", "To a spreadsheet"),
            Tool::PdfFill => ("Fill form", "A PDF per row"),
            Tool::PdfFlatten => ("Flatten", "Fields into page"),
            Tool::PdfCompare => ("Compare", "What changed"),
            Tool::PdfSign => ("Digital signature", "Your certificate"),
            Tool::PdfVerify => ("Signatures", "Check them"),
            Tool::DocxCompress => ("Compress", "Smaller file"),
            Tool::DocxClean => ("Clean", "Author, changes"),
            Tool::DocxReplace => ("Replace", "Find & replace"),
            Tool::DocxPictures => ("Pictures", "Save images"),
            Tool::DocxStats => ("Word count", "Clipboard"),
            Tool::DocxMailMerge => ("Mail merge", "Letter per row"),
            Tool::VideoFrame => ("Frame", "Save a picture"),
            Tool::VideoCompress => ("Compress", "To a size"),
            Tool::OfficePdfA => ("PDF/A", "Archival PDF"),
        }
    }

    /// Page tools that open the page picker for a single multi-page PDF, and the
    /// operation the picker starts on.
    pub fn page_op(self) -> Option<PageOp> {
        match self {
            Tool::PdfToJpg => Some(PageOp::Images {
                png: false,
                dpi: 150,
            }),
            Tool::PdfToPng => Some(PageOp::Images {
                png: true,
                dpi: 150,
            }),
            Tool::PdfSplit | Tool::PdfSplitRanges => Some(PageOp::Split),
            Tool::PdfRotate => Some(PageOp::Rotate),
            Tool::PdfPages => Some(PageOp::Extract),
            _ => None,
        }
    }

    pub fn place(self) -> Place {
        match self {
            Tool::ZipFiles | Tool::CopyText => Place::Actions,
            _ => Place::Tools,
        }
    }

    /// The options this tool takes (empty for most).
    pub fn fields(self) -> &'static [Field] {
        if self.is_docx() {
            crate::doc_ops::fields(self)
        } else if self.is_video() {
            crate::media_ops::fields(self)
        } else {
            crate::pdf_ops::fields(self)
        }
    }

    /// Should the options be shown before it runs? (From the wheel, such tools open the
    /// Board's form instead of running with defaults.)
    pub fn asks(self) -> bool {
        crate::pdf_ops::asks(self) || crate::doc_ops::asks(self) || crate::media_ops::asks(self)
    }

    fn is_pdf_edit(self) -> bool {
        matches!(
            self,
            Tool::PdfCompress
                | Tool::PdfNumbers
                | Tool::PdfHeaderFooter
                | Tool::PdfBates
                | Tool::PdfWatermark
                | Tool::PdfProtect
                | Tool::PdfUnlock
                | Tool::PdfTrim
                | Tool::PdfCrop
                | Tool::PdfResize
                | Tool::PdfNup
                | Tool::PdfBooklet
                | Tool::PdfSplitEvery
                | Tool::PdfSplitRanges
                | Tool::PdfSplitChapters
                | Tool::PdfSplitSize
                | Tool::PdfClean
                | Tool::PdfInfo
                | Tool::PdfRepair
                | Tool::PdfBookmarks
                | Tool::PdfImages
                | Tool::PdfAttachments
                | Tool::PdfAttach
                | Tool::PdfSearchable
                | Tool::PdfRedact
                | Tool::PdfGray
                | Tool::PdfFormFields
                | Tool::PdfFill
                | Tool::PdfFlatten
                | Tool::PdfCompare
                | Tool::PdfSign
                | Tool::PdfVerify
        )
    }

    fn is_docx(self) -> bool {
        matches!(
            self,
            Tool::DocxCompress
                | Tool::DocxClean
                | Tool::DocxReplace
                | Tool::DocxPictures
                | Tool::DocxStats
                | Tool::DocxMailMerge
                | Tool::OfficePdfA
        )
    }

    fn is_video(self) -> bool {
        matches!(self, Tool::VideoFrame | Tool::VideoCompress)
    }
}

/// Documents our layout engines turn into PDF/A.
const OFFICE: &[&str] = &["docx", "odt", "rtf", "pptx", "xlsx"];

const CODE_EXTS: &[&str] = &[
    "rs",
    "py",
    "js",
    "mjs",
    "ts",
    "tsx",
    "jsx",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "java",
    "kt",
    "go",
    "rb",
    "php",
    "sh",
    "bash",
    "ps1",
    "bat",
    "cmd",
    "ini",
    "cfg",
    "conf",
    "sql",
    "css",
    "scss",
    "lua",
    "swift",
    "r",
    "m",
    "pl",
    "env",
    "gitignore",
    "dockerfile",
];

/// Tools that apply to these files, best first. Decided from file *names* only: the agent
/// never opens user files (PROJECT_GUIDE.md rule 3). The tool itself checks content when it runs.
pub fn tools_for(tk: &Toolkit, files: &[PathBuf]) -> Vec<Tool> {
    let reg = tk.registry;
    let keys: Vec<Option<&str>> = files
        .iter()
        .map(|f| reg.from_path(f).map(|id| reg.key(id)))
        .collect();
    let all =
        |pred: &dyn Fn(Option<&str>) -> bool| !keys.is_empty() && keys.iter().all(|k| pred(*k));
    let n = files.len();
    let mut out = Vec::new();
    if all(&|k| k.is_some_and(|k| img::RASTER.contains(&k))) {
        out.extend([
            Tool::ResizeHalf,
            Tool::Fit1920,
            Tool::RotateRight,
            Tool::Flip,
            Tool::Compress,
            Tool::StripMetadata,
        ]);
        if n >= 2 {
            out.push(Tool::ImagesToPdf);
        }
    }
    if all(&|k| k == Some("pdf")) {
        // Most used first: the wheel shows the first seven, the Board all of them.
        if n >= 2 {
            out.push(Tool::PdfMerge);
        }
        if n == 2 {
            out.push(Tool::PdfCompare);
        }
        out.push(Tool::PdfCompress);
        if n == 1 {
            out.push(Tool::PdfPages);
        }
        out.extend([
            Tool::PdfRotate,
            Tool::PdfSplit,
            Tool::PdfNumbers,
            Tool::PdfProtect,
            Tool::PdfWatermark,
            Tool::PdfRedact,
            Tool::PdfToJpg,
            Tool::PdfToPng,
            Tool::PdfTrim,
            Tool::PdfResize,
            Tool::PdfGray,
            Tool::PdfNup,
            Tool::PdfBooklet,
            Tool::PdfSplitEvery,
            Tool::PdfSplitRanges,
            Tool::PdfSplitChapters,
            Tool::PdfSplitSize,
            Tool::PdfHeaderFooter,
            Tool::PdfBates,
            Tool::PdfCrop,
            Tool::PdfUnlock,
            Tool::PdfClean,
            Tool::PdfSearchable,
            Tool::PdfFlatten,
            Tool::PdfFormFields,
            Tool::PdfInfo,
            Tool::PdfBookmarks,
            Tool::PdfImages,
            Tool::PdfAttachments,
            Tool::PdfRepair,
        ]);
        // Signing needs the Windows certificate store; reading signatures back works
        // anywhere (the cryptographic check only on Windows).
        if swirvo_engines::os::certsign::available() {
            out.push(Tool::PdfSign);
        }
        out.push(Tool::PdfVerify);
    }
    if all(&|k| k == Some("docx")) {
        out.extend([
            Tool::DocxCompress,
            Tool::DocxClean,
            Tool::DocxReplace,
            Tool::DocxStats,
            Tool::DocxPictures,
        ]);
        if n == 1 {
            out.push(Tool::DocxMailMerge);
        }
    }
    if all(&|k| k.is_some_and(|k| OFFICE.contains(&k))) {
        out.push(Tool::OfficePdfA);
    }
    // A letter plus its data file: mail merge.
    let docx = keys.iter().filter(|k| **k == Some("docx")).count();
    if docx == 1
        && n == 2
        && keys
            .iter()
            .any(|k| k.is_some_and(|k| crate::doc_ops::DATA.contains(&k)))
    {
        out.push(Tool::DocxMailMerge);
    }
    // One PDF plus other files: attach them to it; plus a data file: fill its form.
    let pdfs = keys.iter().filter(|k| **k == Some("pdf")).count();
    if pdfs == 1
        && n == 2
        && keys
            .iter()
            .any(|k| k.is_some_and(|k| crate::doc_ops::DATA.contains(&k)))
    {
        out.push(Tool::PdfFill);
    }
    if pdfs == 1 && n >= 2 {
        out.push(Tool::PdfAttach);
    }
    let video = |k: Option<&str>| {
        k.and_then(|k| reg.id(k))
            .is_some_and(|id| reg.get(id).category == Category::Video)
    };
    if all(&video) && swirvo_engines::video::available() {
        out.extend([Tool::VideoCompress, Tool::VideoFrame]);
    }
    if all(&|k| k.is_some_and(|k| archive::formats().contains(&k))) {
        out.extend([Tool::ExtractHere, Tool::TestArchive]);
    } else if all(&|k| k.is_some_and(|k| archive::formats().contains(&k) || system_archive(k))) {
        out.push(Tool::ExtractHere);
    }
    out.push(Tool::Checksum);
    out.push(Tool::ZipFiles);
    if n == 1 {
        let ext = files[0]
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let texty = keys[0].and_then(|k| reg.id(k)).is_some_and(|id| {
            let def = reg.get(id);
            matches!(
                def.category,
                Category::Text | Category::Data | Category::Subtitle
            ) || matches!(def.key, "md" | "html" | "csv" | "tsv" | "svg")
        });
        let name = files[0]
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let ocr = keys[0].is_some_and(|k| swirvo_engines::ocr::INPUTS.contains(&k))
            && swirvo_engines::ocr::available();
        if texty
            || ocr
            || CODE_EXTS.contains(&ext.as_str())
            || CODE_EXTS.contains(&name.trim_start_matches('.'))
        {
            out.push(Tool::CopyText);
        }
    }
    out
}

#[derive(Debug, Default, Clone, Serialize, serde::Deserialize)]
pub struct ToolResult {
    pub outputs: Vec<PathBuf>,
    /// One line for a notification or the terminal.
    pub message: String,
    /// Text to put on the clipboard (checksums, copied text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<(PathBuf, String)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("can't read it: {0}")]
    Io(#[from] io::Error),
    #[error("not an archive this build can open")]
    NotArchive,
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error("{0}")]
    Invalid(String),
}

pub(crate) fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// How many files a tool works on at once: one per core (or the Settings choice), at most `cap` (heavier tools
/// pass a smaller cap; video encoders already use every core on their own).
pub(crate) fn workers(files: usize, cap: usize) -> usize {
    // One per core, or what the user set in Settings ("How many files at once").
    let cores = crate::settings::current().jobs();
    cores.min(cap).min(files).max(1)
}

/// `f` on each file, on up to `workers` threads, results in file order. Files not
/// started because of cancelling come back as `None`.
pub(crate) fn par_map<T: Send>(
    files: &[PathBuf],
    workers: usize,
    cancel: &CancelToken,
    f: impl Fn(&Path) -> T + Sync,
) -> Vec<Option<T>> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let slots: Vec<std::sync::Mutex<Option<T>>> =
        files.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let run = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            let Some(file) = files.get(i) else { break };
            if cancel.is_cancelled() {
                break;
            }
            let v = f(file);
            *slots[i].lock().unwrap_or_else(|p| p.into_inner()) = Some(v);
        }
    };
    if workers <= 1 || files.len() <= 1 {
        run();
    } else {
        std::thread::scope(|s| {
            for _ in 0..workers.min(files.len()) {
                s.spawn(run);
            }
        });
    }
    slots
        .into_iter()
        .map(|m| m.into_inner().unwrap_or_else(|p| p.into_inner()))
        .collect()
}

pub(crate) fn parent_of(p: &Path) -> PathBuf {
    p.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

fn writable_dir(dir: &Path) -> io::Result<(tempfile::TempDir, PathBuf, Option<String>)> {
    match tempfile::Builder::new()
        .prefix(".swirvo-tmp-")
        .tempdir_in(dir)
    {
        Ok(t) => Ok((t, dir.to_path_buf(), None)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            let d = fallback_dir()?;
            let t = tempfile::Builder::new()
                .prefix(".swirvo-tmp-")
                .tempdir_in(&d)?;
            Ok((
                t,
                d,
                Some("the folder is read-only, so it went to Downloads/Swirvo".into()),
            ))
        }
        Err(e) => Err(e),
    }
}

/// Produce `stem.ext` (or `stem (2).ext`, …) in `dir` via `write`, atomically, never
/// replacing an existing file.
pub(crate) fn emit(
    dir: &Path,
    stem: &str,
    ext: &str,
    write: impl FnOnce(&Path) -> Result<(), EngineError>,
) -> Result<(PathBuf, Option<String>), ToolError> {
    let (scratch, dir, note) = writable_dir(dir)?;
    let tmp = scratch.path().join(format!("out.{ext}"));
    write(&tmp)?;
    if fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) == 0 {
        return Err(ToolError::Invalid("produced an empty file".into()));
    }
    for k in 1u32..10_000 {
        let cand = dir.join(if k == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem} ({k}).{ext}")
        });
        if cand.exists() {
            continue;
        }
        match commit_no_replace(&tmp, &cand) {
            Ok(()) => return Ok((cand, note)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(ToolError::Invalid("no free file name".into()))
}

/// Create the first free folder among `dir/stem`, `dir/stem (2)`, … `create_dir` is atomic
/// and fails if the name exists, so the folder is ours alone.
pub fn reserve_dir(dir: &Path, stem: &str) -> io::Result<PathBuf> {
    for k in 1u32..10_000 {
        let p = dir.join(if k == 1 {
            stem.to_string()
        } else {
            format!("{stem} ({k})")
        });
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("no free folder name"))
}

/// A new output folder next to `src` (or in Downloads/Swirvo if that's read-only).
fn new_folder(src: &Path, stem: &str) -> Result<(PathBuf, Option<String>), ToolError> {
    match reserve_dir(&parent_of(src), stem) {
        Ok(d) => Ok((d, None)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            Ok((
                reserve_dir(&fallback_dir()?, stem)?,
                Some("the folder is read-only, so it went to Downloads/Swirvo".into()),
            ))
        }
        Err(e) => Err(e.into()),
    }
}

/// Run `f` into a fresh folder; on failure remove the folder (it only ever holds our files).
pub(crate) fn into_folder(
    src: &Path,
    stem: &str,
    f: impl FnOnce(&Path) -> Result<Vec<PathBuf>, EngineError>,
) -> Result<(PathBuf, Vec<PathBuf>, Option<String>), ToolError> {
    let (dir, note) = new_folder(src, stem)?;
    match f(&dir) {
        Ok(files) => Ok((dir, files, note)),
        Err(e) => {
            let _ = fs::remove_dir_all(&dir);
            Err(e.into())
        }
    }
}

fn detect_key(tk: &Toolkit, p: &Path) -> Result<&'static str, ToolError> {
    let det = tk
        .detect(p)?
        .ok_or_else(|| ToolError::Invalid(format!("{}: unrecognized file type", name_of(p))))?;
    Ok(tk.registry.key(det.format))
}

fn sorted(files: &[PathBuf]) -> Vec<PathBuf> {
    let mut v = files.to_vec();
    v.sort_by_key(|p| name_of(p).to_lowercase());
    v
}

/// "1 page", "3 pages".
pub(crate) fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

pub(crate) fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// Run `tool` on `files` with its default options.
pub fn run_tool(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    run_tool_with(tk, tool, files, &ToolArgs::new(), cancel)
}

/// Run `tool` on `files` with `args` (see [`Tool::fields`]). Per-file tools keep going
/// after a failure and report it.
pub fn run_tool_with(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    args: &ToolArgs,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    if files.is_empty() {
        return Err(ToolError::Invalid("no files".into()));
    }
    if tool.is_pdf_edit() || tool.is_docx() || tool.is_video() {
        let r = if tool.is_docx() {
            crate::doc_ops::run(tk, tool, files, args, cancel)?
        } else if tool.is_video() {
            crate::media_ops::run(tk, tool, files, args, cancel)?
        } else {
            crate::pdf_ops::run(tk, tool, files, args, cancel)?
        };
        if r.outputs.is_empty() && !r.failures.is_empty() && files.len() == 1 {
            let (f, e) = &r.failures[0];
            return Err(ToolError::Invalid(format!("{}: {e}", name_of(f))));
        }
        return Ok(r);
    }
    if !args.is_empty() {
        return Err(ToolError::Invalid(format!(
            "{} takes no options",
            tool.id()
        )));
    }
    // Existing PDF tools honour the file's permissions too.
    let need = match tool {
        Tool::PdfRotate => Some(Need::Assemble),
        Tool::PdfToPng | Tool::PdfToJpg => Some(Need::Copy),
        Tool::PdfMerge | Tool::PdfSplit => Some(Need::Everything),
        _ => None,
    };
    if let Some(n) = need {
        for f in files {
            secure::permits(f, n)?;
        }
    }
    let reg = tk.registry;
    let mut r = ToolResult::default();
    let mut note = None;
    match tool {
        Tool::ResizeHalf
        | Tool::Fit1920
        | Tool::RotateRight
        | Tool::Flip
        | Tool::Compress
        | Tool::StripMetadata => {
            let done = par_map(files, workers(files.len(), 8), cancel, |f| {
                image_tool(tk, tool, f)
            });
            cancel.check().map_err(ToolError::Engine)?;
            for (f, res) in files.iter().zip(done) {
                match res {
                    Some(Ok(Some((out, n)))) => {
                        r.outputs.push(out);
                        note = note.or(n);
                    }
                    Some(Ok(None)) | None => {}
                    Some(Err(e)) => r.failures.push((f.clone(), e.to_string())),
                }
            }
            let (label, _) = tool.label();
            r.message = match (r.outputs.len(), files.len()) {
                (0, _) if r.failures.is_empty() => match tool {
                    Tool::Fit1920 => "Already 1920 px or smaller".into(),
                    Tool::Compress => "Already as small as it gets".into(),
                    _ => "Nothing to do".into(),
                },
                (1, 1) => format!("{label}: {}", name_of(&r.outputs[0])),
                (n, _) => format!("{label}: {n} images"),
            };
        }
        Tool::ImagesToPdf => {
            let files = sorted(files);
            let stem = reg.stem(&files[0]);
            let (out, n) = emit(
                &parent_of(&files[0]),
                &format!("{stem} (combined)"),
                "pdf",
                |tmp| pdf_tools::images_to_pdf(&files, tmp, cancel).map(|_| ()),
            )?;
            note = n;
            r.message = format!("{} images → {}", files.len(), name_of(&out));
            r.outputs.push(out);
        }
        Tool::PdfMerge => {
            let files = sorted(files);
            let stem = reg.stem(&files[0]);
            let mut pages = 0;
            let (out, n) = emit(
                &parent_of(&files[0]),
                &format!("{stem} (merged)"),
                "pdf",
                |tmp| {
                    pages = pdf_tools::merge(&files, tmp, cancel)?;
                    Ok(())
                },
            )?;
            note = n;
            r.message = format!("{} PDFs, {pages} pages → {}", files.len(), name_of(&out));
            r.outputs.push(out);
        }
        Tool::PdfRotate => {
            for f in files {
                let stem = reg.stem(f);
                match emit(&parent_of(f), &format!("{stem} (rotated)"), "pdf", |tmp| {
                    pdf_tools::rotate(f, tmp, 90).map(|_| ())
                }) {
                    Ok((out, n)) => {
                        r.outputs.push(out);
                        note = note.or(n);
                    }
                    Err(e) => r.failures.push((f.clone(), e.to_string())),
                }
            }
            r.message = format!(
                "Rotated {} PDF{}",
                r.outputs.len(),
                if r.outputs.len() == 1 { "" } else { "s" }
            );
        }
        Tool::PdfSplit | Tool::PdfToPng | Tool::PdfToJpg => {
            for f in files {
                let stem = reg.stem(f);
                let folder = match tool {
                    Tool::PdfSplit => format!("{stem} (pages)"),
                    _ => format!("{stem} (images)"),
                };
                let res = into_folder(f, &folder, |dir| match tool {
                    Tool::PdfSplit => pdf_tools::split(f, dir, &stem, cancel),
                    Tool::PdfToPng => pdf_tools::render_pages(f, dir, &stem, "png", cancel),
                    _ => pdf_tools::render_pages(f, dir, &stem, "jpg", cancel),
                });
                match res {
                    Ok((dir, made, n)) => {
                        note = note.or(n);
                        r.message = format!("{} pages → {}", made.len(), name_of(&dir));
                        r.outputs.push(dir);
                    }
                    Err(e) => r.failures.push((f.clone(), e.to_string())),
                }
            }
        }
        Tool::PdfPages => {
            return Err(ToolError::Invalid(
                "pick the pages first (page picker, or --pages 1-3)".into(),
            ));
        }
        Tool::ExtractHere => {
            for f in files {
                match extract_here(tk, f, cancel) {
                    Ok(x) => {
                        note = note.or(x.note);
                        r.message = format!("{} files → {}", x.files, name_of(&x.dir));
                        r.outputs.push(x.dir);
                    }
                    Err(e) => r.failures.push((f.clone(), e.to_string())),
                }
            }
        }
        Tool::TestArchive => {
            let mut lines = Vec::new();
            for f in files {
                let key = detect_key(tk, f)?;
                if !archive::formats().contains(&key) {
                    r.failures
                        .push((f.clone(), "not an archive this build can open".into()));
                    continue;
                }
                match archive::test(f, key, cancel) {
                    Ok((n, bytes)) => {
                        lines.push(format!("{}: OK, {n} files, {}", name_of(f), human(bytes)))
                    }
                    Err(e) => r.failures.push((f.clone(), format!("damaged: {e}"))),
                }
            }
            r.message = lines.join("\n");
        }
        Tool::Checksum => {
            let mut lines = Vec::new();
            for f in files {
                match sha256(f, cancel) {
                    Ok(h) => lines.push(format!("{h}  {}", name_of(f))),
                    Err(e) => r.failures.push((f.clone(), e.to_string())),
                }
            }
            r.message = match lines.as_slice() {
                [one] => format!("SHA-256 {}… copied", &one[..16]),
                more => format!("{} checksums copied", more.len()),
            };
            r.clipboard = Some(lines.join("\r\n"));
        }
        Tool::ZipFiles => {
            let first = &files[0];
            let stem = if files.len() == 1 {
                name_of(first)
            } else {
                format!("{} and {} more", reg.stem(first), files.len() - 1)
            };
            let mut count = 0;
            let (out, n) = emit(&parent_of(first), &stem, "zip", |tmp| {
                count = zip_paths(files, tmp, cancel)?;
                Ok(())
            })?;
            note = n;
            r.message = format!("{count} files → {}", name_of(&out));
            r.outputs.push(out);
        }
        Tool::CopyText
            if detect_key(tk, &files[0])
                .is_ok_and(|k| swirvo_engines::ocr::INPUTS.contains(&k)) =>
        {
            let f = &files[0];
            let key = detect_key(tk, f)?;
            let text = swirvo_engines::ocr::recognize_file(f, key)?;
            let text = text.trim().to_string();
            if text.is_empty() {
                return Err(ToolError::Invalid("no text found in this image".into()));
            }
            r.message = format!("Copied {} lines (OCR)", text.lines().count());
            r.clipboard = Some(text.replace('\n', "\r\n"));
        }
        Tool::CopyText => {
            let f = &files[0];
            let meta = fs::metadata(f)?;
            if meta.len() > 8 << 20 {
                return Err(ToolError::Invalid("too big to copy (over 8 MB)".into()));
            }
            let mut bytes = Vec::new();
            fs::File::open(f)?.read_to_end(&mut bytes)?;
            if bytes.contains(&0)
                && !bytes.starts_with(&[0xFF, 0xFE])
                && !bytes.starts_with(&[0xFE, 0xFF])
            {
                return Err(ToolError::Invalid("not a text file".into()));
            }
            let text = swirvo_engines::text_from_bytes(&bytes);
            r.message = format!("Copied {} lines", text.lines().count());
            r.clipboard = Some(text);
        }
        // PDF editing tools returned through pdf_ops above.
        other => {
            return Err(ToolError::Invalid(format!("{} can't run here", other.id())));
        }
    }
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    if r.outputs.is_empty() && r.clipboard.is_none() && !r.failures.is_empty() {
        let (f, e) = &r.failures[0];
        return Err(ToolError::Invalid(format!("{}: {e}", name_of(f))));
    }
    Ok(r)
}

/// 7Z, RAR, ISO, CAB: extracted by the system's libarchive when it's present.
fn system_archive(key: &str) -> bool {
    swirvo_engines::os::systar::FORMATS.contains(&key)
        && swirvo_engines::os::systar::exe().is_some()
}

/// (files, bytes) under `dir`.
fn count_tree(dir: &Path) -> (usize, u64) {
    let mut out = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(e.path()),
                Ok(_) => {
                    out.0 += 1;
                    out.1 += e.metadata().map_or(0, |m| m.len());
                }
                Err(_) => {}
            }
        }
    }
    out
}

/// Page count of a PDF, or None if it can't be read.
pub fn pdf_page_count(p: &Path) -> Option<usize> {
    pdf_tools::page_count(p).ok()
}

/// What the page picker does with the selected pages. Every operation writes new files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum PageOp {
    /// One image per page (a single page goes next to the PDF; more go in a new folder).
    Images { png: bool, dpi: u32 },
    /// One new PDF with just these pages.
    Extract,
    /// One PDF per selected page, in a new folder.
    Split,
    /// A copy with the selected pages turned 90° clockwise.
    Rotate,
    /// A copy without the selected pages.
    Delete,
}

impl PageOp {
    /// Button text: "Export 3 pages as JPG", "Delete 2 pages"…
    pub fn verb(self, n: usize) -> String {
        let pages = if n == 1 {
            "1 page".to_string()
        } else {
            format!("{n} pages")
        };
        match self {
            PageOp::Images { png, .. } => {
                format!("Export {pages} as {}", if png { "PNG" } else { "JPG" })
            }
            PageOp::Extract => format!("Save {pages} as a new PDF"),
            PageOp::Split => format!("Split {pages} into PDFs"),
            PageOp::Rotate => format!("Rotate {pages}"),
            PageOp::Delete => format!("Delete {pages}"),
        }
    }
}

/// Run a page operation on the 0-based `pages` of one PDF.
pub fn run_page_op(
    tk: &Toolkit,
    file: &Path,
    op: PageOp,
    pages: &[usize],
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    if pages.is_empty() {
        return Err(ToolError::Invalid("no pages selected".into()));
    }
    secure::permits(
        file,
        match op {
            PageOp::Images { .. } => Need::Copy,
            PageOp::Rotate => Need::Assemble,
            PageOp::Extract | PageOp::Split | PageOp::Delete => Need::Everything,
        },
    )?;
    let count = pdf_tools::page_count(file)?;
    let stem = tk.registry.stem(file);
    let dir = parent_of(file);
    let mut r = ToolResult::default();
    let label = crate::pages::format(pages);
    let note;
    match op {
        PageOp::Images { png, dpi } => {
            let ext = if png { "png" } else { "jpg" };
            if let [one] = pages {
                let digits = count.to_string().len().max(2);
                let (out, n) = emit(&dir, &format!("{stem} p{:0digits$}", one + 1), ext, |tmp| {
                    pdf_tools::render_one(file, *one, ext, dpi as f32, tmp)
                })?;
                note = n;
                r.message = format!("Saved {}", name_of(&out));
                r.outputs.push(out);
            } else {
                let (folder, made, n) = into_folder(file, &format!("{stem} (images)"), |d| {
                    pdf_tools::render_selected(file, d, &stem, ext, Some(pages), dpi as f32, cancel)
                })?;
                note = n;
                r.message = format!("Saved {} images → {}", made.len(), name_of(&folder));
                r.outputs.push(folder);
            }
        }
        PageOp::Extract => {
            let name = if label.len() <= 32 {
                format!("{stem} (pages {label})")
            } else {
                format!("{stem} ({} pages)", pages.len())
            };
            let (out, n) = emit(&dir, &name, "pdf", |tmp| {
                pdf_tools::extract_pages(file, pages, tmp).map(|_| ())
            })?;
            note = n;
            r.message = format!("{} pages → {}", pages.len(), name_of(&out));
            r.outputs.push(out);
        }
        PageOp::Split => {
            let (folder, made, n) = into_folder(file, &format!("{stem} (pages)"), |d| {
                pdf_tools::split_pages(file, d, &stem, Some(pages), cancel)
            })?;
            note = n;
            r.message = format!("{} PDFs → {}", made.len(), name_of(&folder));
            r.outputs.push(folder);
        }
        PageOp::Rotate => {
            let (out, n) = emit(&dir, &format!("{stem} (rotated)"), "pdf", |tmp| {
                pdf_tools::rotate_pages(file, tmp, 90, Some(pages)).map(|_| ())
            })?;
            note = n;
            r.message = format!("Rotated pages {label} → {}", name_of(&out));
            r.outputs.push(out);
        }
        PageOp::Delete => {
            let keep = crate::pages::complement(pages, count);
            if keep.is_empty() {
                return Err(ToolError::Invalid(
                    "that's every page; keep at least one".into(),
                ));
            }
            let (out, n) = emit(&dir, &format!("{stem} (edited)"), "pdf", |tmp| {
                pdf_tools::extract_pages(file, &keep, tmp).map(|_| ())
            })?;
            note = n;
            r.message = format!(
                "Removed {} pages, {} left → {}",
                pages.len(),
                keep.len(),
                name_of(&out)
            );
            r.outputs.push(out);
        }
    }
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    Ok(r)
}

fn image_tool(
    tk: &Toolkit,
    tool: Tool,
    f: &Path,
) -> Result<Option<(PathBuf, Option<String>)>, ToolError> {
    let key = detect_key(tk, f)?;
    if !img::RASTER.contains(&key) {
        return Err(ToolError::Invalid(
            "not an image this build can edit".into(),
        ));
    }
    let stem = tk.registry.stem(f);
    let dir = parent_of(f);
    let out_key = if img::WRITABLE.contains(&key) {
        key
    } else {
        "png"
    };
    let ext = tk
        .registry
        .id(out_key)
        .map_or(out_key, |id| tk.registry.primary_ext(id));
    if tool == Tool::StripMetadata && key == "jpg" {
        let bytes = fs::read(f)?;
        if let Some(clean) = strip_jpeg(&bytes)
            && jpeg_upright(&bytes)
        {
            return emit(&dir, &format!("{stem} (clean)"), ext, |tmp| {
                fs::write(tmp, &clean).map_err(EngineError::from)
            })
            .map(Some);
        }
    }
    let image = img::load(f, key)?;
    let (w, h) = (image.width(), image.height());
    // Edits re-encode JPEG at high quality to keep generational loss low.
    let save = |i: &DynamicImage, tmp: &Path, q: u8| {
        img::save_with_quality(i, out_key, tmp, Preset::Best, q)
    };
    let (suffix, result): (&str, DynamicImage) = match tool {
        Tool::ResizeHalf => (
            "50%",
            image.resize_exact(
                (w / 2).max(1),
                (h / 2).max(1),
                image::imageops::FilterType::Lanczos3,
            ),
        ),
        Tool::Fit1920 => {
            if w.max(h) <= 1920 {
                return Ok(None);
            }
            (
                "1920px",
                image.resize(1920, 1920, image::imageops::FilterType::Lanczos3),
            )
        }
        Tool::RotateRight => ("rotated", image.rotate90()),
        Tool::Flip => ("flipped", image.fliph()),
        Tool::StripMetadata => ("clean", image),
        Tool::Compress => {
            let original = fs::metadata(f)?.len();
            let (scratch, _, _) = writable_dir(&dir)?;
            let tmp = scratch.path().join(format!("probe.{ext}"));
            save(&image, &tmp, 72)?;
            if fs::metadata(&tmp)?.len() >= original {
                return Ok(None);
            }
            return emit(&dir, &format!("{stem} (compressed)"), ext, |out| {
                fs::copy(&tmp, out).map(|_| ()).map_err(EngineError::from)
            })
            .map(Some);
        }
        _ => unreachable!("image_tool only handles image tools"),
    };
    emit(&dir, &format!("{stem} ({suffix})"), ext, |tmp| {
        save(&result, tmp, 92)
    })
    .map(Some)
}

/// True when a JPEG needs no EXIF rotation to display upright.
fn jpeg_upright(bytes: &[u8]) -> bool {
    image::ImageReader::new(io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.into_decoder().ok())
        .and_then(|mut d| image::ImageDecoder::orientation(&mut d).ok())
        .is_none_or(|o| o == image::metadata::Orientation::NoTransforms)
}

/// Remove EXIF/XMP (APP1), IPTC (APP13), comments and non-ICC APP2 from a JPEG without
/// touching the compressed image data. Keeps JFIF (APP0), ICC profiles and Adobe (APP14).
pub fn strip_jpeg(src: &[u8]) -> Option<Vec<u8>> {
    if !src.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = Vec::with_capacity(src.len());
    out.extend_from_slice(&[0xFF, 0xD8]);
    let mut i = 2;
    while i + 4 <= src.len() {
        if src[i] != 0xFF {
            return None;
        }
        let marker = src[i + 1];
        if marker == 0xFF {
            i += 1; // fill byte
            continue;
        }
        if marker == 0xDA {
            out.extend_from_slice(&src[i..]); // start of scan: the rest is image data
            return Some(out);
        }
        let len = u16::from_be_bytes([src[i + 2], src[i + 3]]) as usize;
        let end = i + 2 + len;
        if len < 2 || end > src.len() {
            return None;
        }
        let payload = &src[i + 4..end];
        let drop = match marker {
            0xE1 | 0xED | 0xFE => true,
            0xE2 => !payload.starts_with(b"ICC_PROFILE\0"),
            0xE3..=0xEC | 0xEF => true, // other app segments: camera maker data
            _ => false,
        };
        if !drop {
            out.extend_from_slice(&src[i..end]);
        }
        i = end;
    }
    None
}

fn sha256(p: &Path, cancel: &CancelToken) -> Result<String, ToolError> {
    let mut f = fs::File::open(p)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        cancel.check()?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Zip files and folders (recursively; symlinks are skipped). Returns files added.
fn zip_paths(paths: &[PathBuf], out: &Path, cancel: &CancelToken) -> Result<usize, EngineError> {
    let mut z = zip::ZipWriter::new(io::BufWriter::new(fs::File::create(out)?));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true);
    let mut count = 0;
    let mut stack: Vec<(PathBuf, String)> = paths.iter().map(|p| (p.clone(), name_of(p))).collect();
    while let Some((p, rel)) = stack.pop() {
        cancel.check()?;
        let meta = fs::symlink_metadata(&p)?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            z.add_directory(format!("{rel}/"), opts)
                .map_err(EngineError::encode)?;
            for e in fs::read_dir(&p)? {
                let e = e?;
                stack.push((
                    e.path(),
                    format!("{rel}/{}", e.file_name().to_string_lossy()),
                ));
            }
        } else {
            z.start_file(rel.as_str(), opts)
                .map_err(EngineError::encode)?;
            io::copy(&mut io::BufReader::new(fs::File::open(&p)?), &mut z)?;
            count += 1;
        }
    }
    z.finish().map_err(EngineError::encode)?.flush()?;
    Ok(count)
}

#[derive(Debug, Clone, Serialize)]
pub struct Extracted {
    pub dir: PathBuf,
    pub files: usize,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// "Extract here": unpack into a new folder named after the archive, next to it (or in
/// Downloads/Swirvo when that folder is read-only). On failure the half-written folder,
/// which only ever holds what we wrote, is removed.
pub fn extract_here(
    tk: &Toolkit,
    path: &Path,
    cancel: &CancelToken,
) -> Result<Extracted, ToolError> {
    let key = detect_key(tk, path).map_err(|_| ToolError::NotArchive)?;
    if tk
        .registry
        .id(key)
        .is_none_or(|id| tk.registry.get(id).category != Category::Archive)
        || !(archive::formats().contains(&key) || system_archive(key))
    {
        return Err(ToolError::NotArchive);
    }
    let stem = tk.registry.stem(path);
    let mut total = (0, 0);
    let (dir, _, note) = into_folder(path, &stem, |dir| {
        if archive::formats().contains(&key) {
            total = archive::extract(path, key, dir, cancel)?;
        } else {
            swirvo_engines::os::systar::extract(path, dir, cancel)?;
            total = count_tree(dir);
        }
        Ok(Vec::new())
    })?;
    Ok(Extracted {
        dir,
        files: total.0,
        bytes: total.1,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_runs_keep_file_order() {
        let files: Vec<PathBuf> = (0..40).map(|i| PathBuf::from(format!("{i}"))).collect();
        let c = CancelToken::new();
        let out = par_map(&files, 8, &c, |f| {
            let n: u64 = f.to_string_lossy().parse().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(40 - n));
            n
        });
        assert_eq!(out, (0..40).map(Some).collect::<Vec<_>>());
        c.cancel();
        assert!(par_map(&files, 4, &c, |_| 1).iter().all(Option::is_none));
        assert_eq!(workers(3, 8).min(3), workers(3, 8));
        assert_eq!(workers(0, 8), 1);
    }

    fn tk() -> Toolkit {
        Toolkit::with_builtin_engines().unwrap()
    }

    fn photo(p: &Path, w: u32, h: u32) {
        image::RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .save_with_format(p, image::ImageFormat::Jpeg)
        .unwrap();
    }

    #[test]
    fn which_tools_show_up() {
        let tk = tk();
        let t = tools_for(&tk, &[PathBuf::from("a.jpg")]);
        assert_eq!(
            &t[..3],
            &[Tool::ResizeHalf, Tool::Fit1920, Tool::RotateRight]
        );
        assert!(!t.contains(&Tool::ImagesToPdf));
        assert!(
            tools_for(&tk, &[PathBuf::from("a.jpg"), PathBuf::from("b.png")])
                .contains(&Tool::ImagesToPdf)
        );
        let p = tools_for(&tk, &[PathBuf::from("a.pdf"), PathBuf::from("b.pdf")]);
        assert_eq!(p[0], Tool::PdfMerge);
        assert!(tools_for(&tk, &[PathBuf::from("x.tar.xz")]).contains(&Tool::ExtractHere));
        assert!(tools_for(&tk, &[PathBuf::from("main.rs")]).contains(&Tool::CopyText));
        assert!(!tools_for(&tk, &[PathBuf::from("movie.mp4")]).contains(&Tool::CopyText));
        for t in Tool::ALL {
            assert_eq!(Tool::from_id(t.id()), Some(*t));
        }
    }

    #[test]
    fn image_tools_write_new_files_and_keep_originals() {
        let d = tempfile::tempdir().unwrap();
        let tk = tk();
        let src = d.path().join("pic.jpg");
        photo(&src, 400, 300);
        let before = fs::read(&src).unwrap();
        let cancel = CancelToken::new();
        let r = run_tool(&tk, Tool::ResizeHalf, std::slice::from_ref(&src), &cancel).unwrap();
        assert_eq!(r.outputs, vec![d.path().join("pic (50%).jpg")]);
        assert_eq!(image::open(&r.outputs[0]).unwrap().width(), 200);
        let r = run_tool(&tk, Tool::RotateRight, std::slice::from_ref(&src), &cancel).unwrap();
        let rot = image::open(&r.outputs[0]).unwrap();
        assert_eq!((rot.width(), rot.height()), (300, 400));
        // A second run never overwrites the first.
        let again = run_tool(&tk, Tool::RotateRight, std::slice::from_ref(&src), &cancel).unwrap();
        assert_eq!(again.outputs, vec![d.path().join("pic (rotated) (2).jpg")]);
        assert!(
            run_tool(&tk, Tool::Fit1920, std::slice::from_ref(&src), &cancel)
                .unwrap()
                .outputs
                .is_empty()
        );
        assert_eq!(fs::read(&src).unwrap(), before);
    }

    #[test]
    fn jpeg_strip_is_lossless_and_removes_exif() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("a.jpg");
        photo(&src, 32, 32);
        let mut bytes = fs::read(&src).unwrap();
        // Splice an EXIF APP1 with a GPS-ish payload right after SOI.
        let exif = b"Exif\0\0MM\0*\0\0\0\x08\0\0GPS-SECRET";
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        seg.extend_from_slice(exif);
        bytes.splice(2..2, seg);
        let clean = strip_jpeg(&bytes).unwrap();
        assert!(!clean.windows(10).any(|w| w == b"GPS-SECRET"));
        let a = image::load_from_memory(&bytes).unwrap().to_rgb8();
        let b = image::load_from_memory(&clean).unwrap().to_rgb8();
        assert_eq!(a, b, "pixels unchanged");
    }

    #[test]
    fn pdf_and_zip_and_checksum_tools() {
        let d = tempfile::tempdir().unwrap();
        let tk = tk();
        let cancel = CancelToken::new();
        let (a, b) = (d.path().join("a.jpg"), d.path().join("b.jpg"));
        photo(&a, 120, 80);
        photo(&b, 80, 120);
        let r = run_tool(&tk, Tool::ImagesToPdf, &[b.clone(), a.clone()], &cancel).unwrap();
        let pdf = r.outputs[0].clone();
        assert_eq!(pdf, d.path().join("a (combined).pdf"));
        let r = run_tool(&tk, Tool::PdfSplit, std::slice::from_ref(&pdf), &cancel).unwrap();
        assert_eq!(fs::read_dir(&r.outputs[0]).unwrap().count(), 2);
        let r = run_tool(&tk, Tool::PdfToPng, std::slice::from_ref(&pdf), &cancel).unwrap();
        assert!(r.outputs[0].ends_with("a (combined) (images)"));

        let r = run_tool(
            &tk,
            Tool::ZipFiles,
            &[a.clone(), r.outputs[0].clone()],
            &cancel,
        )
        .unwrap();
        assert!(r.message.starts_with("3 files"), "{}", r.message);
        let r2 = run_tool(&tk, Tool::TestArchive, &r.outputs, &cancel).unwrap();
        assert!(r2.message.contains("OK, 3 files"), "{}", r2.message);

        let r = run_tool(&tk, Tool::Checksum, std::slice::from_ref(&a), &cancel).unwrap();
        let line = r.clipboard.unwrap();
        assert_eq!(line.len(), 64 + 2 + "a.jpg".len());
    }

    #[test]
    fn page_ops_write_new_files() {
        let d = tempfile::tempdir().unwrap();
        let tk = tk();
        let cancel = CancelToken::new();
        let imgs: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = d.path().join(format!("{i}.jpg"));
                photo(&p, 90 + i * 10, 120);
                p
            })
            .collect();
        let pdf = run_tool(&tk, Tool::ImagesToPdf, &imgs, &cancel)
            .unwrap()
            .outputs[0]
            .clone();
        let before = fs::read(&pdf).unwrap();
        let op = |op, pages: &[usize]| run_page_op(&tk, &pdf, op, pages, &cancel);

        let r = op(PageOp::Extract, &[0, 2]).unwrap();
        assert!(
            r.outputs[0].ends_with("0 (combined) (pages 1, 3).pdf"),
            "{:?}",
            r.outputs
        );
        assert_eq!(pdf_tools::page_count(&r.outputs[0]).unwrap(), 2);

        let r = op(PageOp::Delete, &[1]).unwrap();
        assert!(r.outputs[0].ends_with("0 (combined) (edited).pdf"));
        assert_eq!(pdf_tools::page_count(&r.outputs[0]).unwrap(), 2);
        assert!(op(PageOp::Delete, &[0, 1, 2]).is_err());

        let r = op(
            PageOp::Images {
                png: false,
                dpi: 72,
            },
            &[1],
        )
        .unwrap();
        assert!(
            r.outputs[0].ends_with("0 (combined) p02.jpg"),
            "{:?}",
            r.outputs
        );
        let r = op(PageOp::Images { png: true, dpi: 72 }, &[0, 1]).unwrap();
        assert_eq!(fs::read_dir(&r.outputs[0]).unwrap().count(), 2);

        let r = op(PageOp::Split, &[2]).unwrap();
        assert_eq!(fs::read_dir(&r.outputs[0]).unwrap().count(), 1);
        let r = op(PageOp::Rotate, &[0]).unwrap();
        assert!(r.outputs[0].ends_with("0 (combined) (rotated).pdf"));
        assert!(op(PageOp::Extract, &[]).is_err());
        assert!(op(PageOp::Extract, &[7]).is_err());
        // The source is never touched.
        assert_eq!(fs::read(&pdf).unwrap(), before);
        assert_eq!(
            PageOp::Images {
                png: false,
                dpi: 150
            }
            .verb(3),
            "Export 3 pages as JPG"
        );
    }

    #[test]
    fn extracts_beside_the_archive_without_clobbering() {
        let d = tempfile::tempdir().unwrap();
        let zip_path = d.path().join("Assets.zip");
        {
            let mut z = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
            z.start_file("a.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b"swirvo sample").unwrap();
            z.finish().unwrap();
        }
        fs::create_dir(d.path().join("Assets")).unwrap();
        fs::write(d.path().join("Assets").join("a.txt"), b"mine").unwrap();

        let tk = tk();
        let out = extract_here(&tk, &zip_path, &CancelToken::new()).unwrap();
        assert_eq!(out.dir, d.path().join("Assets (2)"));
        assert_eq!((out.files, out.bytes), (1, 13));
        assert_eq!(
            fs::read(d.path().join("Assets").join("a.txt")).unwrap(),
            b"mine"
        );
        assert_eq!(fs::read(out.dir.join("a.txt")).unwrap(), b"swirvo sample");

        let not = d.path().join("x.txt");
        fs::write(&not, b"hi").unwrap();
        assert!(matches!(
            extract_here(&tk, &not, &CancelToken::new()),
            Err(ToolError::NotArchive)
        ));
    }

    fn letter(p: &Path, body: &str) {
        let parts = [
            ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_string()),
            ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_string()),
            ("word/document.xml", format!(r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/></w:sectPr></w:body></w:document>"#)),
        ];
        let mut z = zip::ZipWriter::new(fs::File::create(p).unwrap());
        for (n, b) in parts {
            z.start_file(n, zip::write::SimpleFileOptions::default())
                .unwrap();
            z.write_all(b.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn word_tools_write_new_documents() {
        let tk = tk();
        let d = tempfile::tempdir().unwrap();
        let doc = d.path().join("letter.docx");
        letter(
            &doc,
            "<w:p><w:r><w:t>Dear {{Name}},</w:t></w:r></w:p><w:p><w:r><w:t>hello hel</w:t></w:r><w:r><w:t>lo world</w:t></w:r></w:p>",
        );
        let before = fs::read(&doc).unwrap();
        let tools = tools_for(&tk, std::slice::from_ref(&doc));
        assert!(tools.contains(&Tool::DocxReplace) && tools.contains(&Tool::DocxMailMerge));
        assert!(!tools.contains(&Tool::PdfCompress));

        let r = run_tool(
            &tk,
            Tool::DocxStats,
            std::slice::from_ref(&doc),
            &CancelToken::new(),
        )
        .unwrap();
        assert!(r.message.contains("5 words"), "{}", r.message);
        assert!(r.clipboard.is_some());

        let mut args = ToolArgs::new();
        args.insert("find".into(), "hello".into());
        args.insert("with".into(), "bye".into());
        let r = run_tool_with(
            &tk,
            Tool::DocxReplace,
            std::slice::from_ref(&doc),
            &args,
            &CancelToken::new(),
        )
        .unwrap();
        assert!(r.message.starts_with("Replaced 2 times"), "{}", r.message);
        assert_eq!(name_of(&r.outputs[0]), "letter (edited).docx");
        // Find is required.
        assert!(
            run_tool(
                &tk,
                Tool::DocxReplace,
                std::slice::from_ref(&doc),
                &CancelToken::new()
            )
            .is_err()
        );

        let csv = d.path().join("people.csv");
        fs::write(&csv, "Name,City\nAda,London\nBob,Paris\n").unwrap();
        let both = [doc.clone(), csv.clone()];
        assert!(tools_for(&tk, &both).contains(&Tool::DocxMailMerge));
        let mut args = ToolArgs::new();
        args.insert("name_field".into(), "Name".into());
        let r = run_tool_with(&tk, Tool::DocxMailMerge, &both, &args, &CancelToken::new()).unwrap();
        assert!(r.message.starts_with("2 letters"), "{}", r.message);
        let dir = &r.outputs[0];
        assert!(dir.join("letter Ada.docx").exists() && dir.join("letter Bob.docx").exists());
        assert_eq!(fs::read(&doc).unwrap(), before, "the original is untouched");
    }
}
