//! Font identity (docs/13 §4.5): PostScript names → the family, weight and slant Word needs.

use super::model::FontInfo;
use std::collections::HashMap;

/// Everything the PDF says about a font, before cleanup.
#[derive(Clone, Debug, Default)]
pub struct FontHints {
    pub ps_name: Option<String>,
    pub weight: Option<u32>,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
    pub force_bold: bool,
}

/// Interned fonts for one document.
#[derive(Default)]
pub struct FontBook {
    pub fonts: Vec<FontInfo>,
    index: HashMap<FontInfo, u16>,
}

impl FontBook {
    pub fn intern(&mut self, f: FontInfo) -> u16 {
        if let Some(&i) = self.index.get(&f) {
            return i;
        }
        let i = u16::try_from(self.fonts.len()).unwrap_or(u16::MAX);
        if i == u16::MAX {
            return 0; // 65 535 distinct fonts: reuse the first rather than overflow
        }
        self.fonts.push(f.clone());
        self.index.insert(f, i);
        i
    }
}

fn strip_subset(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 && tag.bytes().all(|b| b.is_ascii_uppercase()) => rest,
        _ => name,
    }
}

/// Insert spaces at CamelCase boundaries, keeping acronyms ("SegoeUI" → "Segoe UI").
fn uncamel(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    for (i, &ch) in c.iter().enumerate() {
        if i > 0 && ch.is_uppercase() {
            let prev = c[i - 1];
            let next_lower = c.get(i + 1).is_some_and(|n| n.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(ch);
    }
    out.replace('_', " ")
}

const STYLE_WORDS: &[&str] = &[
    "bolditalic",
    "boldoblique",
    "bold",
    "italic",
    "oblique",
    "regular",
    "semibold",
    "demibold",
    "extrabold",
    "black",
    "heavy",
    "light",
    "medium",
    "book",
    "roman",
    "it",
];

fn is_bold_word(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    ["bold", "black", "heavy", "demi", "bd"]
        .iter()
        .any(|w| l.contains(w))
}

fn is_italic_word(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("italic") || l.contains("oblique") || l.ends_with("it") || l == "ital"
}

/// Families that Word users know under another name, plus TeX's Computer Modern.
fn alias(family: &str) -> Option<(&'static str, bool, bool)> {
    let k: String = family
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    // (family, serif, mono)
    let a = match k.as_str() {
        "helvetica" | "helveticaneue" | "arialmt" | "arial" | "nimbussans" | "liberationsans" => {
            ("Arial", false, false)
        }
        "times" | "timesroman" | "timesnewroman" | "timesnewromanps" | "nimbusroman"
        | "liberationserif" => ("Times New Roman", true, false),
        "courier" | "couriernew" | "couriernewps" | "nimbusmono" | "liberationmono" => {
            ("Courier New", false, true)
        }
        "symbol" => ("Symbol", true, false),
        "zapfdingbats" | "dingbats" => ("Segoe UI Symbol", false, false),
        _ => {
            // Computer Modern / Latin Modern: cmr10, cmbx12, cmti10, cmtt10, lmroman10…
            let cm = k.strip_prefix("cm").or_else(|| k.strip_prefix("lm"));
            match cm {
                Some(r) if r.starts_with("tt") || r.starts_with("mono") => {
                    ("Consolas", false, true)
                }
                Some(r) if r.starts_with("ss") || r.starts_with("sans") => ("Arial", false, false),
                Some(r)
                    if r.starts_with('r')
                        || r.starts_with("bx")
                        || r.starts_with("ti")
                        || r.starts_with("sl")
                        || r.starts_with("mi")
                        || r.starts_with("csc")
                        || r.starts_with("roman") =>
                {
                    ("Cambria", true, false)
                }
                _ => return None,
            }
        }
    };
    Some(a)
}

fn guess_serif(family: &str) -> bool {
    let l = family.to_ascii_lowercase();
    !l.contains("sans")
        && [
            "times",
            "georgia",
            "garamond",
            "serif",
            "cambria",
            "minion",
            "palatino",
            "book",
            "baskerville",
            "caslon",
            "bodoni",
            "century",
            "constantia",
            "didot",
            "charter",
        ]
        .iter()
        .any(|w| l.contains(w))
}

fn guess_mono(family: &str) -> bool {
    let l = family.to_ascii_lowercase();
    [
        "mono",
        "courier",
        "consolas",
        "code",
        "typewriter",
        "menlo",
        "fixed",
    ]
    .iter()
    .any(|w| l.contains(w))
}

/// Clean a PDF font into what Word should be told.
pub fn describe(h: &FontHints) -> FontInfo {
    let raw = h.ps_name.as_deref().map(strip_subset).unwrap_or("");
    // Family and style split at the first '-' or ','.
    let (fam_raw, style) = match raw.find(['-', ',']) {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, ""),
    };
    let mut fam = fam_raw.to_string();
    for suffix in ["PSMT", "MT", "PS"] {
        if fam.len() > suffix.len() + 2 && fam.ends_with(suffix) {
            fam.truncate(fam.len() - suffix.len());
            break;
        }
    }
    let mut words: Vec<String> = uncamel(&fam)
        .split_whitespace()
        .map(str::to_string)
        .collect();
    // "ArialBold" with no separator: peel style words off the end.
    let mut tail_style = String::new();
    while words.len() > 1 {
        let last = words[words.len() - 1].to_ascii_lowercase();
        if STYLE_WORDS.contains(&last.as_str()) && last != "book" && last != "roman" {
            tail_style.insert_str(0, &words.pop().unwrap_or_default());
        } else {
            break;
        }
    }
    let style_all = format!("{style}{tail_style}");
    let mut family = words.join(" ");
    let mut bold = h.force_bold
        || h.weight.is_some_and(|w| w >= 600)
        || is_bold_word(&style_all)
        || raw.to_ascii_lowercase().contains("bold");
    let mut italic = h.italic || is_italic_word(&style_all);
    let (mut serif, mut mono) = (h.serif, h.mono);
    if let Some((a, s, m)) = alias(&family) {
        // TeX names carry the style in the name: cmbx = bold, cmti = italic.
        let k = family.to_ascii_lowercase();
        if k.starts_with("cmbx") || k.starts_with("lmromandemi") {
            bold = true;
        }
        if k.starts_with("cmti") || k.starts_with("cmsl") || k.starts_with("cmmi") {
            italic = true;
        }
        family = a.to_string();
        serif = s;
        mono = m;
    }
    if family.trim().is_empty() {
        family = if mono {
            "Courier New".into()
        } else if serif {
            "Times New Roman".into()
        } else {
            "Arial".into()
        };
    }
    if !serif && !mono {
        serif = guess_serif(&family);
    }
    if !mono {
        mono = guess_mono(&family);
    }
    FontInfo {
        family,
        bold,
        italic,
        serif,
        mono,
    }
}

/// Families installed on this PC, or None when we can't tell (then names are kept as-is).
pub fn installed() -> Option<&'static std::collections::HashSet<String>> {
    static SET: std::sync::OnceLock<Option<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    SET.get_or_init(platform_fonts).as_ref()
}

#[cfg(windows)]
fn platform_fonts() -> Option<std::collections::HashSet<String>> {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumValueW,
        RegOpenKeyExW,
    };
    use windows::core::{PCWSTR, PWSTR};
    let path: Vec<u16> = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Fonts\0"
        .encode_utf16()
        .collect();
    let mut set = std::collections::HashSet::new();
    for root in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let mut key = HKEY::default();
        // SAFETY: `path` is NUL-terminated and outlives the call; `key` is a valid out-pointer.
        let ok = unsafe { RegOpenKeyExW(root, PCWSTR(path.as_ptr()), Some(0), KEY_READ, &mut key) };
        if ok.is_err() {
            continue;
        }
        let mut name = vec![0u16; 512];
        for i in 0.. {
            let mut len = name.len() as u32;
            // SAFETY: `name` holds `len` u16s; other out-params are optional and passed as None.
            let r = unsafe {
                RegEnumValueW(
                    key,
                    i,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut len,
                    None,
                    None,
                    None,
                    None,
                )
            };
            if r.is_err() {
                break;
            }
            let value = String::from_utf16_lossy(&name[..len as usize]);
            for fam in registry_families(&value) {
                set.insert(fam.to_ascii_lowercase());
            }
        }
        // SAFETY: `key` was opened above and is closed once.
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
    (!set.is_empty()).then_some(set)
}

#[cfg(not(windows))]
fn platform_fonts() -> Option<std::collections::HashSet<String>> {
    None
}

/// "Calibri Bold Italic (TrueType)" → ["Calibri"]; "Cambria & Cambria Math (TrueType)" → both.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn registry_families(value: &str) -> Vec<String> {
    let base = value.split(" (").next().unwrap_or(value);
    base.split(" & ")
        .map(|f| {
            let mut w: Vec<&str> = f.split_whitespace().collect();
            while w.len() > 1
                && matches!(
                    w[w.len() - 1].to_ascii_lowercase().as_str(),
                    "bold"
                        | "italic"
                        | "oblique"
                        | "regular"
                        | "light"
                        | "semibold"
                        | "semilight"
                        | "black"
                        | "medium"
                        | "thin"
                        | "extralight"
                        | "extrabold"
                        | "condensed"
                        | "demibold"
                )
            {
                w.pop();
            }
            w.join(" ")
        })
        .filter(|f| !f.is_empty())
        .collect()
}

/// The family Word should use: the original when installed (or unknown), else a close standard.
pub fn substitute(f: &FontInfo) -> String {
    let Some(set) = installed() else {
        return f.family.clone();
    };
    if set.contains(&f.family.to_ascii_lowercase()) {
        return f.family.clone();
    }
    let fallback = if f.mono {
        "Courier New"
    } else if f.serif {
        "Times New Roman"
    } else {
        "Arial"
    };
    fallback.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(name: &str) -> FontInfo {
        describe(&FontHints {
            ps_name: Some(name.into()),
            ..Default::default()
        })
    }

    #[test]
    fn postscript_names_become_families() {
        let f = d("ABCDEF+Calibri-Bold");
        assert_eq!(
            (f.family.as_str(), f.bold, f.italic),
            ("Calibri", true, false)
        );
        let f = d("TimesNewRomanPS-BoldItalicMT");
        assert_eq!(
            (f.family.as_str(), f.bold, f.italic, f.serif),
            ("Times New Roman", true, true, true)
        );
        assert_eq!(d("ArialMT").family, "Arial");
        assert_eq!(d("Arial,BoldItalic").family, "Arial");
        assert!(d("Arial,BoldItalic").italic);
        assert_eq!(d("SegoeUI-Semibold").family, "Segoe UI");
        assert!(d("SegoeUI-Semibold").bold);
        assert_eq!(d("OpenSans-Regular").family, "Open Sans");
        assert_eq!(d("SourceSansPro-It").family, "Source Sans Pro");
        assert!(d("SourceSansPro-It").italic);
        assert_eq!(d("Helvetica-Oblique").family, "Arial");
        assert!(d("Helvetica-Oblique").italic);
        assert_eq!(d("CourierNewPSMT").family, "Courier New");
        assert!(d("CourierNewPSMT").mono);
        let f = d("CMBX12");
        assert_eq!((f.family.as_str(), f.bold), ("Cambria", true));
        assert!(d("CMTI10").italic);
        assert_eq!(d("CMTT10").family, "Consolas");
        assert_eq!(d("ArialBold").family, "Arial");
        assert!(d("ArialBold").bold);
        assert!(d("Georgia").serif);
        assert_eq!(d("PTSans-Regular").family, "PT Sans");
    }

    #[test]
    fn registry_value_names() {
        assert_eq!(
            registry_families("Calibri Bold Italic (TrueType)"),
            ["Calibri"]
        );
        assert_eq!(
            registry_families("Cambria & Cambria Math (TrueType)"),
            ["Cambria", "Cambria Math"]
        );
        assert_eq!(
            registry_families("Segoe UI Variable (TrueType)"),
            ["Segoe UI Variable"]
        );
    }
}
