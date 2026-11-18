//! Fonts: find a document's families on this system, measure them the way Word does, and
//! shape text with them.
//!
//! - **Discovery.** The system font list is read once per process (fontdb). Under WSL the
//!   Windows fonts are included too, so a Linux build lays out like the Windows one.
//! - **Substitution.** Missing families fall back to metric-compatible ones (Carlito for
//!   Calibri, Liberation for Arial/Times/Courier…), then by generic class.
//! - **Metrics.** Single line height is GDI's: `winAscent + winDescent` plus the external
//!   leading (`max(0, hhea.lineGap − (win − hhea))`).
//! - **Embedding.** A face whose licence forbids embedding (`fsType` restricted) is
//!   skipped, so the PDF never carries a font it may not.
//! - **Shaping.** rustybuzz with Word's defaults: no ligatures, kerning only when the run
//!   asks for it; complex scripts keep their required features.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use rustybuzz::ttf_parser;

use super::model::Generic;

struct System {
    db: fontdb::Database,
    by_family: HashMap<String, Vec<fontdb::ID>>,
}

fn system() -> &'static System {
    static SYS: OnceLock<System> = OnceLock::new();
    SYS.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        #[cfg(target_os = "linux")]
        {
            let win = std::path::Path::new("/mnt/c/Windows/Fonts");
            if win.is_dir() {
                db.load_fonts_dir(win);
            }
        }
        if let Some(extra) = std::env::var_os("SWIRVO_FONT_DIR") {
            db.load_fonts_dir(extra);
        }
        let mut by_family: HashMap<String, Vec<fontdb::ID>> = HashMap::new();
        for f in db.faces() {
            for (name, _) in &f.families {
                by_family.entry(name.to_lowercase()).or_default().push(f.id);
            }
        }
        System { db, by_family }
    })
}

/// Font file bytes, read once per process and kept for its lifetime (faces are reused by
/// every later conversion).
fn bytes_of(id: fontdb::ID) -> Option<&'static [u8]> {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static [u8]>>> = OnceLock::new();
    let sys = system();
    let info = sys.db.face(id)?;
    let key = match &info.source {
        fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p.display().to_string(),
        fontdb::Source::Binary(_) => format!("#{id:?}"),
    };
    let cache = CACHE.get_or_init(Default::default);
    if let Some(b) = cache.lock().ok()?.get(&key) {
        return Some(b);
    }
    let data = sys.db.with_face_data(id, |d, _| d.to_vec())?;
    let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
    cache.lock().ok()?.insert(key, leaked);
    Some(leaked)
}

#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub id: u16,
    /// Byte offset of the glyph's cluster in the shaped text.
    pub cluster: u32,
    /// Advance and offsets in ems.
    pub adv: f32,
    pub dx: f32,
    pub dy: f32,
}

pub struct Face {
    pub pdf: krilla::text::Font,
    rb: rustybuzz::Face<'static>,
    upem: f32,
    /// Ascent (with external leading) and descent, in ems.
    pub asc: f32,
    pub desc: f32,
    pub ul_pos: f32,
    pub ul_th: f32,
    pub st_pos: f32,
    pub st_th: f32,
    pub synth_bold: bool,
    pub synth_italic: bool,
}

impl Face {
    fn load(id: fontdb::ID, want_bold: bool, want_italic: bool) -> Option<Face> {
        let sys = system();
        let info = sys.db.face(id)?;
        let data = bytes_of(id)?;
        let rb = rustybuzz::Face::from_slice(data, info.index)?;
        if let Some(os2) = rb.tables().os2
            && os2.permissions() == Some(ttf_parser::Permissions::Restricted)
        {
            return None;
        }
        let pdf = krilla::text::Font::new(krilla::Data::from(data), info.index)?;
        let upem = rb.units_per_em() as f32;
        let (h_asc, h_desc, gap) = (
            rb.ascender() as f32,
            -(rb.descender() as f32),
            rb.line_gap() as f32,
        );
        let (asc, desc) = match rb.tables().os2 {
            Some(o)
                if o.windows_ascender() > 0
                    && (o.windows_ascender() as i32 + o.windows_descender().abs() as i32) > 0 =>
            {
                let wa = o.windows_ascender() as f32;
                let wd = (o.windows_descender() as f32).abs();
                let ext = (gap - ((wa + wd) - (h_asc + h_desc))).max(0.0);
                (wa + ext, wd)
            }
            _ => (h_asc + gap.max(0.0), h_desc),
        };
        let ul = rb.underline_metrics();
        let st = rb.strikeout_metrics();
        let weight = info.weight.0;
        let italic = info.style != fontdb::Style::Normal;
        Some(Face {
            pdf,
            asc: asc / upem,
            desc: desc / upem,
            ul_pos: ul.map_or(-0.1, |m| m.position as f32 / upem),
            ul_th: ul.map_or(0.05, |m| (m.thickness as f32 / upem).max(0.02)),
            st_pos: st.map_or(0.26, |m| m.position as f32 / upem),
            st_th: st.map_or(0.05, |m| (m.thickness as f32 / upem).max(0.02)),
            synth_bold: want_bold && weight < 550,
            synth_italic: want_italic && !italic,
            upem,
            rb,
        })
    }

    pub fn has(&self, c: char) -> bool {
        self.rb.glyph_index(c).is_some()
    }
}

/// Weight implied by style words at the end of a family name ("Calibri Light").
fn split_style(name: &str) -> (String, Option<u16>) {
    let words = [
        ("extralight", 200),
        ("ultralight", 200),
        ("semilight", 350),
        ("light", 300),
        ("thin", 100),
        ("medium", 500),
        ("semibold", 600),
        ("demibold", 600),
        ("extrabold", 800),
        ("ultrabold", 800),
        ("black", 900),
        ("heavy", 900),
    ];
    let lower = name.to_lowercase();
    for (w, weight) in words {
        if let Some(base) = lower.strip_suffix(w) {
            let base = base.trim_end().trim_end_matches('-').trim_end();
            if !base.is_empty() {
                return (base.to_string(), Some(weight));
            }
        }
    }
    (lower, None)
}

fn substitutes(family: &str) -> &'static [&'static str] {
    match family {
        "calibri" | "calibri light" => {
            &["carlito", "aptos", "segoe ui", "liberation sans", "arial"]
        }
        "aptos" | "aptos display" | "aptos narrow" => {
            &["calibri", "carlito", "segoe ui", "liberation sans", "arial"]
        }
        "cambria" | "cambria math" => &["caladea", "liberation serif", "times new roman"],
        "times new roman" | "times" | "tms rmn" => {
            &["liberation serif", "tinos", "dejavu serif", "noto serif"]
        }
        "arial" | "helvetica" | "arial nova" | "helvetica neue" => {
            &["liberation sans", "arimo", "dejavu sans", "noto sans"]
        }
        "arial narrow" => &["liberation sans narrow", "arial", "liberation sans"],
        "courier new" | "courier" => &["liberation mono", "cousine", "dejavu sans mono"],
        "consolas" | "lucida console" | "cascadia code" | "cascadia mono" => {
            &["courier new", "liberation mono", "dejavu sans mono"]
        }
        "georgia" | "palatino linotype" | "book antiqua" | "garamond" | "constantia" => &[
            "gelasio",
            "times new roman",
            "liberation serif",
            "dejavu serif",
        ],
        "verdana"
        | "tahoma"
        | "segoe ui"
        | "trebuchet ms"
        | "corbel"
        | "candara"
        | "century gothic"
        | "franklin gothic book"
        | "gill sans mt"
        | "lucida sans" => &["dejavu sans", "noto sans", "liberation sans", "arial"],
        "symbol" => &["standard symbols ps", "opensymbol", "segoe ui symbol"],
        "wingdings" | "wingdings 2" | "wingdings 3" | "webdings" => {
            &["opensymbol", "segoe ui symbol"]
        }
        "ms mincho" | "ms pmincho" | "simsun" | "nsimsun" | "mingliu" | "pmingliu" => &[
            "noto serif cjk sc",
            "noto sans cjk sc",
            "microsoft yahei",
            "yu mincho",
        ],
        "ms gothic" | "ms pgothic" | "yu gothic" | "meiryo" | "microsoft yahei" | "dengxian"
        | "malgun gothic" | "simhei" => &[
            "noto sans cjk sc",
            "noto sans cjk jp",
            "microsoft yahei",
            "yu gothic",
        ],
        "mangal" | "kokila" | "aparajita" | "utsaah" | "nirmala ui" => &[
            "nirmala ui",
            "noto sans devanagari",
            "lohit devanagari",
            "mangal",
        ],
        _ => &[],
    }
}

fn generic_list(g: Generic) -> &'static [&'static str] {
    match g {
        Generic::Serif => &[
            "times new roman",
            "liberation serif",
            "dejavu serif",
            "noto serif",
        ],
        Generic::Mono => &[
            "courier new",
            "liberation mono",
            "dejavu sans mono",
            "noto sans mono",
        ],
        Generic::Sans => &[
            "arial",
            "liberation sans",
            "segoe ui",
            "dejavu sans",
            "noto sans",
        ],
    }
}

/// Fallback families for a character the chosen face lacks, by script: only fonts that
/// can plausibly cover it are opened (CJK fonts are tens of megabytes).
fn fallbacks_for(c: char) -> &'static [&'static str] {
    match c as u32 {
        0x0900..=0x0DFF => &[
            "nirmala ui",
            "noto sans devanagari",
            "noto sans bengali",
            "noto sans tamil",
            "noto sans telugu",
            "noto sans gujarati",
            "noto sans gurmukhi",
            "noto sans kannada",
            "noto sans malayalam",
            "noto sans oriya",
            "noto sans sinhala",
            "mangal",
            "lohit devanagari",
        ],
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF => &[
            "arial",
            "times new roman",
            "segoe ui",
            "noto sans arabic",
            "noto sans hebrew",
            "dejavu sans",
        ],
        0x0E00..=0x0EFF => &["leelawadee ui", "tahoma", "noto sans thai", "noto sans lao"],
        0x3040..=0x30FF | 0x31F0..=0x31FF => &[
            "yu gothic",
            "ms gothic",
            "meiryo",
            "noto sans cjk jp",
            "noto sans cjk sc",
        ],
        0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => {
            &["malgun gothic", "noto sans cjk kr", "noto sans cjk sc"]
        }
        0x2E80..=0x2FFF
        | 0x3000..=0x303F
        | 0x3400..=0x9FFF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF
        | 0x20000..=0x2FFFF => &[
            "microsoft yahei",
            "simsun",
            "yu gothic",
            "noto sans cjk sc",
            "noto sans cjk jp",
            "droid sans fallback",
        ],
        0x1F000..=0x1FAFF | 0x2600..=0x27BF => &[
            "segoe ui emoji",
            "segoe ui symbol",
            "noto color emoji",
            "noto sans symbols2",
            "dejavu sans",
        ],
        0x2200..=0x22FF | 0x2A00..=0x2AFF | 0x1D400..=0x1D7FF => &[
            "cambria math",
            "segoe ui symbol",
            "noto sans math",
            "dejavu sans",
        ],
        _ => &[
            "segoe ui",
            "arial",
            "segoe ui symbol",
            "segoe ui historic",
            "arial unicode ms",
            "noto sans",
            "noto sans symbols",
            "noto sans symbols2",
            "dejavu sans",
            "freesans",
        ],
    }
}

/// Characters that draw nothing, so no face needs them.
fn invisible(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{2064}' | '\u{feff}')
}

pub struct Fonts {
    faces: Vec<Face>,
    by_key: HashMap<(String, bool, bool), Option<usize>>,
    by_id: HashMap<(fontdb::ID, bool, bool), Option<usize>>,
    fallback: HashMap<(usize, char), Option<usize>>,
    shaped: HashMap<(usize, bool, bool, String), Arc<Vec<Glyph>>>,
    generics: HashMap<String, Generic>,
}

impl Fonts {
    pub fn new(generics: HashMap<String, Generic>) -> Fonts {
        Fonts {
            faces: Vec::new(),
            by_key: HashMap::new(),
            by_id: HashMap::new(),
            fallback: HashMap::new(),
            shaped: HashMap::new(),
            generics,
        }
    }

    pub fn get(&self, i: usize) -> &Face {
        &self.faces[i]
    }

    /// Whether this system has any usable font at all.
    pub fn available() -> bool {
        !system().db.is_empty()
    }

    fn load_id(&mut self, id: fontdb::ID, bold: bool, italic: bool) -> Option<usize> {
        if let Some(r) = self.by_id.get(&(id, bold, italic)) {
            return *r;
        }
        let r = Face::load(id, bold, italic).map(|f| {
            self.faces.push(f);
            self.faces.len() - 1
        });
        self.by_id.insert((id, bold, italic), r);
        r
    }

    /// The best face of one family, or None when the family isn't installed.
    fn family(&mut self, family: &str, bold: bool, italic: bool) -> Option<usize> {
        let sys = system();
        let (base, style_weight) = split_style(family);
        let lower = family.to_lowercase();
        let (ids, weight) = match sys.by_family.get(&lower) {
            Some(ids) => (ids, None),
            None => (sys.by_family.get(&base)?, style_weight),
        };
        let want_w = if bold {
            weight.map_or(700, |w| w.max(700))
        } else {
            weight.unwrap_or(400)
        } as i32;
        let mut best: Option<(i32, fontdb::ID)> = None;
        for &id in ids {
            let Some(f) = sys.db.face(id) else { continue };
            let it = f.style != fontdb::Style::Normal;
            let w = f.weight.0 as i32;
            // Prefer the right slant, then the nearest weight, heavier when bold.
            let mut score = if it == italic { 0 } else { 10_000 };
            score += (w - want_w).abs() * if bold && w < want_w { 2 } else { 1 };
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, id));
            }
        }
        let (_, id) = best?;
        self.load_id(id, bold, italic)
    }

    /// A face for a run: the family, a metric-compatible substitute, the generic class
    /// from the font table, then anything at all.
    pub fn face(&mut self, family: &str, bold: bool, italic: bool) -> Option<usize> {
        let key = (family.to_string(), bold, italic);
        if let Some(r) = self.by_key.get(&key) {
            return *r;
        }
        // HTML imports write CSS-style lists: "Calibri;sans-serif".
        let names: Vec<String> = family
            .split([';', ','])
            .map(|s| s.trim().trim_matches(['"', '\'']).to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let lower = names.first().cloned().unwrap_or_default();
        let css_generic = names.iter().find_map(|n| match n.as_str() {
            "serif" => Some(Generic::Serif),
            "sans-serif" | "system-ui" | "ui-sans-serif" => Some(Generic::Sans),
            "monospace" | "ui-monospace" => Some(Generic::Mono),
            _ => None,
        });
        let mut r = None;
        for n in &names {
            r = self.family(n, bold, italic);
            if r.is_some() {
                break;
            }
        }
        if r.is_none() {
            'subs: for n in &names {
                for s in substitutes(n) {
                    r = self.family(s, bold, italic);
                    if r.is_some() {
                        break 'subs;
                    }
                }
            }
        }
        if r.is_none() {
            let g = css_generic
                .or_else(|| self.generics.get(&lower).copied())
                .unwrap_or_else(|| {
                    if lower.contains("mono") || lower.contains("courier") || lower.contains("code")
                    {
                        Generic::Mono
                    } else if lower.contains("serif") && !lower.contains("sans") {
                        Generic::Serif
                    } else {
                        Generic::Sans
                    }
                });
            for s in generic_list(g) {
                r = self.family(s, bold, italic);
                if r.is_some() {
                    break;
                }
            }
        }
        if r.is_none() {
            let ids: Vec<fontdb::ID> = system().db.faces().map(|f| f.id).take(64).collect();
            for id in ids {
                r = self.load_id(id, bold, italic);
                if r.is_some() {
                    break;
                }
            }
        }
        self.by_key.insert(key, r);
        r
    }

    /// A face that has `c`, preferring `face` itself.
    pub fn cover(&mut self, face: usize, c: char, bold: bool, italic: bool) -> usize {
        if invisible(c) || self.faces[face].has(c) {
            return face;
        }
        if let Some(r) = self.fallback.get(&(face, c)) {
            return r.unwrap_or(face);
        }
        let mut found = None;
        for fam in fallbacks_for(c) {
            if let Some(f) = self.family(fam, bold, italic)
                && self.faces[f].has(c)
            {
                found = Some(f);
                break;
            }
        }
        self.fallback.insert((face, c), found);
        found.unwrap_or(face)
    }

    /// Shape `text` with `face`. Advances are in ems.
    pub fn shape(&mut self, face: usize, text: &str, rtl: bool, kern: bool) -> Arc<Vec<Glyph>> {
        let key = (face, rtl, kern, text.to_string());
        if let Some(g) = self.shaped.get(&key) {
            return g.clone();
        }
        let f = &self.faces[face];
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.set_direction(if rtl {
            rustybuzz::Direction::RightToLeft
        } else {
            rustybuzz::Direction::LeftToRight
        });
        buf.guess_segment_properties();
        let simple = text.chars().all(|c| (c as u32) < 0x0590);
        let tag = |t: &[u8; 4]| ttf_parser::Tag::from_bytes(t);
        let mut features = Vec::new();
        if !kern {
            features.push(rustybuzz::Feature::new(tag(b"kern"), 0, ..));
        }
        if simple {
            features.push(rustybuzz::Feature::new(tag(b"liga"), 0, ..));
            features.push(rustybuzz::Feature::new(tag(b"clig"), 0, ..));
        }
        let out = rustybuzz::shape(&f.rb, &features, buf);
        let upem = f.upem;
        let glyphs: Vec<Glyph> = out
            .glyph_infos()
            .iter()
            .zip(out.glyph_positions())
            .map(|(i, p)| Glyph {
                id: i.glyph_id as u16,
                cluster: i.cluster,
                adv: p.x_advance as f32 / upem,
                dx: p.x_offset as f32 / upem,
                dy: p.y_offset as f32 / upem,
            })
            .collect();
        let g = Arc::new(glyphs);
        if self.shaped.len() < 200_000 {
            self.shaped.insert(key, g.clone());
        }
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_words() {
        assert_eq!(split_style("Calibri Light"), ("calibri".into(), Some(300)));
        assert_eq!(
            split_style("Segoe UI Semibold"),
            ("segoe ui".into(), Some(600))
        );
        assert_eq!(split_style("Arial"), ("arial".into(), None));
    }

    #[test]
    fn finds_some_font_and_shapes() {
        if !Fonts::available() {
            eprintln!("SKIPPED: no system fonts");
            return;
        }
        let mut f = Fonts::new(HashMap::new());
        let Some(id) = f.face("Arial", false, false) else {
            eprintln!("SKIPPED: no usable face");
            return;
        };
        let g = f.shape(id, "Hello", false, false);
        assert_eq!(g.len(), 5);
        assert!(g.iter().all(|g| g.adv > 0.0));
        let face = f.get(id);
        assert!(face.asc > 0.5 && face.desc > 0.0 && face.asc + face.desc < 2.0);
    }
}
