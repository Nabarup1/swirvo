//! Figures and backgrounds as pictures (docs/13 §4.4). A figure is rendered with its labels;
//! a background is rendered from a copy of the page whose text-showing operators were
//! removed, so the text in front of it stays live in the DOCX.

use super::extract;
use super::model::{BBox, Media};
use crate::pdf_tools::PageRenderer;
use std::collections::BTreeSet;

const DPI: f32 = 200.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Req {
    pub page: usize,
    pub bbox: BBox,
    pub textless: bool,
}

/// One rendered picture per request, `None` where rendering wasn't possible.
pub fn render(bytes: &[u8], reqs: &[Req]) -> Vec<Option<Media>> {
    let mut out: Vec<Option<Media>> = vec![None; reqs.len()];
    for textless in [false, true] {
        let mut idx: Vec<usize> = (0..reqs.len())
            .filter(|&i| reqs[i].textless == textless)
            .collect();
        if idx.is_empty() {
            continue;
        }
        idx.sort_by_key(|&i| reqs[i].page);
        let src = if textless {
            let pages: BTreeSet<usize> = idx.iter().map(|&i| reqs[i].page).collect();
            // Without a text-free copy, leave backgrounds out rather than doubling the text.
            let Some(b) = strip_text(bytes, &pages) else {
                continue;
            };
            b
        } else {
            bytes.to_vec()
        };
        let Ok(r) = PageRenderer::from_bytes(src) else {
            continue;
        };
        let sizes: Vec<Option<(f32, f32)>> = (0..r.count()).map(|i| r.size(i)).collect();
        r.session(|s| {
            let mut cur: Option<(usize, image::RgbaImage)> = None;
            for &i in &idx {
                let q = &reqs[i];
                if cur.as_ref().map(|c| c.0) != Some(q.page) {
                    cur = s
                        .render_dpi_transparent(q.page, DPI)
                        .ok()
                        .map(|img| (q.page, img));
                }
                let (Some((_, img)), Some(Some((pw, _)))) = (&cur, sizes.get(q.page)) else {
                    continue;
                };
                let k = img.width() as f32 / pw.max(1.0);
                let x = (q.bbox.x0 * k).floor().clamp(0.0, img.width() as f32 - 1.0) as u32;
                let y = (q.bbox.y0 * k)
                    .floor()
                    .clamp(0.0, img.height() as f32 - 1.0) as u32;
                let w = ((q.bbox.x1 * k).ceil() as u32)
                    .min(img.width())
                    .saturating_sub(x)
                    .max(1);
                let h = ((q.bbox.y1 * k).ceil() as u32)
                    .min(img.height())
                    .saturating_sub(y)
                    .max(1);
                let crop = image::imageops::crop_imm(img, x, y, w, h).to_image();
                out[i] = extract::encode(crop);
            }
        });
    }
    out
}

/// A transparent pixel, for a picture that couldn't be rendered.
pub fn blank() -> Media {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    extract::encode(img).unwrap_or(Media {
        bytes: Vec::new(),
        ext: "png",
    })
}

fn strip_ops(mut c: lopdf::content::Content) -> lopdf::content::Content {
    c.operations
        .retain(|op| !matches!(op.operator.as_str(), "Tj" | "TJ" | "'" | "\""));
    c
}

/// A copy of the PDF with the text-showing operators removed from `pages` and every form.
/// Graphics state set inside text objects is kept, so later drawing is unchanged.
pub fn strip_text(bytes: &[u8], pages: &BTreeSet<usize>) -> Option<Vec<u8>> {
    use lopdf::{Document, Object, content::Content};
    let mut doc = Document::load_mem(bytes).ok()?;
    if doc.is_encrypted() {
        return None;
    }
    let ids: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
    for (num, id) in ids {
        if !pages.contains(&(num as usize).saturating_sub(1)) {
            continue;
        }
        let data = doc.get_page_content(id).ok()?;
        let c = Content::decode(&data).ok()?;
        doc.change_page_content(id, strip_ops(c).encode().ok()?)
            .ok()?;
    }
    for obj in doc.objects.values_mut() {
        if let Object::Stream(s) = obj
            && s.dict
                .get(b"Subtype")
                .and_then(Object::as_name)
                .is_ok_and(|n| n == b"Form")
        {
            let _ = s.decompress();
            if let Ok(c) = Content::decode(&s.content)
                && let Ok(b) = strip_ops(c).encode()
            {
                s.set_plain_content(b);
            }
        }
    }
    let mut out = Vec::new();
    doc.save_to(&mut out).ok()?;
    Some(out)
}
