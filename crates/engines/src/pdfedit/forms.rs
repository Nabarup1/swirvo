//! PDF forms: list the fields, fill them (from a spreadsheet: one PDF per row), and
//! flatten fields and comments into the page so they print and can't be changed.
//!
//! Filled text fields get a fresh appearance drawn with the standard Helvetica font when
//! the value fits its Latin character set; otherwise the old appearance is removed and
//! viewers are asked to draw it (NeedAppearances). XFA data (Adobe LiveCycle forms) is
//! dropped on fill, so every viewer shows the values that were filled in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream, decode_text_string, text_string};
use swirvo_core::{CancelToken, EngineError};

use super::geometry::{Box4, cm, read_box};
use super::{deref, fresh_name, own_resources, resources_mut, sub_dict, wrap_and_append};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Check,
    Radio,
    Choice,
    Button,
    Signature,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldInfo {
    /// Full name (`parent.child`), what a spreadsheet column is matched against.
    pub name: String,
    pub kind: Kind,
    pub value: String,
    /// Choice options, or the "on" names of a check box / radio group.
    pub options: Vec<String>,
}

const MAX_DEPTH: usize = 32;

fn acroform(doc: &Document) -> Option<&Dictionary> {
    let cat = doc.catalog().ok()?;
    deref(doc, cat.get(b"AcroForm").ok()?).as_dict().ok()
}

/// A field attribute, inherited from parent fields when the field doesn't set it.
fn inh<'a>(doc: &'a Document, id: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut cur = id;
    for _ in 0..MAX_DEPTH {
        let d = doc.get_dictionary(cur).ok()?;
        if let Ok(v) = d.get(key) {
            return Some(deref(doc, v));
        }
        cur = d.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

fn flags(doc: &Document, id: ObjectId) -> i64 {
    inh(doc, id, b"Ff")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0)
}

fn kind(doc: &Document, id: ObjectId) -> Option<Kind> {
    let ft = inh(doc, id, b"FT")?.as_name().ok()?;
    let ff = flags(doc, id);
    Some(match ft {
        b"Tx" => Kind::Text,
        b"Ch" => Kind::Choice,
        b"Sig" => Kind::Signature,
        b"Btn" if ff & (1 << 16) != 0 => Kind::Button,
        b"Btn" if ff & (1 << 15) != 0 => Kind::Radio,
        b"Btn" => Kind::Check,
        _ => return None,
    })
}

/// Terminal fields with their full names.
fn terminal_fields(doc: &Document) -> Vec<(ObjectId, String)> {
    let mut out = Vec::new();
    let Some(Object::Array(roots)) = acroform(doc)
        .and_then(|a| a.get(b"Fields").ok())
        .map(|f| deref(doc, f))
    else {
        return out;
    };
    let mut stack: Vec<(ObjectId, String, usize)> = roots
        .iter()
        .rev()
        .filter_map(|o| o.as_reference().ok())
        .map(|id| (id, String::new(), 0))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    while let Some((id, prefix, depth)) = stack.pop() {
        if depth > MAX_DEPTH || !seen.insert(id) {
            continue;
        }
        let Ok(d) = doc.get_dictionary(id) else {
            continue;
        };
        let part = d
            .get(b"T")
            .ok()
            .and_then(|t| decode_text_string(deref(doc, t)).ok());
        let name = match (&part, prefix.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{prefix}.{p}"),
            (None, _) => prefix.clone(),
        };
        let kids: Vec<ObjectId> = match d.get(b"Kids").map(|k| deref(doc, k)) {
            Ok(Object::Array(k)) => k.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => Vec::new(),
        };
        let field_kids: Vec<ObjectId> = kids
            .iter()
            .copied()
            .filter(|k| doc.get_dictionary(*k).is_ok_and(|kd| kd.has(b"T")))
            .collect();
        if field_kids.is_empty() {
            if !name.is_empty() {
                out.push((id, name));
            }
        } else {
            for k in field_kids.into_iter().rev() {
                stack.push((k, name.clone(), depth + 1));
            }
        }
    }
    out
}

/// The widget annotations of a terminal field (its kids, or the field itself).
fn widgets(doc: &Document, id: ObjectId) -> Vec<ObjectId> {
    match doc
        .get_dictionary(id)
        .ok()
        .and_then(|d| d.get(b"Kids").ok())
        .map(|k| deref(doc, k))
    {
        Some(Object::Array(k)) => k.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => vec![id],
    }
}

/// The "on" appearance names of a check box or radio widget.
fn on_states(doc: &Document, widget: ObjectId) -> Vec<String> {
    let Some(n) = doc
        .get_dictionary(widget)
        .ok()
        .and_then(|d| d.get(b"AP").ok())
        .and_then(|ap| deref(doc, ap).as_dict().ok())
        .and_then(|ap| ap.get(b"N").ok())
        .and_then(|n| deref(doc, n).as_dict().ok())
    else {
        return Vec::new();
    };
    n.iter()
        .map(|(k, _)| String::from_utf8_lossy(k).into_owned())
        .filter(|k| k != "Off")
        .collect()
}

fn value_text(doc: &Document, o: Option<&Object>) -> String {
    match o.map(|o| deref(doc, o)) {
        Some(Object::Name(n)) => String::from_utf8_lossy(n).into_owned(),
        Some(s @ Object::String(..)) => decode_text_string(s).unwrap_or_default(),
        Some(Object::Array(a)) => a
            .iter()
            .filter_map(|x| decode_text_string(deref(doc, x)).ok())
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// Every fillable field.
/// Signature fields: the field object and its full name.
pub(crate) fn signature_fields(doc: &Document) -> Vec<(ObjectId, String)> {
    terminal_fields(doc)
        .into_iter()
        .filter(|(id, _)| kind(doc, *id) == Some(Kind::Signature))
        .collect()
}

pub fn list(doc: &Document) -> Vec<FieldInfo> {
    terminal_fields(doc)
        .into_iter()
        .filter_map(|(id, name)| {
            let kind = kind(doc, id)?;
            let options = match kind {
                Kind::Check | Kind::Radio => {
                    let mut o: Vec<String> = widgets(doc, id)
                        .iter()
                        .flat_map(|w| on_states(doc, *w))
                        .collect();
                    o.dedup();
                    o
                }
                Kind::Choice => match inh(doc, id, b"Opt") {
                    Some(Object::Array(a)) => a
                        .iter()
                        .filter_map(|x| match deref(doc, x) {
                            Object::Array(pair) => pair
                                .get(1)
                                .and_then(|s| decode_text_string(deref(doc, s)).ok()),
                            s => decode_text_string(s).ok(),
                        })
                        .collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            };
            Some(FieldInfo {
                name,
                kind,
                value: value_text(doc, inh(doc, id, b"V")),
                options,
            })
        })
        .collect()
}

/// The fields as a CSV template: a header of field names and a row of current values.
pub fn export_csv(input: &Path, out: &Path) -> Result<usize, EngineError> {
    let doc = super::open(input, None)?;
    let fields: Vec<FieldInfo> = list(&doc)
        .into_iter()
        .filter(|f| !matches!(f.kind, Kind::Button | Kind::Signature))
        .collect();
    if fields.is_empty() {
        return Err(EngineError::Invalid("it has no form fields to fill".into()));
    }
    let mut w = csv::Writer::from_path(out).map_err(EngineError::encode)?;
    w.write_record(fields.iter().map(|f| f.name.as_str()))
        .map_err(EngineError::encode)?;
    w.write_record(fields.iter().map(|f| f.value.as_str()))
        .map_err(EngineError::encode)?;
    w.flush()?;
    Ok(fields.len())
}

fn truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "y" | "yes" | "true" | "x" | "on" | "checked" | "✓" | "✔"
    )
}

/// Fill `values` (field name → value; names match case-insensitively, by full name or the
/// last part) into the open document. Returns how many fields were set.
pub fn fill(doc: &mut Document, values: &BTreeMap<String, String>) -> Result<usize, EngineError> {
    let fields = terminal_fields(doc);
    if fields.is_empty() {
        return Err(EngineError::Invalid("it has no form fields to fill".into()));
    }
    let lookup = |name: &str| -> Option<&String> {
        let short = name.rsplit('.').next().unwrap_or(name);
        values
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .or_else(|| values.iter().find(|(k, _)| k.eq_ignore_ascii_case(short)))
            .map(|(_, v)| v)
    };
    let mut set = 0;
    let mut need_viewer = false;
    for (id, name) in fields {
        let Some(v) = lookup(&name) else { continue };
        let Some(k) = kind(doc, id) else { continue };
        if flags(doc, id) & 1 != 0 {
            continue; // read-only
        }
        match k {
            Kind::Text | Kind::Choice => {
                set_dict(doc, id, b"V", text_string(v));
                for w in widgets(doc, id) {
                    match text_appearance(doc, id, w, v) {
                        Some(ap) => {
                            let ap = doc.add_object(ap);
                            let mut d = Dictionary::new();
                            d.set("N", Object::Reference(ap));
                            set_dict(doc, w, b"AP", Object::Dictionary(d));
                        }
                        None => {
                            remove_key(doc, w, b"AP");
                            need_viewer = true;
                        }
                    }
                }
            }
            Kind::Check | Kind::Radio => {
                let ws = widgets(doc, id);
                // A check box takes yes/no; a radio group takes the option's name (or yes
                // for a single option).
                let pick: Option<String> = ws
                    .iter()
                    .flat_map(|w| on_states(doc, *w))
                    .find(|s| s.eq_ignore_ascii_case(v.trim()))
                    .or_else(|| {
                        truthy(v)
                            .then(|| ws.iter().flat_map(|w| on_states(doc, *w)).next())
                            .flatten()
                    });
                let state = pick.clone().unwrap_or_else(|| "Off".into());
                set_dict(doc, id, b"V", Object::Name(state.clone().into_bytes()));
                for w in ws {
                    let on = on_states(doc, w).contains(&state);
                    let s = if on { state.clone() } else { "Off".into() };
                    set_dict(doc, w, b"AS", Object::Name(s.into_bytes()));
                }
            }
            Kind::Button | Kind::Signature => continue,
        }
        set += 1;
    }
    if let Some(af) = acroform_id_or_direct(doc) {
        let d = match af {
            Some(id) => doc.get_dictionary_mut(id).ok(),
            None => doc
                .catalog_mut()
                .ok()
                .and_then(|c| c.get_mut(b"AcroForm").ok())
                .and_then(|a| a.as_dict_mut().ok()),
        };
        if let Some(d) = d {
            d.remove(b"XFA");
            if need_viewer {
                d.set("NeedAppearances", Object::Boolean(true));
            }
        }
    }
    Ok(set)
}

/// Where the AcroForm dictionary lives: Some(Some(id)) indirect, Some(None) inline in the
/// catalog.
fn acroform_id_or_direct(doc: &Document) -> Option<Option<ObjectId>> {
    match doc.catalog().ok()?.get(b"AcroForm").ok()? {
        Object::Reference(id) => Some(Some(*id)),
        Object::Dictionary(_) => Some(None),
        _ => None,
    }
}

fn set_dict(doc: &mut Document, id: ObjectId, key: &[u8], v: Object) {
    if let Ok(d) = doc.get_dictionary_mut(id) {
        d.set(key.to_vec(), v);
    }
}

fn remove_key(doc: &mut Document, id: ObjectId, key: &[u8]) {
    if let Ok(d) = doc.get_dictionary_mut(id) {
        d.remove(key);
    }
}

/// WinAnsi bytes for `s`, if every character has one.
pub(crate) fn win_ansi(s: &str) -> Option<Vec<u8>> {
    s.chars()
        .map(|c| match c as u32 {
            0x20..=0x7E | 0xA0..=0xFF => Some(c as u32 as u8),
            _ => (0x80u8..0xA0).find(|&b| crate::docxlayout::rtf::cp1252(b) == c),
        })
        .collect()
}

pub(crate) fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'(');
    for &b in bytes {
        if matches!(b, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// Font size and colour operator from a default-appearance string (`/Helv 0 Tf 0 g`).
fn parse_da(da: &str) -> (f32, String) {
    let t: Vec<&str> = da.split_whitespace().collect();
    let mut size = 0.0;
    let mut color = "0 g".to_string();
    for (i, w) in t.iter().enumerate() {
        match *w {
            "Tf" if i >= 1 => size = t[i - 1].parse().unwrap_or(0.0),
            "g" | "rg" | "k" => {
                let n = match *w {
                    "g" => 1,
                    "rg" => 3,
                    _ => 4,
                };
                if i >= n {
                    color = format!("{} {w}", t[i - n..i].join(" "));
                }
            }
            _ => {}
        }
    }
    (size, color)
}

/// A normal appearance for a text or choice widget showing `value`, or None when the value
/// needs characters outside Helvetica's set.
fn text_appearance(
    doc: &Document,
    field: ObjectId,
    widget: ObjectId,
    value: &str,
) -> Option<Stream> {
    let wd = doc.get_dictionary(widget).ok()?;
    let r = read_box(doc, wd.get(b"Rect").ok()?)?;
    let (w, h) = ((r[2] - r[0]).abs(), (r[3] - r[1]).abs());
    let da = inh(doc, widget, b"DA")
        .or_else(|| inh(doc, field, b"DA"))
        .or_else(|| acroform(doc).and_then(|a| a.get(b"DA").ok()))
        .and_then(|o| decode_text_string(o).ok())
        .unwrap_or_default();
    let (mut size, color) = parse_da(&da);
    let q = inh(doc, field, b"Q")
        .and_then(|o| o.as_i64().ok())
        .unwrap_or(0);
    let multiline = flags(doc, field) & (1 << 12) != 0;
    let lines: Vec<&str> = if multiline {
        value.lines().collect()
    } else {
        vec![value]
    };
    let encoded: Vec<Vec<u8>> = lines.iter().map(|l| win_ansi(l)).collect::<Option<_>>()?;
    let widest = lines
        .iter()
        .map(|l| super::stamp::text_width(l, 1.0, false))
        .fold(0.0, f32::max);
    if size <= 0.0 {
        let by_h = if multiline {
            12.0
        } else {
            ((h - 4.0) * 0.75).clamp(4.0, 12.0)
        };
        let by_w = if widest > 0.0 {
            (w - 4.0) / widest
        } else {
            by_h
        };
        size = by_h.min(by_w).max(4.0);
    }
    let mut c = format!(
        "/Tx BMC q 1 1 {} {} re W n BT /Helv {size} Tf {color} ",
        w - 2.0,
        h - 2.0
    )
    .into_bytes();
    let lead = size * 1.15;
    let first = if multiline {
        h - 2.0 - size
    } else {
        (h - size) / 2.0 + 0.22 * size
    };
    for (k, (l, e)) in lines.iter().zip(&encoded).enumerate() {
        let tw = super::stamp::text_width(l, size, false);
        let x = match q {
            1 => (w - tw) / 2.0,
            2 => w - 2.0 - tw,
            _ => 2.0,
        };
        let y = first - k as f32 * lead;
        c.extend(format!("1 0 0 1 {x:.2} {y:.2} Tm ").into_bytes());
        c.extend(escape(e));
        c.extend(b" Tj ");
    }
    c.extend(b"ET Q EMC");
    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    font.set("Encoding", Object::Name(b"WinAnsiEncoding".to_vec()));
    let mut fonts = Dictionary::new();
    fonts.set("Helv", Object::Dictionary(font));
    let mut res = Dictionary::new();
    res.set("Font", Object::Dictionary(fonts));
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Form".to_vec()));
    d.set(
        "BBox",
        Object::Array(vec![0.into(), 0.into(), Object::Real(w), Object::Real(h)]),
    );
    d.set("Resources", Object::Dictionary(res));
    Some(Stream::new(d, c))
}

/// The appearance stream an annotation shows now (its normal appearance, in its current
/// state).
fn appearance(doc: &Document, annot: &Dictionary) -> Option<ObjectId> {
    let ap = deref(doc, annot.get(b"AP").ok()?).as_dict().ok()?;
    match ap.get(b"N").ok()? {
        Object::Reference(id) => match doc.get_object(*id).ok()? {
            Object::Stream(_) => Some(*id),
            Object::Dictionary(states) => {
                let s = annot.get(b"AS").ok()?.as_name().ok()?;
                states.get(s).ok()?.as_reference().ok()
            }
            _ => None,
        },
        Object::Dictionary(states) => {
            let s = annot.get(b"AS").ok()?.as_name().ok()?;
            states.get(s).ok()?.as_reference().ok()
        }
        _ => None,
    }
}

/// The matrix that draws a form XObject into `rect` (PDF 32000-1 §12.5.5).
fn fit_matrix(doc: &Document, form: ObjectId, rect: Box4) -> Option<[f32; 6]> {
    let st = doc.get_object(form).ok()?.as_stream().ok()?;
    let bb = read_box(doc, st.dict.get(b"BBox").ok()?)?;
    let m: [f32; 6] = match st.dict.get(b"Matrix") {
        Ok(Object::Array(a)) if a.len() == 6 => {
            let v: Vec<f32> = a.iter().filter_map(|x| x.as_float().ok()).collect();
            v.try_into().ok()?
        }
        _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
    };
    let p = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let pts = [
        p(bb[0], bb[1]),
        p(bb[2], bb[1]),
        p(bb[0], bb[3]),
        p(bb[2], bb[3]),
    ];
    let tx0 = pts.iter().map(|q| q.0).fold(f32::INFINITY, f32::min);
    let ty0 = pts.iter().map(|q| q.1).fold(f32::INFINITY, f32::min);
    let tx1 = pts.iter().map(|q| q.0).fold(f32::NEG_INFINITY, f32::max);
    let ty1 = pts.iter().map(|q| q.1).fold(f32::NEG_INFINITY, f32::max);
    let (tw, th) = (tx1 - tx0, ty1 - ty0);
    if tw <= 0.0 || th <= 0.0 {
        return None;
    }
    let (rx0, ry0) = (rect[0].min(rect[2]), rect[1].min(rect[3]));
    let sx = (rect[2] - rect[0]).abs() / tw;
    let sy = (rect[3] - rect[1]).abs() / th;
    Some([sx, 0.0, 0.0, sy, rx0 - tx0 * sx, ry0 - ty0 * sy])
}

/// Draw form fields (and with `comments`, comments and markup too) into the pages and
/// remove them as interactive objects. Returns how many were flattened.
pub fn flatten(doc: &mut Document, comments: bool) -> Result<usize, EngineError> {
    let mut count = 0;
    let mut widgets_left = false;
    for page in super::pages(doc) {
        let annots: Vec<Object> =
            match super::inherited(doc, page, b"Annots").map(|o| deref(doc, o)) {
                Some(Object::Array(a)) => a.clone(),
                _ => continue,
            };
        let mut keep = Vec::new();
        let mut draws: Vec<(ObjectId, [f32; 6])> = Vec::new();
        let mut dropped_parents = std::collections::BTreeSet::new();
        for a in &annots {
            let Some(d) = (match a {
                Object::Reference(id) => doc.get_dictionary(*id).ok().cloned(),
                Object::Dictionary(d) => Some(d.clone()),
                _ => None,
            }) else {
                continue;
            };
            let sub = d
                .get(b"Subtype")
                .ok()
                .and_then(|s| s.as_name().ok())
                .unwrap_or(b"");
            let take = match sub {
                b"Widget" => true,
                b"Link" | b"Popup" => false,
                _ => comments,
            };
            if !take {
                if sub == b"Widget" {
                    widgets_left = true;
                }
                keep.push(a.clone());
                continue;
            }
            if let Object::Reference(id) = a {
                dropped_parents.insert(*id);
            }
            let f = d.get(b"F").and_then(Object::as_i64).unwrap_or(0);
            let hidden = f & (2 | 32) != 0;
            if !hidden
                && let Some(form) = appearance(doc, &d)
                && let Some(rect) = d.get(b"Rect").ok().and_then(|r| read_box(doc, r))
                && let Some(m) = fit_matrix(doc, form, rect)
            {
                draws.push((form, m));
            }
            count += 1;
        }
        // Popups whose parent comment was flattened go too.
        keep.retain(|a| {
            let Object::Reference(id) = a else {
                return true;
            };
            let parent = doc
                .get_dictionary(*id)
                .ok()
                .and_then(|d| d.get(b"Parent").ok())
                .and_then(|p| p.as_reference().ok());
            !parent.is_some_and(|p| dropped_parents.contains(&p))
        });
        if keep.len() == annots.len() {
            continue;
        }
        if !draws.is_empty() {
            own_resources(doc, page)?;
            let objects = doc.objects.clone();
            let res = resources_mut(doc, page)?;
            let xo = sub_dict(&objects, res, b"XObject");
            let mut content = Vec::new();
            for (form, m) in draws {
                let name = fresh_name(xo, "Fl");
                xo.set(name.clone(), Object::Reference(form));
                content.extend(format!("q {} /", cm(m)).into_bytes());
                content.extend(&name);
                content.extend(b" Do Q\n");
            }
            wrap_and_append(doc, page, content)?;
        }
        doc.get_dictionary_mut(page)
            .map_err(EngineError::encode)?
            .set("Annots", Object::Array(keep));
    }
    if !widgets_left && count > 0 {
        doc.catalog_mut()
            .map_err(EngineError::encode)?
            .remove(b"AcroForm");
    }
    Ok(count)
}

/// Flatten a file.
pub fn flatten_file(input: &Path, out: &Path, comments: bool) -> Result<usize, EngineError> {
    let mut doc = super::open(input, None)?;
    let n = flatten(&mut doc, comments)?;
    if n == 0 {
        return Err(EngineError::Invalid(if comments {
            "it has no form fields or comments".into()
        } else {
            "it has no form fields".into()
        }));
    }
    doc.prune_objects();
    super::save(&mut doc, out, false)?;
    Ok(n)
}

/// One filled PDF per row into `dir`, named from `name_field` (or numbered).
pub fn fill_rows(
    input: &Path,
    rows: &[BTreeMap<String, String>],
    dir: &Path,
    stem: &str,
    name_field: Option<&str>,
    flat: bool,
    cancel: &CancelToken,
) -> Result<Vec<PathBuf>, EngineError> {
    if rows.is_empty() {
        return Err(EngineError::Invalid("the data file has no rows".into()));
    }
    let bytes = std::fs::read(input)?;
    let digits = rows.len().to_string().len().max(3);
    let mut used = std::collections::HashSet::new();
    let mut outs = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        cancel.check()?;
        let mut doc = super::open_bytes(&bytes, None)?;
        let n = fill(&mut doc, row)?;
        if n == 0 {
            return Err(EngineError::Invalid(
                "no column in the data file matches a field name (export the fields to see them)"
                    .into(),
            ));
        }
        if flat {
            flatten(&mut doc, false)?;
            doc.prune_objects();
        }
        let label = name_field
            .and_then(|f| {
                row.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(f))
                    .map(|(_, v)| v)
            })
            .map(|v| crate::docxtools::safe_label(v))
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| format!("{:0digits$}", i + 1));
        let mut name = format!("{stem} {label}");
        let mut k = 2;
        while !used.insert(name.to_lowercase()) {
            name = format!("{stem} {label} ({k})");
            k += 1;
        }
        let out = dir.join(format!("{name}.pdf"));
        super::save(&mut doc, &out, false)?;
        outs.push(out);
    }
    Ok(outs)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn dict(pairs: Vec<(&str, Object)>) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in pairs {
            d.set(k, v);
        }
        d
    }

    /// Add a text field and a check box to page 1 of `path`.
    pub fn add_form(path: &Path) {
        let mut doc = super::super::open(path, None).unwrap();
        let page = super::super::pages(&doc)[0];
        let on = doc.add_object(Stream::new(
            {
                let mut d = Dictionary::new();
                d.set("Subtype", Object::Name(b"Form".to_vec()));
                d.set(
                    "BBox",
                    Object::Array(vec![0.into(), 0.into(), 20.into(), 20.into()]),
                );
                d
            },
            b"0 g 2 2 16 16 re f".to_vec(),
        ));
        let off = doc.add_object(Stream::new(
            {
                let mut d = Dictionary::new();
                d.set("Subtype", Object::Name(b"Form".to_vec()));
                d.set(
                    "BBox",
                    Object::Array(vec![0.into(), 0.into(), 20.into(), 20.into()]),
                );
                d
            },
            Vec::new(),
        ));
        let name = doc.add_object(dict(vec![
            ("Type", Object::from("Annot")),
            ("Subtype", Object::from("Widget")),
            ("FT", Object::from("Tx")),
            ("T", text_string("name")),
            (
                "Rect",
                Object::from(vec![100.into(), 600.into(), 400.into(), 624.into()]),
            ),
            ("DA", text_string("/Helv 0 Tf 0 g")),
            ("P", Object::Reference(page)),
        ]));
        let mut nd = Dictionary::new();
        nd.set("Yes", Object::Reference(on));
        nd.set("Off", Object::Reference(off));
        let mut ap = Dictionary::new();
        ap.set("N", Object::Dictionary(nd));
        let check = doc.add_object(dict(vec![
            ("Type", Object::from("Annot")),
            ("Subtype", Object::from("Widget")),
            ("FT", Object::from("Btn")),
            ("T", text_string("agree")),
            (
                "Rect",
                Object::from(vec![100.into(), 560.into(), 120.into(), 580.into()]),
            ),
            ("AP", Object::Dictionary(ap)),
            ("AS", Object::from("Off")),
            ("V", Object::from("Off")),
            ("P", Object::Reference(page)),
        ]));
        doc.get_dictionary_mut(page).unwrap().set(
            "Annots",
            Object::Array(vec![Object::Reference(name), Object::Reference(check)]),
        );
        let form = doc.add_object(dict(vec![(
            "Fields",
            Object::from(vec![Object::Reference(name), Object::Reference(check)]),
        )]));
        doc.catalog_mut()
            .unwrap()
            .set("AcroForm", Object::Reference(form));
        doc.save(path).unwrap();
    }

    #[test]
    fn lists_fills_and_flattens() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("form.pdf");
        super::super::testing::sample(&src, 1, false);
        add_form(&src);
        let doc = super::super::open(&src, None).unwrap();
        let f = list(&doc);
        assert_eq!(f.len(), 2);
        assert_eq!((f[0].name.as_str(), f[0].kind), ("name", Kind::Text));
        assert_eq!(
            (f[1].kind, f[1].options.clone()),
            (Kind::Check, vec!["Yes".to_string()])
        );

        let csv = d.path().join("fields.csv");
        assert_eq!(export_csv(&src, &csv).unwrap(), 2);
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .starts_with("name,agree")
        );

        let rows = vec![
            BTreeMap::from([
                ("Name".to_string(), "Ada Lovelace".to_string()),
                ("agree".to_string(), "yes".to_string()),
            ]),
            BTreeMap::from([
                ("Name".to_string(), "Zoë Brontë".to_string()),
                ("agree".to_string(), "no".to_string()),
            ]),
        ];
        let outs = fill_rows(
            &src,
            &rows,
            d.path(),
            "form",
            Some("name"),
            false,
            &CancelToken::new(),
        )
        .unwrap();
        assert_eq!(outs.len(), 2);
        assert!(outs[0].ends_with("form Ada Lovelace.pdf"), "{outs:?}");
        let doc = super::super::open(&outs[0], None).unwrap();
        let f = list(&doc);
        assert_eq!(f[0].value, "Ada Lovelace");
        assert_eq!(f[1].value, "Yes");
        let doc2 = super::super::open(&outs[1], None).unwrap();
        assert_eq!(list(&doc2)[1].value, "Off");

        // Flattened: no fields left, the value is drawn on the page as real text.
        let flat = d.path().join("flat.pdf");
        assert_eq!(flatten_file(&outs[0], &flat, false).unwrap(), 2);
        let doc = super::super::open(&flat, None).unwrap();
        assert!(list(&doc).is_empty());
        let g = crate::pdflayout::debug_page(&std::fs::read(&flat).unwrap(), 0).unwrap();
        let text: String = g.lines().filter_map(|l| l.split('\'').nth(1)).collect();
        assert!(text.contains("Ada Lovelace"), "{g}");
        let img = crate::pdf_tools::PageRenderer::open(&flat)
            .unwrap()
            .session(|s| s.render_dpi(0, 72.0))
            .unwrap();
        // The ticked box is drawn (page 842 pt tall, box 560..580 from the bottom).
        assert!(img.get_pixel(110, 842 - 570)[0] < 50);
    }
}
