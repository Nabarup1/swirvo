//! The Board and the page picker (docs/12 §3–4), as portable [`swirvo_ui::App`]s.
//! Hosts: the Win32 window in `platform/win`; snapshots in tests and `examples/snapshot`.

pub mod board;
pub mod pages;
mod preview;
pub mod runner;
pub mod settings;
pub mod thumbs;

use std::path::Path;
use std::sync::Arc;

use swirvo_core::Category;
use swirvo_jobs::Toolkit;
use swirvo_ui::Kind;

pub use board::Board;
pub use pages::PagePicker;
pub use settings::{InstallState, SettingsApp};

pub fn kind_of(c: Category) -> Kind {
    match c {
        Category::Image => Kind::Image,
        Category::Pdf
        | Category::Document
        | Category::Slides
        | Category::Ebook
        | Category::Text => Kind::Document,
        Category::Audio => Kind::Audio,
        Category::Video => Kind::Video,
        Category::Archive => Kind::Archive,
        Category::Data | Category::Spreadsheet | Category::Subtitle => Kind::Data,
        Category::Font => Kind::Font,
    }
}

pub fn human(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.0} KB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A Word, PowerPoint, Excel or OpenDocument preview gives up after this long, so one
/// huge document can't hold a thumbnail worker.
const PREVIEW_LIMIT: std::time::Duration = std::time::Duration::from_secs(6);

/// The default thumbnail decoder: raster images (EXIF-oriented), SVG, PDF page 1, and
/// page 1 of Word, PowerPoint, Excel, ODT and RTF files laid out by our own engines.
pub fn decoder(tk: Arc<Toolkit>) -> thumbs::Decoder {
    Arc::new(move |path: &Path, max: u32| {
        let reg = tk.registry;
        let key = reg.from_path(path).map(|f| reg.key(f))?;
        match key {
            "pdf" => {
                let r = swirvo_engines::pdf_tools::PageRenderer::open(path).ok()?;
                r.session(|s| s.render_fit(0, max, max)).ok()
            }
            "svg" => swirvo_engines::svg::render_fit(path, max).ok(),
            k if swirvo_engines::preview::KINDS.contains(&k) => {
                swirvo_engines::preview::first_page(path, k, max, PREVIEW_LIMIT).ok()
            }
            k if swirvo_engines::image::RASTER.contains(&k) => {
                let img = swirvo_engines::image::load(path, k).ok()?;
                Some(img.thumbnail(max, max).to_rgba8())
            }
            _ => None,
        }
    })
}

/// The Swirvo mark for title bars.
pub fn app_icon(px: u32) -> Option<swirvo_ui::Pixmap> {
    let pm = swirvo_render::app_icon(px)?;
    swirvo_ui::Pixmap::from_vec(pm.data().to_vec(), swirvo_ui::PixSize::from_wh(px, px)?)
}
