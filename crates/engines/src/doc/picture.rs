//! Pictures for the word-processor and PDF writers: a file format the target can hold and
//! a size on the page.
//!
//! PNG, JPEG and GIF go in as they are. Other bitmaps (BMP, TIFF, WebP…) become PNG. EMF
//! and WMF stay vectors where the target draws them (Word, ODF) and are rendered
//! otherwise, like SVG. A picture that can't be read is left out.

use resvg::{tiny_skia, usvg};

use super::Picture;

/// A picture as placed in a document.
pub(crate) struct Placed {
    pub mime: &'static str,
    pub ext: &'static str,
    pub bytes: Vec<u8>,
    /// Size on the page in points, within the text area.
    pub w: f32,
    pub h: f32,
}

/// Bitmaps without a resolution are taken to be 96 dpi, like Word and browsers do.
const PX_PT: f32 = 0.75;
/// Longest side of a rendered vector picture, in pixels.
const MAX_RENDER: f32 = 4096.0;

/// The picture in a format the target reads, sized as placed in the source (or its own
/// size), scaled down to fit `max_w` × `max_h` points. `metafiles`: the target draws EMF
/// and WMF itself.
pub(crate) fn place(p: &Picture, max_w: f32, max_h: f32, metafiles: bool) -> Option<Placed> {
    use image::ImageFormat as F;
    let b = p.bytes.as_slice();
    let (mime, ext, bytes, nw, nh) = match image::guess_format(b) {
        Ok(fmt) => {
            let (w, h) = image::ImageReader::with_format(std::io::Cursor::new(b), fmt)
                .into_dimensions()
                .ok()?;
            let (w, h) = (w as f32 * PX_PT, h as f32 * PX_PT);
            match fmt {
                F::Png => ("image/png", "png", b.to_vec(), w, h),
                F::Jpeg => ("image/jpeg", "jpg", b.to_vec(), w, h),
                F::Gif => ("image/gif", "gif", b.to_vec(), w, h),
                _ => {
                    let img = image::load_from_memory(b).ok()?;
                    let mut png = Vec::new();
                    img.write_to(&mut std::io::Cursor::new(&mut png), F::Png)
                        .ok()?;
                    ("image/png", "png", png, w, h)
                }
            }
        }
        Err(_) if crate::emf::is_metafile(b) => {
            let pic = crate::emf::parse(b).ok()?;
            if pic.items.is_empty() {
                return None;
            }
            if metafiles {
                let (mime, ext) = if b.get(..4) == Some(&[1, 0, 0, 0]) {
                    ("image/x-emf", "emf")
                } else {
                    ("image/x-wmf", "wmf")
                };
                (mime, ext, b.to_vec(), pic.w, pic.h)
            } else {
                let svg = crate::emf::to_svg_string(b)?;
                let (png, w, h) = svg_png(svg.as_bytes())?;
                ("image/png", "png", png, w, h)
            }
        }
        Err(_) if super::write::is_svg(b) => {
            let (png, w, h) = svg_png(b)?;
            ("image/png", "png", png, w, h)
        }
        Err(_) => return None,
    };
    if !(nw.is_finite() && nh.is_finite()) || nw < 0.5 || nh < 0.5 {
        return None;
    }
    let mut w = p.width_pt.map_or(nw, |w| w as f32);
    let mut h = w * nh / nw;
    let k = (max_w / w).min(max_h / h).min(1.0);
    w *= k;
    h *= k;
    Some(Placed {
        mime,
        ext,
        bytes,
        w,
        h,
    })
}

/// An SVG rendered at 2×, with its size in points. Nothing outside the SVG is loaded.
pub(crate) fn svg_png(svg: &[u8]) -> Option<(Vec<u8>, f32, f32)> {
    let mut opt = usvg::Options::default();
    opt.image_href_resolver.resolve_string = Box::new(|_, _| None);
    opt.fontdb = fonts();
    let tree = usvg::Tree::from_data(svg, &opt).ok()?;
    let size = tree.size();
    // usvg sizes are CSS pixels.
    let scale = 2.0f32.min(MAX_RENDER / size.width().max(size.height()).max(1.0));
    let (w, h) = (
        (size.width() * scale).ceil().max(1.0) as u32,
        (size.height() * scale).ceil().max(1.0) as u32,
    );
    let mut pm = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pm.as_mut(),
    );
    let png = pm.encode_png().ok()?;
    Some((png, size.width() * PX_PT, size.height() * PX_PT))
}

/// The system fonts, loaded once (text in metafiles and SVGs).
fn fonts() -> std::sync::Arc<usvg::fontdb::Database> {
    static DB: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
    .clone()
}

/// Pictures written once per document however often they appear: the package name of
/// each, keyed by the shared bytes.
#[derive(Default)]
pub(crate) struct Media {
    pub files: Vec<(String, &'static str, Vec<u8>)>,
    seen: std::collections::HashMap<(*const Vec<u8>, &'static str), usize>,
}

impl Media {
    /// This picture's index in `files` and its file name under `dir`
    /// (`media/image1.png`).
    pub fn add(&mut self, p: &Picture, placed: &mut Placed, dir: &str) -> (usize, String) {
        let key = (std::sync::Arc::as_ptr(&p.bytes), placed.ext);
        if let Some(&k) = self.seen.get(&key) {
            return (k, self.files[k].0.clone());
        }
        let k = self.files.len();
        let name = format!("{dir}image{}.{}", k + 1, placed.ext);
        self.files
            .push((name.clone(), placed.mime, std::mem::take(&mut placed.bytes)));
        self.seen.insert(key, k);
        (k, name)
    }
}
