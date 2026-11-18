//! Digital signatures in PDF files (docs/17 §2), the part that isn't cryptography: the
//! signature field, the incremental update that adds it, the byte ranges a signature
//! covers, and reading signatures back. Signing and checking the CMS (PKCS#7) value is
//! the operating system's job (`os::certsign`, through the Windows certificate store);
//! this module never handles keys.
//!
//! Signing appends an incremental update to the file, so everything before it, earlier
//! signatures included, stays byte for byte the same. The update holds the signature
//! dictionary with a `/Contents` placeholder of zeros and a `/ByteRange` covering the
//! whole file except that placeholder; the signer hashes those ranges and the detached
//! signature is written into the placeholder as hex.

use std::fmt::Write as _;

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use swirvo_core::EngineError;

use super::forms::{escape, win_ansi};
use super::geometry::{Box4, Geom, box_object};
use super::stamp::text_width;

/// Room for the signature: a CMS with a certificate chain of three or four certificates
/// and a signing time fits easily.
pub const RESERVED: usize = 16_384;

/// Where and how a visible signature box is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Visible {
    /// 0-based page.
    pub page: usize,
    /// Display coordinates (points, origin bottom-left of the page as shown).
    pub rect: Box4,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    /// The signer's name as shown (from the certificate).
    pub name: String,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// Signing time, UTC: year, month, day, hour, minute, second.
    pub time: [u32; 6],
    pub visible: Option<Visible>,
}

/// A file with its signature field and an empty placeholder, ready to be signed.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub bytes: Vec<u8>,
    /// `[offset, length, offset, length]` of the signed parts.
    pub range: [usize; 4],
}

impl Prepared {
    /// What the signature covers: the whole file except the placeholder.
    pub fn signed_data(&self) -> Vec<u8> {
        let [a, b, c, d] = self.range;
        let mut v = Vec::with_capacity(b + d);
        v.extend_from_slice(&self.bytes[a..a + b]);
        v.extend_from_slice(&self.bytes[c..c + d]);
        v
    }

    /// Put the DER-encoded detached signature into the placeholder.
    pub fn finish(mut self, der: &[u8]) -> Result<Vec<u8>, EngineError> {
        if der.len() > RESERVED {
            return Err(EngineError::Encode(format!(
                "the signature is {} bytes, more than the {RESERVED} reserved for it",
                der.len()
            )));
        }
        // The placeholder is between '<' at range[1]-… and '>' at range[2]-1.
        let start = self.range[1] + 1;
        let mut hex = String::with_capacity(der.len() * 2);
        for b in der {
            let _ = write!(hex, "{b:02X}");
        }
        self.bytes[start..start + hex.len()].copy_from_slice(hex.as_bytes());
        Ok(self.bytes)
    }
}

/// The reasons a file can't be signed, or Ok.
pub fn can_sign(doc: &Document) -> Result<(), EngineError> {
    if doc.trailer.get(b"Encrypt").is_ok() || doc.encryption_state.is_some() {
        return Err(EngineError::Invalid(
            "this PDF is protected; signing it would need its passwords and permissions \
             carried into the signature, which isn't supported yet"
                .into(),
        ));
    }
    // A certification signature that allows no changes (DocMDP P = 1).
    let cat = doc
        .catalog()
        .map_err(|e| EngineError::Decode(e.to_string()))?;
    let perms = cat.get(b"Perms").ok().and_then(|p| dict(doc, p));
    if let Some(sig) = perms
        .and_then(|p| p.get(b"DocMDP").ok())
        .and_then(|s| dict(doc, s))
    {
        let p = sig
            .get(b"Reference")
            .ok()
            .and_then(|r| deref(doc, r).as_array().ok())
            .and_then(|a| a.first())
            .and_then(|r| dict(doc, r))
            .and_then(|r| r.get(b"TransformParams").ok())
            .and_then(|t| dict(doc, t))
            .and_then(|t| t.get(b"P").ok())
            .and_then(|p| p.as_i64().ok())
            .unwrap_or(2);
        if p == 1 {
            return Err(EngineError::Invalid(
                "this PDF is certified to allow no changes, so it can't be signed again".into(),
            ));
        }
    }
    Ok(())
}

fn deref<'a>(doc: &'a Document, o: &'a Object) -> &'a Object {
    super::deref(doc, o)
}

fn dict<'a>(doc: &'a Document, o: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, o).as_dict().ok()
}

fn name(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

fn text(s: &str) -> Object {
    lopdf::text_string(s)
}

/// `D:YYYYMMDDHHmmSS+00'00'`.
pub fn pdf_date(t: [u32; 6]) -> String {
    format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}+00'00'",
        t[0], t[1], t[2], t[3], t[4], t[5]
    )
}

/// The current UTC time as `[y, m, d, h, min, s]`.
pub fn now() -> [u32; 6] {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (y, m, d) = crate::archive::civil_from_days((secs / 86_400) as i64);
    let r = secs % 86_400;
    [
        y.clamp(0, 9999) as u32,
        m,
        d,
        (r / 3600) as u32,
        (r / 60 % 60) as u32,
        (r % 60) as u32,
    ]
}

// ---------------------------------------------------------------- object writing

fn real(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.into()
    }
}

/// Serialize one object the way a PDF writer would.
fn ser(o: &Object, out: &mut Vec<u8>) {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => out.extend_from_slice(real(*r).as_bytes()),
        Object::Name(n) => {
            out.push(b'/');
            for &b in n {
                if (0x21..=0x7E).contains(&b) && !b"()<>[]{}/%#".contains(&b) {
                    out.push(b);
                } else {
                    out.extend_from_slice(format!("#{b:02X}").as_bytes());
                }
            }
        }
        Object::String(s, StringFormat::Hexadecimal) => {
            out.push(b'<');
            for b in s {
                out.extend_from_slice(format!("{b:02X}").as_bytes());
            }
            out.push(b'>');
        }
        Object::String(s, StringFormat::Literal) => {
            out.push(b'(');
            for &b in s {
                match b {
                    b'(' | b')' | b'\\' => {
                        out.push(b'\\');
                        out.push(b);
                    }
                    b'\r' => out.extend_from_slice(b"\\r"),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    0x20..=0x7E => out.push(b),
                    _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
                }
            }
            out.push(b')');
        }
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                ser(x, out);
            }
            out.push(b']');
        }
        Object::Dictionary(d) => ser_dict(d, out),
        Object::Stream(s) => {
            let mut d = s.dict.clone();
            d.set("Length", Object::Integer(s.content.len() as i64));
            ser_dict(&d, out);
            out.extend_from_slice(b"\nstream\r\n");
            out.extend_from_slice(&s.content);
            out.extend_from_slice(b"\r\nendstream");
        }
        Object::Reference((id, g)) => out.extend_from_slice(format!("{id} {g} R").as_bytes()),
    }
}

fn ser_dict(d: &Dictionary, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        ser(&Object::Name(k.clone()), out);
        out.push(b' ');
        ser(v, out);
    }
    out.extend_from_slice(b">>");
}

/// The offset in the last `startxref`, and whether it points to a cross-reference stream.
fn last_xref(bytes: &[u8]) -> Result<(usize, bool), EngineError> {
    let tail = &bytes[bytes.len().saturating_sub(2048)..];
    let at = tail
        .windows(9)
        .rposition(|w| w == b"startxref")
        .ok_or_else(|| EngineError::Decode("no cross-reference offset at the end".into()))?;
    let num: String = tail[at + 9..]
        .iter()
        .map(|&b| b as char)
        .skip_while(|c| c.is_ascii_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();
    let off: usize = num
        .parse()
        .map_err(|_| EngineError::Decode("bad cross-reference offset".into()))?;
    let stream = !bytes
        .get(off..)
        .is_some_and(|r| r.trim_ascii_start().starts_with(b"xref"));
    Ok((off, stream))
}

/// A visible signature's appearance: a thin frame and up to four lines of text.
fn appearance(w: f32, h: f32, lines: &[String]) -> Vec<u8> {
    let mut c = format!(
        "q 0.18 0.33 0.6 RG 0.8 w 0.4 0.4 {} {} re S ",
        real(w - 0.8),
        real(h - 0.8)
    )
    .into_bytes();
    let n = lines.len().max(1) as f32;
    let widest = lines
        .iter()
        .map(|l| text_width(l, 1.0, false))
        .fold(0.0, f32::max);
    let size = ((h - 6.0) / (n * 1.25))
        .min(if widest > 0.0 {
            (w - 8.0) / widest
        } else {
            12.0
        })
        .clamp(4.0, 12.0);
    c.extend(b"BT 0 g ");
    for (i, l) in lines.iter().enumerate() {
        let Some(bytes) = win_ansi(l) else { continue };
        let y = h - 3.0 - size * (1.0 + 1.25 * i as f32) + size * 0.2;
        c.extend(format!("/Helv {} Tf 1 0 0 1 4 {} Tm ", real(size), real(y)).into_bytes());
        c.extend(escape(&bytes));
        c.extend(b" Tj ");
    }
    c.extend(b"ET Q");
    c
}

/// Prepare `input` for signing: an incremental update with a signature field (visible
/// or not) and the placeholder. Nothing before the update changes.
pub fn prepare(input: &[u8], setup: &Setup) -> Result<Prepared, EngineError> {
    let doc = Document::load_mem(input).map_err(|e| match e {
        lopdf::Error::Decryption(_) | lopdf::Error::InvalidPassword => {
            EngineError::Invalid("this PDF is password-protected; unlock it first".into())
        }
        e => EngineError::Decode(format!("couldn't open this PDF ({e})")),
    })?;
    can_sign(&doc)?;
    let (prev_xref, xref_stream) = last_xref(input)?;
    let pages = super::pages(&doc);
    let root_id = doc
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| EngineError::Decode("no document catalog".into()))?;
    let size = doc
        .trailer
        .get(b"Size")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(doc.max_id as i64 + 1) as u32;
    let mut next = size;
    let mut fresh = || {
        next += 1;
        (next - 1, 0)
    };
    let sig_id = fresh();
    let field_id = fresh();
    let mut updates: Vec<(ObjectId, Object)> = Vec::new();

    // A unique field name.
    let existing = super::forms::list(&doc);
    let field_name = (1..)
        .map(|i| format!("Signature{i}"))
        .find(|n| !existing.iter().any(|f| f.name == *n))
        .unwrap_or_else(|| "Signature".into());

    // The widget: on the visible page, or on the first page with no area.
    let (page_idx, rect_user, ap) = match &setup.visible {
        Some(v) => {
            let &pid = pages.get(v.page).ok_or_else(|| {
                EngineError::Invalid(format!("page {} doesn't exist", v.page + 1))
            })?;
            let g = Geom::of(&doc, pid);
            let r = [
                v.rect[0].min(v.rect[2]),
                v.rect[1].min(v.rect[3]),
                v.rect[0].max(v.rect[2]),
                v.rect[1].max(v.rect[3]),
            ];
            let (w, h) = (r[2] - r[0], r[3] - r[1]);
            let mut lines = vec![format!("Digitally signed by {}", setup.name)];
            let t = setup.time;
            lines.push(format!(
                "Date: {:04}-{:02}-{:02} {:02}:{:02} UTC",
                t[0], t[1], t[2], t[3], t[4]
            ));
            if let Some(reason) = &setup.reason {
                lines.push(format!("Reason: {reason}"));
            }
            if let Some(loc) = &setup.location {
                lines.push(format!("Location: {loc}"));
            }
            let m = g.to_user();
            let mut fd = Dictionary::new();
            fd.set("Type", name("XObject"));
            fd.set("Subtype", name("Form"));
            fd.set("BBox", box_object([0.0, 0.0, w, h]));
            if g.rotate != 0 {
                fd.set(
                    "Matrix",
                    Object::Array(
                        [m[0], m[1], m[2], m[3], 0.0, 0.0]
                            .iter()
                            .map(|&v| Object::Real(v))
                            .collect(),
                    ),
                );
            }
            let mut font = Dictionary::new();
            font.set("Type", name("Font"));
            font.set("Subtype", name("Type1"));
            font.set("BaseFont", name("Helvetica"));
            font.set("Encoding", name("WinAnsiEncoding"));
            let mut fonts = Dictionary::new();
            fonts.set("Helv", Object::Dictionary(font));
            let mut res = Dictionary::new();
            res.set("Font", Object::Dictionary(fonts));
            fd.set("Resources", Object::Dictionary(res));
            let ap_id = fresh();
            updates.push((
                ap_id,
                Object::Stream(lopdf::Stream::new(fd, appearance(w, h, &lines))),
            ));
            (v.page, g.rect_to_user(r), Some(ap_id))
        }
        None => (0, [0.0; 4], None),
    };
    let &page_id = pages
        .get(page_idx)
        .ok_or_else(|| EngineError::Invalid("this PDF has no pages".into()))?;

    // The signature dictionary, with fixed-width placeholders patched after writing.
    let mut sig = Dictionary::new();
    sig.set("Type", name("Sig"));
    sig.set("Filter", name("Adobe.PPKLite"));
    sig.set("SubFilter", name("adbe.pkcs7.detached"));
    sig.set("Name", text(&setup.name));
    sig.set("M", Object::string_literal(pdf_date(setup.time)));
    if let Some(r) = &setup.reason {
        sig.set("Reason", text(r));
    }
    if let Some(l) = &setup.location {
        sig.set("Location", text(l));
    }
    // ByteRange and Contents are written by hand below, last, so their offsets are known.

    let mut field = Dictionary::new();
    field.set("Type", name("Annot"));
    field.set("Subtype", name("Widget"));
    field.set("FT", name("Sig"));
    field.set("T", text(&field_name));
    field.set("V", Object::Reference(sig_id));
    field.set("F", Object::Integer(4 | 128)); // Print, Locked
    field.set("P", Object::Reference(page_id));
    field.set("Rect", box_object(rect_user));
    if let Some(ap) = ap {
        let mut apd = Dictionary::new();
        apd.set("N", Object::Reference(ap));
        field.set("AP", Object::Dictionary(apd));
    }
    updates.push((field_id, Object::Dictionary(field)));

    // The page lists the widget among its annotations.
    let page = doc
        .get_dictionary(page_id)
        .map_err(|e| EngineError::Decode(e.to_string()))?;
    match page.get(b"Annots") {
        Ok(Object::Reference(arr)) => {
            let mut a = doc
                .get_object(*arr)
                .and_then(Object::as_array)
                .cloned()
                .unwrap_or_default();
            a.push(Object::Reference(field_id));
            updates.push((*arr, Object::Array(a)));
        }
        other => {
            let mut a = match other {
                Ok(Object::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            a.push(Object::Reference(field_id));
            let mut p = page.clone();
            p.set("Annots", Object::Array(a));
            updates.push((page_id, Object::Dictionary(p)));
        }
    }

    // The form lists the field; SigFlags 3: has signatures, append only.
    let catalog = doc
        .get_dictionary(root_id)
        .map_err(|e| EngineError::Decode(e.to_string()))?
        .clone();
    let add_field = |mut form: Dictionary, updates: &mut Vec<(ObjectId, Object)>| {
        match form.get(b"Fields") {
            Ok(Object::Reference(fid)) => {
                let mut a = doc
                    .get_object(*fid)
                    .and_then(Object::as_array)
                    .cloned()
                    .unwrap_or_default();
                a.push(Object::Reference(field_id));
                updates.push((*fid, Object::Array(a)));
            }
            other => {
                let mut a = match other {
                    Ok(Object::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                a.push(Object::Reference(field_id));
                form.set("Fields", Object::Array(a));
            }
        }
        form.set("SigFlags", Object::Integer(3));
        form
    };
    match catalog.get(b"AcroForm") {
        Ok(Object::Reference(fid)) => {
            let form = doc
                .get_dictionary(*fid)
                .cloned()
                .unwrap_or_else(|_| Dictionary::new());
            let form = add_field(form, &mut updates);
            updates.push((*fid, Object::Dictionary(form)));
        }
        other => {
            let form = match other {
                Ok(Object::Dictionary(d)) => d.clone(),
                _ => Dictionary::new(),
            };
            let form = add_field(form, &mut updates);
            let mut cat = catalog.clone();
            cat.set("AcroForm", Object::Dictionary(form));
            updates.push((root_id, Object::Dictionary(cat)));
        }
    }

    // Write the update.
    let mut out = input.to_vec();
    if out.last() != Some(&b'\n') {
        out.push(b'\n');
    }
    let mut offsets: Vec<(u32, usize)> = Vec::new();
    for (id, obj) in &updates {
        offsets.push((id.0, out.len()));
        out.extend_from_slice(format!("{} {} obj\n", id.0, id.1).as_bytes());
        ser(obj, &mut out);
        out.extend_from_slice(b"\nendobj\n");
    }
    // The signature dictionary last but one, with its placeholders.
    offsets.push((sig_id.0, out.len()));
    out.extend_from_slice(format!("{} 0 obj\n", sig_id.0).as_bytes());
    let mut head = Vec::new();
    ser_dict(&sig, &mut head);
    head.truncate(head.len() - 2); // reopen the dictionary: drop ">>"
    out.extend_from_slice(&head);
    out.extend_from_slice(b"/ByteRange ");
    let br_at = out.len();
    out.extend_from_slice(b"[0 0000000000 0000000000 0000000000]");
    let br_len = out.len() - br_at;
    out.extend_from_slice(b"/Contents ");
    let contents_at = out.len();
    out.push(b'<');
    out.extend(std::iter::repeat_n(b'0', RESERVED * 2));
    out.push(b'>');
    let contents_end = out.len();
    out.extend_from_slice(b">>\nendobj\n");

    // Cross-reference section of the same kind as the file's.
    offsets.sort_unstable();
    let new_size = next;
    let mut trailer = Dictionary::new();
    trailer.set("Size", Object::Integer(new_size as i64));
    trailer.set("Root", Object::Reference(root_id));
    for k in [&b"Info"[..], b"ID"] {
        if let Ok(v) = doc.trailer.get(k) {
            trailer.set(k, v.clone());
        }
    }
    trailer.set("Prev", Object::Integer(prev_xref as i64));
    let xref_at = out.len();
    if xref_stream {
        // An uncompressed cross-reference stream: type, 4-byte offset, 2-byte generation.
        let xs_id = new_size;
        let mut entries: Vec<(u32, usize)> = offsets.clone();
        entries.push((xs_id, xref_at));
        entries.sort_unstable();
        let mut data = Vec::new();
        let mut index = Vec::new();
        for &(id, off) in &entries {
            index.push(Object::Integer(id as i64));
            index.push(Object::Integer(1));
            data.push(1u8);
            data.extend_from_slice(&(off as u32).to_be_bytes());
            data.extend_from_slice(&[0, 0]);
        }
        trailer.set("Size", Object::Integer(xs_id as i64 + 1));
        trailer.set("Type", name("XRef"));
        trailer.set(
            "W",
            Object::Array(vec![
                Object::Integer(1),
                Object::Integer(4),
                Object::Integer(2),
            ]),
        );
        trailer.set("Index", Object::Array(index));
        out.extend_from_slice(format!("{xs_id} 0 obj\n").as_bytes());
        ser(&Object::Stream(lopdf::Stream::new(trailer, data)), &mut out);
        out.extend_from_slice(b"\nendobj\n");
    } else {
        out.extend_from_slice(b"xref\n");
        // Subsections of consecutive ids.
        let mut i = 0;
        while i < offsets.len() {
            let mut j = i + 1;
            while j < offsets.len() && offsets[j].0 == offsets[j - 1].0 + 1 {
                j += 1;
            }
            out.extend_from_slice(format!("{} {}\n", offsets[i].0, j - i).as_bytes());
            for &(_, off) in &offsets[i..j] {
                out.extend_from_slice(format!("{off:010} 00000 n\r\n").as_bytes());
            }
            i = j;
        }
        out.extend_from_slice(b"trailer\n");
        ser_dict(&trailer, &mut out);
        out.push(b'\n');
    }
    out.extend_from_slice(format!("startxref\n{xref_at}\n%%EOF\n").as_bytes());

    // Patch the byte range now that the length is known.
    let range = [0, contents_at, contents_end, out.len() - contents_end];
    let text = format!("[0 {} {} {}]", range[1], range[2], range[3]);
    if text.len() > br_len {
        return Err(EngineError::Encode("the file is too large to sign".into()));
    }
    let padded = format!("{text:<br_len$}");
    out[br_at..br_at + br_len].copy_from_slice(padded.as_bytes());
    Ok(Prepared { bytes: out, range })
}

/// A signature found in a file, before its cryptography is checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub field: String,
    /// The name the signer gave (`/Name`), the claimed time (`/M`), reason, location.
    pub name: Option<String>,
    pub time: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub sub_filter: String,
    /// The byte range is well formed: it starts at 0 and skips exactly `/Contents`.
    pub range_ok: bool,
    /// It covers the whole file: nothing was added after this signature.
    pub whole_file: bool,
    /// The signed bytes and the DER signature (without the zero padding).
    pub data: Vec<u8>,
    pub der: Vec<u8>,
}

/// Length of the DER element at the start of `b` (header and content), when well formed.
pub fn der_len(b: &[u8]) -> Option<usize> {
    let l0 = *b.get(1)? as usize;
    let (len, head) = if l0 < 0x80 {
        (l0, 2)
    } else {
        let n = l0 & 0x7F;
        if n == 0 || n > 4 {
            return None;
        }
        let mut v = 0usize;
        for i in 0..n {
            v = (v << 8) | *b.get(2 + i)? as usize;
        }
        (v, 2 + n)
    };
    let total = head.checked_add(len)?;
    (total <= b.len()).then_some(total)
}

fn string(d: &Dictionary, k: &[u8]) -> Option<String> {
    d.get(k)
        .ok()
        .and_then(|o| lopdf::decode_text_string(o).ok())
        .filter(|s| !s.trim().is_empty())
}

/// Every signature field with a value, in the order of the form.
pub fn signatures(bytes: &[u8]) -> Result<Vec<Found>, EngineError> {
    let doc = Document::load_mem(bytes)
        .map_err(|e| EngineError::Decode(format!("couldn't open ({e})")))?;
    let mut out = Vec::new();
    for (id, field) in super::forms::signature_fields(&doc) {
        let Some(v) = doc
            .get_dictionary(id)
            .ok()
            .and_then(|d| d.get(b"V").ok())
            .and_then(|v| dict(&doc, v))
            .cloned()
        else {
            continue;
        };
        let range: Vec<usize> = v
            .get(b"ByteRange")
            .ok()
            .and_then(|r| deref(&doc, r).as_array().ok())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_i64().ok())
                    .map(|x| x.max(0) as usize)
                    .collect()
            })
            .unwrap_or_default();
        let contents = match v.get(b"Contents") {
            Ok(Object::String(s, _)) => s.clone(),
            _ => Vec::new(),
        };
        let der = der_len(&contents).map_or_else(Vec::new, |n| contents[..n].to_vec());
        let (range_ok, whole_file, data) = match range.as_slice() {
            &[a, b, c, d]
                if a == 0
                    && c > b
                    && c.checked_add(d).is_some_and(|e| e <= bytes.len())
                    && bytes.get(b) == Some(&b'<')
                    && bytes.get(c - 1) == Some(&b'>') =>
            {
                let mut data = bytes[..b].to_vec();
                data.extend_from_slice(&bytes[c..c + d]);
                (
                    true,
                    c + d == bytes.len() || only_padding(&bytes[c + d..]),
                    data,
                )
            }
            _ => (false, false, Vec::new()),
        };
        out.push(Found {
            field,
            name: string(&v, b"Name"),
            time: string(&v, b"M"),
            reason: string(&v, b"Reason"),
            location: string(&v, b"Location"),
            sub_filter: v
                .get(b"SubFilter")
                .ok()
                .and_then(|s| s.as_name().ok())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .unwrap_or_default(),
            range_ok,
            whole_file,
            data,
            der,
        });
    }
    Ok(out)
}

/// Some writers leave line ends after `%%EOF`.
fn only_padding(b: &[u8]) -> bool {
    b.iter().all(|c| c.is_ascii_whitespace() || *c == 0)
}

/// "2026-11-10 14:05 UTC" from a PDF date.
pub fn readable_date(d: &str) -> String {
    let digits: String = d
        .trim_start_matches("D:")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.len() < 12 {
        return d.to_string();
    }
    let rest = &d.trim_start_matches("D:")[digits.len()..];
    let zone = match rest.chars().next() {
        Some('Z') | None => " UTC".to_string(),
        Some(c @ ('+' | '-')) => {
            let z: String = rest[1..].chars().filter(char::is_ascii_digit).collect();
            if z.trim_start_matches('0').is_empty() {
                " UTC".into()
            } else if z.len() >= 4 {
                format!(" UTC{c}{}:{}", &z[..2], &z[2..4])
            } else {
                format!(" UTC{c}{z}")
            }
        }
        _ => String::new(),
    };
    format!(
        "{}-{}-{} {}:{}{zone}",
        &digits[..4],
        &digits[4..6],
        &digits[6..8],
        &digits[8..10],
        &digits[10..12]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(visible: bool) -> Setup {
        Setup {
            name: "Ada Example".into(),
            reason: Some("Approved".into()),
            location: Some("Pune".into()),
            time: [2026, 11, 10, 9, 30, 0],
            visible: visible.then_some(Visible {
                page: 1,
                rect: [300.0, 40.0, 540.0, 110.0],
            }),
        }
    }

    /// A fake "signature": any DER element works for the structure.
    fn fake_der(n: usize) -> Vec<u8> {
        let mut v = vec![0x30, 0x82, (n >> 8) as u8, n as u8];
        v.extend(std::iter::repeat_n(0xAB, n));
        v
    }

    fn check(input: &[u8], visible: bool) {
        let p = prepare(input, &setup(visible)).unwrap();
        // Everything before the update is untouched.
        assert_eq!(&p.bytes[..input.len()], input);
        let [a, b, c, d] = p.range;
        assert_eq!(a, 0);
        assert_eq!(c + d, p.bytes.len(), "the range reaches the end");
        assert_eq!(c - b, RESERVED * 2 + 2, "it skips exactly the placeholder");
        assert_eq!(p.bytes[b], b'<');
        assert_eq!(p.bytes[c - 1], b'>');
        let data = p.signed_data();
        let der = fake_der(300);
        let signed = p.finish(&der).unwrap();
        // The update parses: one signature field, intact, covering the file.
        let doc = Document::load_mem(&signed).unwrap();
        assert_eq!(super::super::pages(&doc).len(), 2);
        let found = signatures(&signed).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let f = &found[0];
        assert!(f.range_ok && f.whole_file);
        assert_eq!(f.der, der, "the padding is trimmed");
        assert_eq!(f.data, data, "the same bytes are signed and checked");
        assert_eq!(f.name.as_deref(), Some("Ada Example"));
        assert_eq!(f.reason.as_deref(), Some("Approved"));
        assert_eq!(f.sub_filter, "adbe.pkcs7.detached");
        assert_eq!(
            readable_date(f.time.as_deref().unwrap()),
            "2026-11-10 09:30 UTC"
        );
        // Appending anything afterwards shows.
        let mut later = signed.clone();
        later.extend_from_slice(b"\n% changed\n");
        let f2 = &signatures(&later).unwrap()[0];
        assert!(f2.range_ok && !f2.whole_file);
        // A second signature keeps the first intact (it no longer covers the whole file).
        let p2 = prepare(&signed, &setup(false)).unwrap();
        let twice = p2.finish(&fake_der(200)).unwrap();
        let both = signatures(&twice).unwrap();
        assert_eq!(both.len(), 2);
        assert!(!both[0].whole_file && both[1].whole_file);
        assert_eq!(both[0].data, data);
        assert_ne!(both[0].field, both[1].field);
    }

    #[test]
    fn prepares_signs_and_reads_back_with_a_classic_xref() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&p, 2, false);
        let bytes = std::fs::read(&p).unwrap();
        // Rewritten by lopdf with a classic table, so both kinds are covered.
        let mut doc = Document::load_mem(&bytes).unwrap();
        let mut classic = Vec::new();
        doc.save_to(&mut classic).unwrap();
        assert!(!last_xref(&classic).unwrap().1);
        check(&classic, true);
    }

    #[test]
    fn prepares_signs_and_reads_back_with_an_xref_stream() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&p, 2, false);
        let mut doc = Document::load(&p).unwrap();
        let mut modern = Vec::new();
        doc.save_modern(&mut modern).unwrap();
        assert!(last_xref(&modern).unwrap().1);
        check(&modern, false);
    }

    #[test]
    fn the_visible_box_is_drawn_where_asked() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&p, 2, false);
        let prepared = prepare(&std::fs::read(&p).unwrap(), &setup(true)).unwrap();
        let signed = prepared.finish(&fake_der(100)).unwrap();
        let out = d.path().join("s.pdf");
        std::fs::write(&out, &signed).unwrap();
        let r = crate::pdf_tools::PageRenderer::open(&out).unwrap();
        r.session(|s| {
            let img = s.render_fit(1, 595, 842).unwrap();
            // The frame's left edge at x = 300 pt, inside the box's height.
            let k = img.width() as f32 / 595.0;
            let y = ((842.0 - 75.0) * k) as u32;
            let dark = (295..306).any(|x| {
                let px = img.get_pixel((x as f32 * k) as u32, y);
                px[2] > 80 && px[0] < 120
            });
            assert!(dark, "a frame where the box is");
        });
        // The words are in the widget's appearance.
        let doc = Document::load_mem(&signed).unwrap();
        let all: Vec<u8> = doc
            .objects
            .values()
            .filter_map(|o| o.as_stream().ok())
            .flat_map(|s| s.content.clone())
            .collect();
        let text = String::from_utf8_lossy(&all);
        assert!(text.contains("Digitally signed by Ada Example"), "{text}");
        assert!(text.contains("Reason: Approved"));
    }

    #[test]
    fn refuses_protected_and_locked_files() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.pdf");
        crate::pdfedit::testing::sample(&p, 1, false);
        let locked = d.path().join("l.pdf");
        crate::pdfedit::secure::protect(
            &p,
            &locked,
            "",
            Some("owner"),
            crate::pdfedit::secure::Allow {
                print: true,
                copy: false,
                edit: false,
            },
        )
        .unwrap();
        let e = prepare(&std::fs::read(&locked).unwrap(), &setup(false)).unwrap_err();
        assert!(e.to_string().contains("protected"), "{e}");
    }

    #[test]
    fn der_lengths_and_dates() {
        assert_eq!(der_len(&[0x30, 0x03, 1, 2, 3, 0, 0]), Some(5));
        assert_eq!(
            der_len(&[0x30, 0x82, 0x01, 0x00]),
            None,
            "longer than the data"
        );
        let mut long = vec![0x30, 0x82, 0x01, 0x00];
        long.extend(vec![0; 0x100 + 10]);
        assert_eq!(der_len(&long), Some(0x104));
        assert_eq!(readable_date("D:20261110093000Z"), "2026-11-10 09:30 UTC");
        assert_eq!(
            readable_date("D:20261110093000+05'30'"),
            "2026-11-10 09:30 UTC+05:30"
        );
        assert_eq!(pdf_date([2026, 1, 2, 3, 4, 5]), "D:20260102030405+00'00'");
    }
}
