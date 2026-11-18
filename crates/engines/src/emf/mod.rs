//! Windows metafiles (EMF and WMF), drawn as vectors (docs/16 §12).
//!
//! Office files embed them for logos, charts pasted from other programs, equation
//! previews and OLE object pictures. Playback ([`gdi`]) follows GDI: mapping modes,
//! window and viewport, world transforms, saved states, the object table, paths and
//! clipping. The result is a [`Picture`]: a list of fills, strokes, text, bitmaps and
//! clips in points. The layout engines draw it through the shared PDF writer
//! ([`ops`]), and it converts on its own to SVG, PNG and PDF.
//!
//! Untrusted input: every record is size-checked, counts are validated against the
//! record, and playback stops at fixed limits on records, points, items and pixels.

mod dib;
mod emr;
mod gdi;
pub(crate) mod ops;
mod svg;
#[cfg(test)]
mod tests;
mod wmr;

use std::sync::Arc;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

use crate::docxlayout::layout::{Op, PathCmd, Pen};
use crate::docxlayout::props::Rgb;

/// Something drawn, in picture coordinates (points, y down, origin top-left).
#[derive(Debug, Clone)]
pub enum Item {
    Fill {
        path: Arc<Vec<PathCmd>>,
        color: Rgb,
        alpha: f32,
        even_odd: bool,
    },
    Stroke {
        path: Arc<Vec<PathCmd>>,
        pen: Pen,
    },
    /// A linear gradient from `c0` at `from` to `c1` at `to`.
    Gradient {
        path: Arc<Vec<PathCmd>>,
        from: (f32, f32),
        to: (f32, f32),
        c0: Rgb,
        c1: Rgb,
    },
    Text(Box<TextItem>),
    /// A PNG drawn over the unit square mapped by `m`.
    Image {
        png: Arc<Vec<u8>>,
        m: [f32; 6],
    },
    /// Clip what follows, up to the matching [`Item::Unclip`].
    Clip {
        path: Arc<Vec<PathCmd>>,
        even_odd: bool,
    },
    Unclip,
}

/// One text record. Text space: the reference point at the origin, x along the
/// baseline, y down, in the metafile's logical units; `m` maps it into the picture.
#[derive(Debug, Clone)]
pub struct TextItem {
    pub m: [f32; 6],
    pub text: String,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// Em size, or the cell height (ascent + descent) when `cell`.
    pub size: f32,
    pub cell: bool,
    pub color: Rgb,
    /// 0 left, 1 centre, 2 right of the reference point.
    pub halign: u8,
    /// The reference point is the text's 0 top, 1 baseline, 2 bottom.
    pub valign: u8,
    /// Per-character advances (empty: the font's own).
    pub dx: Vec<f32>,
}

/// A played-back metafile: its natural size in points and what it draws.
#[derive(Debug, Clone, Default)]
pub struct Picture {
    pub w: f32,
    pub h: f32,
    pub items: Vec<Item>,
}

/// EMF or WMF (placeable or not)?
pub fn is_metafile(b: &[u8]) -> bool {
    emr::is_emf(b) || wmr::is_wmf(b)
}

/// Pictures the layout engines can draw: anything `image` decodes, and metafiles.
pub fn drawable(b: &[u8]) -> bool {
    image::guess_format(b).is_ok() || is_metafile(b)
}

/// A metafile as a standalone SVG document (pictures in web pages and e-books).
pub(crate) fn to_svg_string(b: &[u8]) -> Option<String> {
    parse(b).ok().map(|p| svg::to_svg(&p))
}

pub(crate) use svg::base64;

/// Play a metafile back.
pub fn parse(b: &[u8]) -> Result<Picture, EngineError> {
    let pic = std::panic::catch_unwind(|| {
        if emr::is_emf(b) {
            emr::parse(b)
        } else {
            wmr::parse(b)
        }
    })
    .map_err(|_| EngineError::Decode("couldn't read this metafile".into()))??;
    Ok(pic)
}

/// A metafile as layout ops in its own coordinates, with its natural size: what the PDF
/// writer draws in place of a picture. None when nothing can be drawn.
pub(crate) fn layout_ops(
    b: &[u8],
    fonts: &mut crate::docxlayout::fonts::Fonts,
) -> Option<Arc<(Vec<Op>, f32, f32)>> {
    let pic = parse(b).ok()?;
    if pic.items.is_empty() {
        return None;
    }
    Some(Arc::new((ops::ops(&pic, fonts), pic.w, pic.h)))
}

/// Render to an RGBA bitmap through SVG (resvg), `scale` pixels per point.
fn rasterize(pic: &Picture, scale: f32) -> Result<image::RgbaImage, EngineError> {
    use resvg::{tiny_skia, usvg};
    let svg = svg::to_svg(pic);
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(svg.as_bytes(), &opt).map_err(EngineError::decode)?;
    let size = tree.size();
    // The SVG is sized in points, which usvg turns into CSS pixels (96 per inch).
    let scale = scale * 72.0 / 96.0;
    let scale = scale.min(8192.0 / size.width().max(size.height()).max(1.0));
    let (w, h) = (
        (size.width() * scale).ceil().max(1.0) as u32,
        (size.height() * scale).ceil().max(1.0) as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w, h)
        .ok_or_else(|| EngineError::Invalid(format!("can't allocate {w}×{h}")))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pm.as_mut(),
    );
    let mut px = pm.take();
    for p in px.as_chunks_mut::<4>().0 {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    image::RgbaImage::from_raw(w, h, px).ok_or_else(|| EngineError::Encode("bad bitmap".into()))
}

/// A one-page PDF of the picture at its natural size.
fn to_pdf(
    bytes: &[u8],
    pic: &Picture,
    title: &str,
    out: &std::path::Path,
) -> Result<(), EngineError> {
    use crate::docxlayout::layout::PageOut;
    use crate::docxlayout::model::Media;
    let mut fonts = crate::docxlayout::fonts::Fonts::new(Default::default());
    let page = PageOut {
        w: pic.w,
        h: pic.h,
        ops: vec![Op::Image {
            media: Arc::new(Media {
                bytes: Arc::new(bytes.to_vec()),
            }),
            x: 0.0,
            y: 0.0,
            w: pic.w,
            h: pic.h,
            crop: [0.0; 4],
        }],
        ..Default::default()
    };
    crate::docxlayout::render::render(&[page], &mut fonts, &Default::default(), title, out)
}

pub struct MetafileEngine;

impl Engine for MetafileEngine {
    fn id(&self) -> &'static str {
        "metafile"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut v = Vec::new();
        for from in ["emf", "wmf"] {
            v.push(
                Capability::new("metafile", from, "svg")
                    .loss(1)
                    .time(30, 60)
                    .note("vector paths, text and pictures"),
            );
            v.push(
                Capability::new("metafile", from, "pdf")
                    .loss(1)
                    .time(40, 60)
                    .note("vector, with fonts embedded"),
            );
            v.push(
                Capability::new("metafile", from, "png")
                    .loss(3)
                    .time(60, 150)
                    .note("rendered at 2× (vector → pixels)"),
            );
        }
        v
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let bytes = std::fs::read(req.input)?;
        let pic = parse(&bytes)?;
        req.cancel.check()?;
        match req.to {
            "svg" => std::fs::write(req.output, svg::to_svg(&pic))?,
            "png" => {
                let img = rasterize(&pic, 2.0)?;
                img.save_with_format(req.output, image::ImageFormat::Png)
                    .map_err(EngineError::encode)?;
            }
            "pdf" => {
                let title = req
                    .input
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                to_pdf(&bytes, &pic, &title, req.output)?;
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "metafile engine can't write {other}"
                )));
            }
        }
        Ok(())
    }
}
