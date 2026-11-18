//! SVG → PNG with resvg (pure Rust). Other raster targets and PDF chain from PNG through the
//! image engine, so SVG → JPG/WebP/ICO/PDF all work via the planner.

use resvg::{tiny_skia, usvg};
use swirvo_core::{Capability, Engine, EngineError, StepRequest};

/// Longest side of the rendered bitmap.
const MAX_SIDE: f32 = 8192.0;

pub struct SvgEngine;

impl Engine for SvgEngine {
    fn id(&self) -> &'static str {
        "svg"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("svg", "svg", "png")
                .loss(2)
                .time(60, 200)
                .note("rendered at 2× (vector → pixels)"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let data = std::fs::read(req.input)?;
        let mut opt = usvg::Options::default();
        // Text in SVGs needs fonts. No resources_dir: external files referenced by the SVG
        // are never loaded.
        opt.fontdb_mut().load_system_fonts();
        let tree = usvg::Tree::from_data(&data, &opt).map_err(EngineError::decode)?;
        req.cancel.check()?;
        let size = tree.size();
        let scale = (2.0f32).min(MAX_SIDE / size.width().max(size.height()).max(1.0));
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
        let png = pm.encode_png().map_err(EngineError::encode)?;
        std::fs::write(req.output, png)?;
        Ok(())
    }
}

/// Render an SVG to fit `max_side` px (thumbnails). Straight-alpha RGBA.
pub fn render_fit(path: &std::path::Path, max_side: u32) -> Result<image::RgbaImage, EngineError> {
    let data = std::fs::read(path)?;
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_data(&data, &opt).map_err(EngineError::decode)?;
    let size = tree.size();
    let scale = max_side as f32 / size.width().max(size.height()).max(1.0);
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

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_core::{CancelToken, Preset};

    #[test]
    fn renders_at_twice_the_size() {
        let d = tempfile::tempdir().unwrap();
        let (i, o) = (d.path().join("a.svg"), d.path().join("a.png"));
        std::fs::write(&i, r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20"><rect width="40" height="20" fill="#e8772e"/></svg>"##).unwrap();
        let cancel = CancelToken::new();
        SvgEngine
            .run(&StepRequest {
                input: &i,
                output: &o,
                from: "svg",
                to: "png",
                preset: Preset::Balanced,
                pdf: Default::default(),
                cancel: &cancel,
            })
            .unwrap();
        let img = image::open(&o).unwrap();
        assert_eq!((img.width(), img.height()), (80, 40));
    }
}
