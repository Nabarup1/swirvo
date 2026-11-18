//! PDF editing tools (docs/17): pages, stamps, cropping and sizing, compression, security
//! and clean-up. Every function reads one file and writes a new one; originals are never
//! opened for writing (PROJECT_GUIDE.md rule 1).
//!
//! Two techniques, chosen per tool:
//! - **In place** (lopdf): the page objects are edited and everything else is kept as it was:
//!   links, form fields, bookmarks, metadata. Stamps, crop, resize, compress, security.
//! - **Re-embedded** (krilla): pages are drawn into a new document. Used where pages are
//!   rearranged onto new sheets (N-up, booklet) or assembled from several files.

pub mod annotate;
pub mod assemble;
pub mod clean;
pub mod compare;
pub mod compress;
pub mod forms;
pub mod geometry;
pub mod gray;
pub mod organise;
pub mod redact;
pub mod searchable;
pub mod secure;
pub mod sign;
pub mod stamp;
mod visual;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};
use swirvo_core::EngineError;

/// Open a PDF for editing. A password-protected file needs `password`; an empty password
/// (owner-only protection, the common "can't print/copy" kind) is tried automatically.
pub fn open(path: &Path, password: Option<&str>) -> Result<Document, EngineError> {
    let bytes = std::fs::read(path)?;
    open_bytes(&bytes, password)
}

pub fn open_bytes(bytes: &[u8], password: Option<&str>) -> Result<Document, EngineError> {
    let doc = match password {
        Some(pw) => Document::load_mem_with_options(bytes, lopdf::LoadOptions::with_password(pw)),
        None => Document::load_mem(bytes),
    }
    .map_err(|e| match e {
        lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => {
            EngineError::Invalid(if password.is_some() {
                "that password doesn't open this PDF".into()
            } else {
                "this PDF is password-protected; unlock it first".into()
            })
        }
        e => EngineError::Decode(format!("couldn't open this PDF ({e})")),
    })?;
    if doc.is_encrypted() {
        return Err(EngineError::Invalid(
            "this PDF is password-protected; unlock it first".into(),
        ));
    }
    Ok(doc)
}

/// Write `doc` to `out`. `compact` packs objects into compressed object streams (PDF 1.5+,
/// read by every viewer since 2004), which is most of what "compress" gains on text PDFs.
///
/// A document that was protected when opened is protected again with the same passwords
/// and restrictions (lopdf decrypts on load but doesn't re-encrypt on save).
pub fn save(doc: &mut Document, out: &Path, compact: bool) -> Result<(), EngineError> {
    if let Some(state) = doc.encryption_state.clone()
        && !doc.is_encrypted()
    {
        doc.trailer.remove(b"Encrypt");
        doc.encrypt(&state).map_err(EngineError::encode)?;
    }
    let mut w = BufWriter::new(File::create(out)?);
    if compact {
        if doc.version.as_str() < "1.5" {
            doc.version = "1.5".into();
        }
        let opts = lopdf::SaveOptions::builder()
            .use_object_streams(true)
            .use_xref_streams(true)
            .compression_level(9)
            .build();
        doc.save_with_options(&mut w, opts)
            .map_err(EngineError::encode)?;
    } else {
        doc.save_to(&mut w).map_err(EngineError::encode)?;
    }
    w.flush()?;
    Ok(())
}

/// Pages in order (0-based index → object id).
pub fn pages(doc: &Document) -> Vec<ObjectId> {
    doc.get_pages().into_values().collect()
}

/// Validate a 0-based selection against `n` pages; None = all.
pub fn selection(pages: Option<&[usize]>, n: usize) -> Result<Vec<usize>, EngineError> {
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

/// A page attribute, following `/Parent` for the inheritable ones (Resources, MediaBox,
/// CropBox, Rotate).
pub(crate) fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page;
    for _ in 0..64 {
        let d = doc.get_dictionary(id).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(v);
        }
        id = d.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// Resolve a possibly indirect object.
pub(crate) fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    match o {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(o),
        _ => o,
    }
}

/// Give `page` its own Resources dictionary (copied from what it inherits or points to), so
/// adding a stamp to one page never changes another page sharing the same resources.
pub(crate) fn own_resources(doc: &mut Document, page: ObjectId) -> Result<(), EngineError> {
    let res = match inherited(doc, page, b"Resources") {
        Some(o) => match deref(doc, o) {
            Object::Dictionary(d) => d.clone(),
            _ => Dictionary::new(),
        },
        None => Dictionary::new(),
    };
    doc.get_dictionary_mut(page)
        .map_err(EngineError::encode)?
        .set("Resources", Object::Dictionary(res));
    Ok(())
}

/// The page's own (direct) Resources dictionary; call [`own_resources`] first.
pub(crate) fn resources_mut(
    doc: &mut Document,
    page: ObjectId,
) -> Result<&mut Dictionary, EngineError> {
    doc.get_dictionary_mut(page)
        .and_then(|d| d.get_mut(b"Resources"))
        .and_then(Object::as_dict_mut)
        .map_err(|_| EngineError::Encode("page resources".into()))
}

/// A sub-dictionary of `d` (XObject, ExtGState, Font…), created when missing. An indirect
/// one is copied in, so the edit stays local to this page.
pub(crate) fn sub_dict<'a>(
    doc_objects: &std::collections::BTreeMap<ObjectId, Object>,
    d: &'a mut Dictionary,
    key: &[u8],
) -> &'a mut Dictionary {
    let current = match d.get(key) {
        Ok(Object::Dictionary(x)) => Some(x.clone()),
        Ok(Object::Reference(id)) => match doc_objects.get(id) {
            Some(Object::Dictionary(x)) => Some(x.clone()),
            _ => None,
        },
        _ => None,
    };
    d.set(key, Object::Dictionary(current.unwrap_or_default()));
    match d.get_mut(key) {
        Ok(Object::Dictionary(x)) => x,
        _ => unreachable!("just set to a dictionary"),
    }
}

/// Wrap a page's existing content in `q … Q` and append `after`, so our additions start
/// from a clean graphics state whatever the original content left behind.
pub(crate) fn wrap_and_append(
    doc: &mut Document,
    page: ObjectId,
    after: Vec<u8>,
) -> Result<(), EngineError> {
    let mut tail = b"\nQ\n".to_vec();
    tail.extend(after);
    wrap_with(doc, page, b"q\n".to_vec(), tail)
}

/// Put `before` ahead of the page's content and `after` behind it, as separate streams
/// (the original streams are shared, not copied).
pub(crate) fn wrap_with(
    doc: &mut Document,
    page: ObjectId,
    before: Vec<u8>,
    after: Vec<u8>,
) -> Result<(), EngineError> {
    let before = doc.add_object(lopdf::Stream::new(Dictionary::new(), before));
    let after = doc.add_object(lopdf::Stream::new(Dictionary::new(), after));
    let existing: Vec<Object> = match doc
        .get_dictionary(page)
        .ok()
        .and_then(|d| d.get(b"Contents").ok())
    {
        Some(Object::Reference(id)) => match doc.get_object(*id) {
            Ok(Object::Array(a)) => a.clone(),
            _ => vec![Object::Reference(*id)],
        },
        Some(Object::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let mut all = vec![Object::Reference(before)];
    all.extend(existing);
    all.push(Object::Reference(after));
    doc.get_dictionary_mut(page)
        .map_err(EngineError::encode)?
        .set("Contents", Object::Array(all));
    Ok(())
}

/// A name for a new resource that no page resource already uses.
pub(crate) fn fresh_name(d: &Dictionary, prefix: &str) -> Vec<u8> {
    (0..)
        .map(|i| format!("{prefix}{i}").into_bytes())
        .find(|n| !d.has(n))
        .unwrap_or_else(|| prefix.as_bytes().to_vec())
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::Path;

    /// A PDF of `n` pages (A4 portrait, or landscape with `wide`), each showing its number,
    /// with a link annotation on page 1, written by krilla like our other outputs.
    pub fn sample(path: &Path, n: usize, wide: bool) {
        use krilla::geom::{PathBuilder, Rect};
        use krilla::page::PageSettings;
        let mut doc = krilla::Document::new();
        let (w, h) = if wide { (842.0, 595.0) } else { (595.0, 842.0) };
        for i in 0..n {
            let mut page = doc.start_page_with(PageSettings::from_wh(w, h).unwrap());
            if i == 0 {
                page.add_annotation(
                    krilla::annotation::LinkAnnotation::new(
                        Rect::from_xywh(50.0, 50.0, 100.0, 20.0).unwrap(),
                        krilla::annotation::Target::Action(
                            krilla::action::LinkAction::new("https://example.com/".into()).into(),
                        ),
                    )
                    .into(),
                );
            }
            let mut s = page.surface();
            // A dark block whose size encodes the page number (rendering tests find it).
            let mut pb = PathBuilder::new();
            pb.push_rect(Rect::from_xywh(100.0, 100.0, 40.0 + i as f32 * 10.0, 40.0).unwrap());
            s.set_fill(Some(krilla::paint::Fill::default()));
            s.draw_path(&pb.finish().unwrap());
            s.finish();
            page.finish();
        }
        std::fs::write(path, doc.finish().unwrap()).unwrap();
    }
}
