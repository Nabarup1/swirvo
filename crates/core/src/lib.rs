//! Domain types shared by every Swirvo crate.
//!
//! This crate has no I/O and no OS dependencies (docs/03 §5): the planner, scheduler and
//! wheel model build and test on any host.

use std::fmt;
use std::ops::Add;
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};

/// Broad family of a format. Drives wheel colors, tool lists and priors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Image,
    Video,
    Audio,
    Pdf,
    Document,
    Slides,
    Spreadsheet,
    Data,
    Ebook,
    Subtitle,
    Archive,
    Font,
    Text,
}

impl Category {
    pub const fn name(self) -> &'static str {
        match self {
            Category::Image => "image",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Pdf => "pdf",
            Category::Document => "document",
            Category::Slides => "slides",
            Category::Spreadsheet => "spreadsheet",
            Category::Data => "data",
            Category::Ebook => "ebook",
            Category::Subtitle => "subtitle",
            Category::Archive => "archive",
            Category::Font => "font",
            Category::Text => "text",
        }
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Format traits as a tiny bitset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Traits(pub u16);

impl Traits {
    pub const NONE: Traits = Traits(0);
    pub const LOSSY: Traits = Traits(1 << 0);
    pub const ALPHA: Traits = Traits(1 << 1);
    pub const ANIMATION: Traits = Traits(1 << 2);
    pub const MULTIPAGE: Traits = Traits(1 << 3);
    pub const HDR: Traits = Traits(1 << 4);
    pub const CONTAINER: Traits = Traits(1 << 5);
    pub const TEXT: Traits = Traits(1 << 6);
    /// Zip-based containers (OOXML, ODF, EPUB): content sniffing may only say "zip".
    pub const ZIP_FAMILY: Traits = Traits(1 << 7);

    pub const fn with(self, other: Traits) -> Traits {
        Traits(self.0 | other.0)
    }
    pub const fn has(self, other: Traits) -> bool {
        self.0 & other.0 == other.0
    }
}

/// Dense index of a format inside the registry; used directly as a bit position in
/// reachability sets, so the registry is capped at 256 formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FormatId(pub u16);

impl FormatId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// Cost vector of a conversion edge or route. Presets decide how components compare
/// (docs/04 §2); the default ordering is (loss, download, time).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cost {
    /// 0 = no quality loss. Each lossy re-encode adds its penalty, so two lossy hops
    /// always cost more than one.
    pub loss: u32,
    /// Penalty for needing an engine pack that is not installed.
    pub download: u32,
    /// Estimated milliseconds.
    pub time_ms: u32,
}

impl Add for Cost {
    type Output = Cost;
    fn add(self, o: Cost) -> Cost {
        Cost {
            loss: self.loss.saturating_add(o.loss),
            download: self.download.saturating_add(o.download),
            time_ms: self.time_ms.saturating_add(o.time_ms),
        }
    }
}

/// Quality/route preset chosen in the sub-ring or with `--preset`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Preset {
    Fastest,
    #[default]
    Balanced,
    Best,
    Smallest,
}

impl Preset {
    /// Lexicographic key used by the planner. `hops` breaks ties so shorter routes win.
    pub fn rank(self, c: Cost, hops: u32) -> (u32, u32, u32, u32) {
        match self {
            Preset::Balanced | Preset::Smallest => (c.loss, c.download, c.time_ms, hops),
            Preset::Best => (c.loss, c.time_ms, c.download, hops),
            Preset::Fastest => (c.download, c.time_ms, c.loss, hops),
        }
    }

    /// Encoder quality (0–100) for lossy targets that take a single quality knob.
    pub fn quality(self) -> u8 {
        match self {
            Preset::Best => 92,
            Preset::Balanced | Preset::Fastest => 85,
            Preset::Smallest => 72,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Preset::Fastest => "fastest",
            Preset::Balanced => "balanced",
            Preset::Best => "best",
            Preset::Smallest => "smallest",
        }
    }
}

impl FromStr for Preset {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "fastest" | "fast" => Ok(Preset::Fastest),
            "balanced" | "default" => Ok(Preset::Balanced),
            "best" | "high" => Ok(Preset::Best),
            "smallest" | "small" => Ok(Preset::Smallest),
            other => Err(format!(
                "unknown preset '{other}' (fastest, balanced, best, smallest)"
            )),
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Lossless shortcuts the planner prefers automatically (docs/05 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FastPath {
    /// Container change with stream copy.
    Remux,
    /// Bytes embedded without re-encoding (e.g. JPEG inside a PDF).
    Passthrough,
    /// Entries or objects copied between containers (archives, PDF pages).
    ObjectCopy,
    /// Lossless recompression (e.g. JPEG → JPEG XL).
    LosslessRecompress,
}

/// One edge of the conversion graph, declared by an engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub engine: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    /// Quality penalty of this hop; 0 = lossless.
    pub loss: u32,
    pub fixed_ms: u32,
    pub ms_per_mb: u32,
    /// Engine pack needed, if it is not part of Core.
    pub pack: Option<&'static str>,
    pub fast_path: Option<FastPath>,
    /// One-line technique note for hover bubbles and `plan --explain`.
    pub note: &'static str,
}

impl Capability {
    pub const fn new(engine: &'static str, from: &'static str, to: &'static str) -> Self {
        Capability {
            engine,
            from,
            to,
            loss: 0,
            fixed_ms: 20,
            ms_per_mb: 40,
            pack: None,
            fast_path: None,
            note: "",
        }
    }
    pub const fn loss(mut self, loss: u32) -> Self {
        self.loss = loss;
        self
    }
    pub const fn time(mut self, fixed_ms: u32, ms_per_mb: u32) -> Self {
        self.fixed_ms = fixed_ms;
        self.ms_per_mb = ms_per_mb;
        self
    }
    pub const fn fast(mut self, fp: FastPath) -> Self {
        self.fast_path = Some(fp);
        self
    }
    pub const fn pack(mut self, pack: &'static str) -> Self {
        self.pack = Some(pack);
        self
    }
    pub const fn note(mut self, note: &'static str) -> Self {
        self.note = note;
        self
    }

    /// Cost of running this edge on `mb` megabytes of input.
    pub fn cost(&self, mb: f64, pack_installed: bool) -> Cost {
        let t = self.fixed_ms as f64 + self.ms_per_mb as f64 * mb.max(0.0);
        Cost {
            loss: self.loss,
            download: if self.pack.is_some() && !pack_installed {
                1
            } else {
                0
            },
            time_ms: t.min(u32::MAX as f64) as u32,
        }
    }
}

/// Cooperative cancellation shared by a batch, its jobs and their steps.
#[derive(Debug, Default)]
pub struct CancelToken(AtomicBool);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    /// Checkpoint for long loops in engines.
    pub fn check(&self) -> Result<(), EngineError> {
        if self.is_cancelled() {
            Err(EngineError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// How PDFs we write should be made (docs/17 §2). Engines that don't write PDFs, or
/// that hand the work to another program, ignore it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PdfOptions {
    /// PDF/A-2 for long-term archiving: every font embedded, colours device-independent,
    /// XMP metadata. With `tagged`, PDF/A-2a; otherwise PDF/A-2b.
    pub archival: bool,
    /// Tagged (accessible) PDF: headings, paragraphs, lists, tables and figures with
    /// their alternative text, in reading order, with the document's language.
    pub tagged: bool,
    /// With `archival`: PDF/A-3 instead of PDF/A-2, with the source document embedded
    /// in the PDF as its associated file (relationship *Source*), so the original
    /// travels with the archive copy.
    pub attach_source: bool,
    /// PDF/UA-1 (universal accessibility): tagged, with a title shown in the viewer's
    /// title bar, an outline, alternative text on every figure. One standard per file:
    /// it takes the place of `archival`.
    pub accessible: bool,
}

/// What an engine is asked to do for one step of a plan.
#[derive(Debug)]
pub struct StepRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    /// Format keys from the registry, e.g. "heic", "tar.gz".
    pub from: &'a str,
    pub to: &'a str,
    pub preset: Preset,
    pub pdf: PdfOptions,
    pub cancel: &'a CancelToken,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{engine} can't convert {from} to {to}")]
    Unsupported {
        engine: &'static str,
        from: String,
        to: String,
    },
    #[error("couldn't read the input: {0}")]
    Decode(String),
    #[error("couldn't write the output: {0}")]
    Encode(String),
    #[error("{0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
}

impl EngineError {
    pub fn decode(e: impl fmt::Display) -> Self {
        EngineError::Decode(e.to_string())
    }
    pub fn encode(e: impl fmt::Display) -> Self {
        EngineError::Encode(e.to_string())
    }
}

/// A conversion engine. Engines run inside worker processes (docs/03 §8); in the CLI's
/// standalone mode they run in-process.
pub trait Engine: Send + Sync {
    fn id(&self) -> &'static str;
    fn capabilities(&self) -> Vec<Capability>;
    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_adds_saturating() {
        let a = Cost {
            loss: u32::MAX,
            download: 1,
            time_ms: 5,
        };
        let b = Cost {
            loss: 1,
            download: 0,
            time_ms: 7,
        };
        assert_eq!(
            a + b,
            Cost {
                loss: u32::MAX,
                download: 1,
                time_ms: 12
            }
        );
    }

    #[test]
    fn presets_order_components_differently() {
        let lossless_slow = Cost {
            loss: 0,
            download: 0,
            time_ms: 900,
        };
        let lossy_fast = Cost {
            loss: 30,
            download: 0,
            time_ms: 10,
        };
        assert!(Preset::Balanced.rank(lossless_slow, 1) < Preset::Balanced.rank(lossy_fast, 1));
        assert!(Preset::Fastest.rank(lossy_fast, 1) < Preset::Fastest.rank(lossless_slow, 1));
    }

    #[test]
    fn traits_bitset() {
        let t = Traits::LOSSY.with(Traits::ALPHA);
        assert!(t.has(Traits::LOSSY) && t.has(Traits::ALPHA) && !t.has(Traits::HDR));
    }
}
