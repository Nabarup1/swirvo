//! PDF tools (docs/01 §8): merge, split, rotate, pages → images, images → PDF.
//!
//! Merge and split re-embed whole pages with krilla (content, fonts and annotations are
//! copied, not re-rendered). Rotate only changes each page's /Rotate entry (lopdf), so it's
//! lossless. Rendering uses hayro, a pure-Rust rasterizer.

use std::fs::OpenOptions;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use image::{DynamicImage, ImageFormat};
use krilla::configure::{Configuration, PdfVersion};
use krilla::geom::{Size, Transform};
use krilla::page::PageSettings;
use krilla::pdf::{Pdf, PdfDocument};
use krilla::{Document, SerializeSettings};
use swirvo_core::{CancelToken, EngineError, Preset};

/// Rendering resolution for "Pages to PNG/JPG".
const RENDER_DPI: f32 = 150.0;
/// Pixel budget per rendered page.
const MAX_PAGE_PIXELS: f32 = 40_000_000.0;

fn load(path: &Path) -> Result<Arc<Pdf>, EngineError> {
    let bytes = std::fs::read(path)?;
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(bytes);
    let pdf = Pdf::new(data).map_err(|e| {
        EngineError::Decode(format!(
            "couldn't open this PDF ({e:?}); it may be damaged or password-protected"
        ))
    })?;
    Ok(Arc::new(pdf))
}

fn new_doc(pdf20: bool) -> Result<Document, EngineError> {
    let configuration = Configuration::new_with_version(if pdf20 {
        PdfVersion::Pdf20
    } else {
        PdfVersion::Pdf17
    });
    Ok(Document::new_with(SerializeSettings {
        configuration,
        ..Default::default()
    }))
}

fn is_pdf20(p: &Pdf) -> bool {
    format!("{:?}", p.version()).contains("20")
}

fn finish(doc: Document, out: &Path) -> Result<(), EngineError> {
    let bytes = doc
        .finish()
        .map_err(|e| EngineError::Encode(format!("PDF: {e:?}")))?;
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(out)?;
    f.write_all(&bytes)?;
    Ok(())
}

pub fn page_count(path: &Path) -> Result<usize, EngineError> {
    Ok(load(path)?.pages().len())
}

/// Concatenate `inputs` (in order) into `out`. Returns the page count.
pub fn merge(inputs: &[PathBuf], out: &Path, cancel: &CancelToken) -> Result<usize, EngineError> {
    let docs: Vec<Arc<Pdf>> = inputs.iter().map(|p| load(p)).collect::<Result<_, _>>()?;
    let mut doc = new_doc(docs.iter().any(|d| is_pdf20(d)))?;
    let mut pages = 0;
    for d in docs {
        cancel.check()?;
        let n = d.pages().len();
        doc.embed_pdf_pages(&PdfDocument::new(d), &(0..n).collect::<Vec<_>>());
        pages += n;
    }
    finish(doc, out)?;
    Ok(pages)
}

/// One PDF per page in `dir` (which must be new and empty): `stem p01.pdf`, …
pub fn split(
    input: &Path,
    dir: &Path,
    stem: &str,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    split_pages(input, dir, stem, None, cancel)
}

/// Like [`split`], for the 0-based `pages` only (None = all). Files keep the original page
/// numbers in their names, so "p07" is always page 7 of the source.
pub fn split_pages(
    input: &Path,
    dir: &Path,
    stem: &str,
    pages: Option<&[usize]>,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let pdf = load(input)?;
    let n = pdf.pages().len();
    let sel = selection(pages, n)?;
    let digits = n.to_string().len().max(2);
    let mut outs = Vec::with_capacity(sel.len());
    for i in sel {
        cancel.check()?;
        let mut doc = new_doc(is_pdf20(&pdf))?;
        doc.embed_pdf_pages(&PdfDocument::new(pdf.clone()), &[i]);
        let out = dir.join(format!("{stem} p{:0digits$}.pdf", i + 1));
        finish(doc, &out)?;
        outs.push(out);
    }
    Ok(outs)
}

/// A new PDF holding `pages` (0-based, in the given order). Returns the page count.
pub fn extract_pages(input: &Path, pages: &[usize], out: &Path) -> Result<usize, EngineError> {
    let pdf = load(input)?;
    let sel = selection(Some(pages), pdf.pages().len())?;
    let mut doc = new_doc(is_pdf20(&pdf))?;
    doc.embed_pdf_pages(&PdfDocument::new(pdf), &sel);
    finish(doc, out)?;
    Ok(sel.len())
}

/// Validate a page selection against `n` pages. None = every page. Empty or out-of-range
/// selections are errors, so a bad request never produces an empty file.
fn selection(pages: Option<&[usize]>, n: usize) -> Result<Vec<usize>, EngineError> {
    let sel: Vec<usize> = match pages {
        None => (0..n).collect(),
        Some(p) => p.to_vec(),
    };
    if sel.is_empty() {
        return Err(EngineError::Invalid("no pages selected".into()));
    }
    if let Some(&bad) = sel.iter().find(|&&i| i >= n) {
        return Err(EngineError::Invalid(format!(
            "page {} doesn't exist (the PDF has {n})",
            bad + 1
        )));
    }
    Ok(sel)
}

/// Rotate every page by `degrees` (multiple of 90) by editing /Rotate. Lossless.
pub fn rotate(input: &Path, out: &Path, degrees: i64) -> Result<usize, EngineError> {
    rotate_pages(input, out, degrees, None)
}

/// Rotate the 0-based `pages` (None = all) by `degrees`; other pages are untouched.
pub fn rotate_pages(
    input: &Path,
    out: &Path,
    degrees: i64,
    pages: Option<&[usize]>,
) -> Result<usize, EngineError> {
    let mut doc = crate::pdfedit::open(input, None)?;
    // get_pages() is keyed by 1-based page number.
    let all: Vec<(usize, lopdf::ObjectId)> = doc
        .get_pages()
        .into_iter()
        .map(|(n, id)| (n as usize - 1, id))
        .collect();
    let sel = selection(pages, all.len())?;
    let wanted: std::collections::HashSet<usize> = sel.into_iter().collect();
    let mut turned = 0;
    for &(i, id) in &all {
        if !wanted.contains(&i) {
            continue;
        }
        let current = inherited_rotate(&doc, id);
        if let Ok(d) = doc.get_dictionary_mut(id) {
            d.set(
                "Rotate",
                lopdf::Object::Integer((current + degrees).rem_euclid(360)),
            );
            turned += 1;
        }
    }
    // Saved like every in-place edit: a protected file stays protected.
    crate::pdfedit::save(&mut doc, out, false)?;
    Ok(turned)
}

fn inherited_rotate(doc: &lopdf::Document, mut id: lopdf::ObjectId) -> i64 {
    for _ in 0..32 {
        let Ok(d) = doc.get_dictionary(id) else {
            return 0;
        };
        if let Ok(r) = d.get(b"Rotate").and_then(lopdf::Object::as_i64) {
            return r;
        }
        match d.get(b"Parent").and_then(lopdf::Object::as_reference) {
            Ok(p) => id = p,
            Err(_) => return 0,
        }
    }
    0
}

/// Render every page to `dir` as `fmt` ("png" or "jpg"): `stem p01.png`, …
pub fn render_pages(
    input: &Path,
    dir: &Path,
    stem: &str,
    fmt: &str,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    render_selected(input, dir, stem, fmt, None, RENDER_DPI, cancel)
}

/// Render the 0-based `pages` (None = all) at `dpi` into `dir`. Names keep the source page
/// numbers (`stem p07.jpg`).
pub fn render_selected(
    input: &Path,
    dir: &Path,
    stem: &str,
    fmt: &str,
    pages: Option<&[usize]>,
    dpi: f32,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    let r = PageRenderer::open(input)?;
    let n = r.count();
    let sel = selection(pages, n)?;
    let digits = n.to_string().len().max(2);
    r.session(|s| {
        let mut outs = Vec::with_capacity(sel.len());
        for i in sel {
            cancel.check()?;
            let img = s.render_dpi(i, dpi)?;
            let out = dir.join(format!("{stem} p{:0digits$}.{fmt}", i + 1));
            render_save(img, fmt, &out)?;
            outs.push(out);
        }
        Ok(outs)
    })
}

/// Render one page straight to `out` (a single-page export).
pub fn render_one(
    input: &Path,
    page: usize,
    fmt: &str,
    dpi: f32,
    out: &Path,
) -> Result<(), EngineError> {
    let r = PageRenderer::open(input)?;
    selection(Some(&[page]), r.count())?;
    let img = r.session(|s| s.render_dpi(page, dpi))?;
    render_save(img, fmt, out)
}

fn render_save(img: image::RgbaImage, fmt: &str, out: &Path) -> Result<(), EngineError> {
    crate::image::save_with_quality(&DynamicImage::ImageRgba8(img), fmt, out, Preset::Best, 90)
}

/// A parsed PDF kept open for rendering many pages (exports, page-picker thumbnails).
/// Not `Send`: keep it on the thread that opened it. Rendering happens in a
/// [`PageRenderer::session`], which owns hayro's per-document cache (fonts, images).
pub struct PageRenderer {
    pdf: hayro::hayro_syntax::Pdf,
}

/// Renders pages of one [`PageRenderer`], sharing caches between pages.
pub struct Session<'a> {
    pages: &'a hayro::hayro_syntax::page::Pages<'a>,
    cache: hayro::RenderCache<'a>,
    interp: hayro::hayro_interpret::InterpreterSettings,
    settings: hayro::RenderSettings,
}

impl PageRenderer {
    pub fn open(input: &Path) -> Result<PageRenderer, EngineError> {
        Self::from_bytes(std::fs::read(input)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<PageRenderer, EngineError> {
        let data: hayro::hayro_syntax::PdfData = Arc::new(bytes).into();
        let pdf = hayro::hayro_syntax::Pdf::new(data).map_err(|e| {
            EngineError::Decode(format!(
                "couldn't open this PDF ({e:?}); it may be damaged or password-protected"
            ))
        })?;
        Ok(PageRenderer { pdf })
    }

    pub fn count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Page size in points (rotation applied), or None past the end.
    pub fn size(&self, i: usize) -> Option<(f32, f32)> {
        self.pdf.pages().get(i).map(|p| p.render_dimensions())
    }

    pub fn session<R>(&self, f: impl FnOnce(&mut Session<'_>) -> R) -> R {
        let mut s = Session {
            pages: self.pdf.pages(),
            cache: hayro::RenderCache::new(),
            interp: hayro::hayro_interpret::InterpreterSettings::default(),
            settings: hayro::RenderSettings::default(),
        };
        f(&mut s)
    }
}

impl Session<'_> {
    fn size(&self, i: usize) -> Result<(f32, f32), EngineError> {
        self.pages
            .get(i)
            .map(|p| p.render_dimensions())
            .ok_or_else(|| EngineError::Invalid(format!("no page {}", i + 1)))
    }

    /// Render page `i` at `dpi`, capped at 40 MP and 16 000 px per side.
    pub fn render_dpi(&mut self, i: usize, dpi: f32) -> Result<image::RgbaImage, EngineError> {
        let (w, h) = self.size(i)?;
        let mut scale = dpi.clamp(18.0, 1200.0) / 72.0;
        let px = w * h * scale * scale;
        if px > MAX_PAGE_PIXELS {
            scale *= (MAX_PAGE_PIXELS / px).sqrt();
        }
        scale = scale.min(16_000.0 / w.max(h).max(1.0));
        self.render_scale(i, scale)
    }

    /// Render page `i` to fit inside `max_w` × `max_h` pixels (thumbnails).
    pub fn render_fit(
        &mut self,
        i: usize,
        max_w: u32,
        max_h: u32,
    ) -> Result<image::RgbaImage, EngineError> {
        let (w, h) = self.size(i)?;
        let scale = (max_w as f32 / w.max(1.0)).min(max_h as f32 / h.max(1.0));
        self.render_scale(i, scale.clamp(0.01, 4.0))
    }

    fn render_scale(&mut self, i: usize, scale: f32) -> Result<image::RgbaImage, EngineError> {
        self.render_bg(i, scale, false)
    }

    /// Render page `i` at `dpi` on a transparent background (straight alpha), with the same
    /// size caps as [`Session::render_dpi`]. Used for figures placed over other content.
    pub fn render_dpi_transparent(
        &mut self,
        i: usize,
        dpi: f32,
    ) -> Result<image::RgbaImage, EngineError> {
        let (w, h) = self.size(i)?;
        let mut scale = dpi.clamp(18.0, 1200.0) / 72.0;
        let px = w * h * scale * scale;
        if px > MAX_PAGE_PIXELS {
            scale *= (MAX_PAGE_PIXELS / px).sqrt();
        }
        scale = scale.min(16_000.0 / w.max(h).max(1.0));
        self.render_bg(i, scale, true)
    }

    fn render_bg(
        &mut self,
        i: usize,
        scale: f32,
        transparent: bool,
    ) -> Result<image::RgbaImage, EngineError> {
        let page = self
            .pages
            .get(i)
            .ok_or_else(|| EngineError::Invalid(format!("no page {}", i + 1)))?;
        let pm = hayro::render(
            page,
            &self.cache,
            &self.interp,
            &self.settings,
            &hayro::PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: if transparent {
                    hayro::vello_cpu::color::palette::css::TRANSPARENT
                } else {
                    hayro::vello_cpu::color::palette::css::WHITE
                },
            },
        );
        let (pw, ph) = (u32::from(pm.width()), u32::from(pm.height()));
        let mut px = pm.data_as_u8_slice().to_vec();
        if transparent {
            // hayro's pixmaps are premultiplied; images want straight alpha.
            for p in px.as_chunks_mut::<4>().0 {
                let a = u32::from(p[3]);
                if a != 0 && a != 255 {
                    for c in &mut p[..3] {
                        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                    }
                }
            }
        }
        // With the opaque white background, premultiplied == straight alpha.
        image::RgbaImage::from_raw(pw, ph, px)
            .ok_or_else(|| EngineError::Encode("bad page bitmap".into()))
    }
}

/// One A4 page per image (portrait or landscape to match), fitted with a small margin.
/// JPEGs are embedded as-is unless their EXIF orientation needs applying.
pub fn images_to_pdf(
    inputs: &[PathBuf],
    out: &Path,
    cancel: &CancelToken,
) -> Result<usize, EngineError> {
    let mut doc = Document::new();
    for p in inputs {
        cancel.check()?;
        let bytes = std::fs::read(p)?;
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let fmt = image::guess_format(&bytes).ok();
        let upright = || -> bool {
            image::ImageReader::new(Cursor::new(&bytes))
                .with_guessed_format()
                .ok()
                .and_then(|r| r.into_decoder().ok())
                .and_then(|mut d| image::ImageDecoder::orientation(&mut d).ok())
                .is_none_or(|o| o == image::metadata::Orientation::NoTransforms)
        };
        let img = match fmt {
            Some(ImageFormat::Jpeg) if upright() => {
                krilla::image::Image::from_jpeg(bytes.into(), true)
            }
            Some(ImageFormat::Png) => krilla::image::Image::from_png(bytes.into(), true),
            Some(ImageFormat::Gif) => krilla::image::Image::from_gif(bytes.into(), true),
            _ => {
                // Anything else (or a rotated JPEG): decode with orientation applied, embed as PNG.
                let key = match fmt {
                    Some(ImageFormat::Jpeg) => "jpg",
                    Some(ImageFormat::WebP) => "webp",
                    Some(ImageFormat::Bmp) => "bmp",
                    Some(ImageFormat::Tiff) => "tiff",
                    Some(ImageFormat::Ico) => "ico",
                    Some(ImageFormat::Qoi) => "qoi",
                    Some(ImageFormat::Pnm) => "pnm",
                    _ => p.extension().and_then(|e| e.to_str()).unwrap_or("png"),
                };
                let dynimg = crate::image::load(p, &key.to_ascii_lowercase())?;
                let mut png = Vec::new();
                dynimg
                    .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
                    .map_err(EngineError::encode)?;
                krilla::image::Image::from_png(png.into(), true)
            }
        }
        .map_err(|e| EngineError::Decode(format!("{name}: {e}")))?;
        let (iw, ih) = img.size();
        let (pw, ph) = if iw > ih {
            (841.89, 595.28)
        } else {
            (595.28, 841.89)
        };
        let margin = 28.0;
        // Fit the page, but never blow a small image up beyond 2× its 96-dpi size.
        let s = ((pw - 2.0 * margin) / iw as f32)
            .min((ph - 2.0 * margin) / ih as f32)
            .min(1.5);
        let (w, h) = (iw as f32 * s, ih as f32 * s);
        let settings = PageSettings::from_wh(pw, ph)
            .ok_or_else(|| EngineError::Encode("bad page size".into()))?;
        let mut page = doc.start_page_with(settings);
        let mut surface = page.surface();
        surface.push_transform(&Transform::from_translate((pw - w) / 2.0, (ph - h) / 2.0));
        if let Some(size) = Size::from_wh(w, h) {
            surface.draw_image(img, size);
        }
        surface.pop();
        surface.finish();
        page.finish();
    }
    finish(doc, out)?;
    Ok(inputs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(path: &Path, w: u32, h: u32) {
        image::RgbImage::from_pixel(w, h, image::Rgb([200, 80, 40]))
            .save_with_format(path, ImageFormat::Png)
            .unwrap();
    }

    #[test]
    fn combine_merge_split_rotate_render() {
        let d = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let (a, b) = (d.path().join("a.png"), d.path().join("b.png"));
        png(&a, 300, 200);
        png(&b, 200, 300);
        let two = d.path().join("two.pdf");
        assert_eq!(
            images_to_pdf(&[a.clone(), b.clone()], &two, &cancel).unwrap(),
            2
        );
        assert_eq!(page_count(&two).unwrap(), 2);

        let one = d.path().join("one.pdf");
        images_to_pdf(std::slice::from_ref(&a), &one, &cancel).unwrap();
        let merged = d.path().join("merged.pdf");
        assert_eq!(
            merge(&[two.clone(), one.clone()], &merged, &cancel).unwrap(),
            3
        );
        assert_eq!(page_count(&merged).unwrap(), 3);

        let parts = d.path().join("parts");
        std::fs::create_dir(&parts).unwrap();
        let outs = split(&merged, &parts, "m", &cancel).unwrap();
        assert_eq!(outs.len(), 3);
        assert!(outs[0].ends_with("m p01.pdf"));
        assert_eq!(page_count(&outs[2]).unwrap(), 1);

        let rotated = d.path().join("rot.pdf");
        assert_eq!(rotate(&two, &rotated, 90).unwrap(), 2);
        let doc = lopdf::Document::load(&rotated).unwrap();
        for id in doc.get_pages().values() {
            assert_eq!(inherited_rotate(&doc, *id), 90);
        }

        let imgs = d.path().join("imgs");
        std::fs::create_dir(&imgs).unwrap();
        let pages = render_pages(&two, &imgs, "two", "png", &cancel).unwrap();
        assert_eq!(pages.len(), 2);
        let first = image::open(&pages[0]).unwrap().to_rgb8();
        // Landscape page; the centre shows the orange image.
        assert!(first.width() > first.height());
        let c = first.get_pixel(first.width() / 2, first.height() / 2);
        assert!(c[0] > 150 && c[1] < 120, "{c:?}");
    }

    #[test]
    fn page_subsets() {
        let d = tempfile::tempdir().unwrap();
        let cancel = CancelToken::new();
        let imgs: Vec<PathBuf> = (0..4)
            .map(|i| {
                let p = d.path().join(format!("{i}.png"));
                png(&p, 200 + i * 40, 300);
                p
            })
            .collect();
        let four = d.path().join("four.pdf");
        images_to_pdf(&imgs, &four, &cancel).unwrap();

        let sub = d.path().join("sub.pdf");
        assert_eq!(extract_pages(&four, &[3, 0], &sub).unwrap(), 2);
        assert_eq!(page_count(&sub).unwrap(), 2);
        assert!(extract_pages(&four, &[], &sub).is_err());
        assert!(extract_pages(&four, &[4], &sub).is_err());

        let rot = d.path().join("rot.pdf");
        assert_eq!(rotate_pages(&four, &rot, 90, Some(&[1, 2])).unwrap(), 2);
        let doc = lopdf::Document::load(&rot).unwrap();
        let turned: Vec<i64> = doc
            .get_pages()
            .values()
            .map(|id| inherited_rotate(&doc, *id))
            .collect();
        assert_eq!(turned, [0, 90, 90, 0]);

        let parts = d.path().join("parts");
        std::fs::create_dir(&parts).unwrap();
        let outs = split_pages(&four, &parts, "f", Some(&[2]), &cancel).unwrap();
        assert_eq!(outs.len(), 1);
        assert!(outs[0].ends_with("f p03.pdf"));

        let r = PageRenderer::open(&four).unwrap();
        assert_eq!(r.count(), 4);
        let t = r.session(|s| s.render_fit(0, 120, 160)).unwrap();
        assert!(t.width() <= 120 && t.height() <= 160 && t.width() > 60);
        let one = d.path().join("one.jpg");
        render_one(&four, 1, "jpg", 72.0, &one).unwrap();
        assert!(image::open(&one).is_ok());
    }
}
