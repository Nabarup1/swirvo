//! OpenDocument Text (ODT) → the layout model, so ODT gets the same page layout as DOCX:
//! page layouts and master pages (with headers and footers), paragraph and text styles
//! with inheritance, lists, tables, pictures and text boxes, footnotes and endnotes,
//! links, page-number fields and outline levels.

use std::collections::HashMap;
use std::sync::Arc;

use swirvo_core::EngineError;

use super::model::*;
use super::numbering::{NumFmt, Suffix, format_number};
use super::props::{
    Align, Border, BorderStyle, Leader, LineRule, Rgb, TabDef, TabKind, Underline, VAlign, VMerge,
    Vert, Width,
};
use super::xml::{El, Node};
use crate::opc::Package;

/// A length in points: `2cm`, `0.5in`, `12pt`, `10mm`, `1pc`, `96px`.
pub fn len(v: &str) -> Option<f32> {
    let v = v.trim();
    let split = v
        .find(|c: char| c.is_ascii_alphabetic() || c == '%')
        .unwrap_or(v.len());
    let (n, unit) = v.split_at(split);
    let n: f32 = n.trim().parse().ok()?;
    Some(match unit {
        "pt" | "" => n,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "in" | "inch" => n * 72.0,
        "pc" => n * 12.0,
        "px" => n * 0.75,
        _ => return None,
    })
}

fn color(v: &str) -> Option<Rgb> {
    if v == "transparent" {
        return None;
    }
    Rgb::hex(v)
}

/// `0.5pt solid #000000` → a border.
fn border(v: &str) -> Option<Border> {
    if v.trim() == "none" {
        return None;
    }
    let mut width = 0.5;
    let mut style = BorderStyle::Single;
    let mut col = Rgb(0, 0, 0);
    for part in v.split_whitespace() {
        if let Some(w) = len(part) {
            width = w;
        } else if let Some(c) = color(part) {
            col = c;
        } else {
            style = match part {
                "double" => BorderStyle::Double,
                "dotted" => BorderStyle::Dotted,
                "dashed" => BorderStyle::Dashed,
                "none" | "hidden" => return None,
                _ => BorderStyle::Single,
            };
        }
    }
    Some(Border {
        style,
        width,
        color: col,
        space: 0.0,
    })
}

// ------------------------------------------------------------ styles

#[derive(Debug, Clone, Default)]
struct Tp {
    font: Option<String>,
    size: Option<f32>,
    size_pct: Option<f32>,
    b: Option<bool>,
    i: Option<bool>,
    u: Option<bool>,
    strike: Option<bool>,
    color: Option<Rgb>,
    vert: Option<Vert>,
    caps: Option<bool>,
    small_caps: Option<bool>,
    highlight: Option<Rgb>,
}

#[derive(Debug, Clone, Default)]
struct Pp {
    align: Option<Align>,
    ml: Option<f32>,
    mr: Option<f32>,
    mt: Option<f32>,
    mb: Option<f32>,
    indent: Option<f32>,
    line: Option<LineRule>,
    bg: Option<Option<Rgb>>,
    borders: Option<[Option<Border>; 4]>,
    keep_next: Option<bool>,
    break_before: Option<bool>,
    tabs: Option<Vec<TabDef>>,
    /// `style:contextual-spacing`: no space between paragraphs of the same style.
    contextual: Option<bool>,
}

#[derive(Debug, Clone, Default)]
struct Style {
    parent: Option<String>,
    tp: Tp,
    pp: Pp,
    master: Option<String>,
    list: Option<String>,
    /// Table, column, row and cell properties as raw elements (read when used).
    table: Option<El>,
    column: Option<El>,
    row: Option<El>,
    cell: Option<El>,
    graphic: Option<El>,
    section: Option<El>,
    /// An automatic style (hard formatting on the paragraph), not one the user named.
    auto: bool,
}

fn on(v: Option<&str>, yes: &[&str]) -> Option<bool> {
    v.map(|v| yes.contains(&v))
}

impl Tp {
    fn parse(e: &El, fonts: &HashMap<String, String>) -> Tp {
        let a = |k: &str| e.attr(k);
        let size = a("font-size");
        Tp {
            font: a("font-name")
                .map(|n| fonts.get(n).cloned().unwrap_or_else(|| n.to_string()))
                .or_else(|| a("font-family").map(|f| f.trim_matches(['\'', '"']).to_string())),
            size: size.and_then(len),
            size_pct: size
                .and_then(|s| s.strip_suffix('%'))
                .and_then(|p| p.parse::<f32>().ok())
                .map(|p| p / 100.0),
            b: a("font-weight").map(|w| w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)),
            i: on(a("font-style"), &["italic", "oblique"]),
            u: a("text-underline-style").map(|u| u != "none"),
            strike: a("text-line-through-style").map(|u| u != "none"),
            color: a("color").and_then(color),
            vert: a("text-position").map(|p| {
                if p.starts_with("super")
                    || p.trim_start()
                        .starts_with(|c: char| c.is_ascii_digit() && c != '0')
                {
                    Vert::Super
                } else if p.starts_with("sub") || p.starts_with('-') {
                    Vert::Sub
                } else {
                    Vert::Baseline
                }
            }),
            caps: a("text-transform").map(|t| t == "uppercase"),
            small_caps: a("font-variant").map(|v| v == "small-caps"),
            highlight: a("background-color").and_then(color),
        }
    }

    fn over(&mut self, o: &Tp) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            font, b, i, u, strike, color, vert, caps, small_caps, highlight
        );
        if let Some(s) = o.size {
            self.size = Some(s);
        } else if let Some(p) = o.size_pct {
            self.size = Some(self.size.unwrap_or(12.0) * p);
        }
    }
}

impl Pp {
    fn parse(e: &El) -> Pp {
        let a = |k: &str| e.attr(k);
        let all = a("border").and_then(border);
        let side = |k: &str| a(k).map(border).unwrap_or(all);
        let has_border = [
            "border",
            "border-top",
            "border-left",
            "border-bottom",
            "border-right",
        ]
        .iter()
        .any(|k| a(k).is_some());
        Pp {
            align: a("text-align").map(|v| match v {
                "center" => Align::Center,
                "end" | "right" => Align::Right,
                "justify" => Align::Both,
                _ => Align::Left,
            }),
            ml: a("margin-left").and_then(len),
            mr: a("margin-right").and_then(len),
            mt: a("margin-top").and_then(len),
            mb: a("margin-bottom").and_then(len),
            indent: a("text-indent").and_then(len),
            line: a("line-height")
                .and_then(|v| {
                    if let Some(p) = v.strip_suffix('%') {
                        p.parse::<f32>().ok().map(|p| LineRule::Auto(p / 100.0))
                    } else {
                        len(v).map(LineRule::Exact)
                    }
                })
                .or_else(|| {
                    a("line-height-at-least")
                        .and_then(len)
                        .map(LineRule::AtLeast)
                }),
            bg: a("background-color").map(color),
            borders: has_border.then(|| {
                [
                    side("border-top"),
                    side("border-left"),
                    side("border-bottom"),
                    side("border-right"),
                ]
            }),
            keep_next: a("keep-with-next").map(|v| v == "always"),
            contextual: on(a("contextual-spacing"), &["true"]),
            break_before: a("break-before").map(|v| v == "page"),
            tabs: e.child("tab-stops").map(|t| {
                t.all("tab-stop")
                    .filter_map(|s| {
                        Some(TabDef {
                            pos: s.attr("position").and_then(len)?,
                            kind: match s.attr("type") {
                                Some("right") => TabKind::Right,
                                Some("center") => TabKind::Center,
                                Some("char") => TabKind::Decimal,
                                _ => TabKind::Left,
                            },
                            leader: match s.attr("leader-text").or(s.attr("leader-style")) {
                                Some(".") | Some("dotted") => Leader::Dot,
                                Some("-") | Some("dash") => Leader::Hyphen,
                                Some("_") | Some("solid") => Leader::Underscore,
                                _ => Leader::None,
                            },
                        })
                    })
                    .collect()
            }),
        }
    }

    fn over(&mut self, o: &Pp) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            align,
            ml,
            mr,
            mt,
            mb,
            indent,
            line,
            bg,
            borders,
            keep_next,
            break_before,
            tabs,
            contextual
        );
    }
}

/// Styles by family and name.
#[derive(Debug, Default)]
struct Styles {
    map: HashMap<(String, String), Style>,
    defaults: HashMap<String, Style>,
    lists: HashMap<String, El>,
    fonts: HashMap<String, String>,
    generics: HashMap<String, Generic>,
    tab: f32,
}

impl Styles {
    fn add(&mut self, root: &El, auto: bool) {
        for st in root.children() {
            let props = |s: &mut Style| {
                if let Some(t) = st.child("text-properties") {
                    s.tp = Tp::parse(t, &self.fonts);
                }
                if let Some(p) = st.child("paragraph-properties") {
                    s.pp = Pp::parse(p);
                }
                s.table = st.child("table-properties").cloned();
                s.column = st.child("table-column-properties").cloned();
                s.row = st.child("table-row-properties").cloned();
                s.cell = st.child("table-cell-properties").cloned();
                s.section = st.child("section-properties").cloned();
                s.graphic = st.child("graphic-properties").cloned();
            };
            match &*st.name {
                "style" => {
                    let (Some(name), Some(fam)) = (st.attr("name"), st.attr("family")) else {
                        continue;
                    };
                    let mut s = Style {
                        parent: st.attr("parent-style-name").map(str::to_string),
                        master: st
                            .attr("master-page-name")
                            .filter(|m| !m.is_empty())
                            .map(str::to_string),
                        list: st.attr("list-style-name").map(str::to_string),
                        auto,
                        ..Style::default()
                    };
                    props(&mut s);
                    self.map.insert((fam.to_string(), name.to_string()), s);
                }
                "default-style" => {
                    let Some(fam) = st.attr("family") else {
                        continue;
                    };
                    let mut s = Style::default();
                    props(&mut s);
                    if fam == "paragraph"
                        && let Some(t) = st
                            .child("paragraph-properties")
                            .and_then(|p| p.attr("tab-stop-distance"))
                            .and_then(len)
                    {
                        self.tab = t;
                    }
                    self.defaults.insert(fam.to_string(), s);
                }
                "list-style" => {
                    if let Some(n) = st.attr("name") {
                        self.lists.insert(n.to_string(), st.clone());
                    }
                }
                _ => {}
            }
        }
    }

    /// A style with its parents, root first; the family default at the bottom.
    fn chain(&self, fam: &str, name: Option<&str>) -> Vec<&Style> {
        let mut v = Vec::new();
        let mut cur = name.map(str::to_string);
        while let Some(n) = cur {
            let Some(s) = self.map.get(&(fam.to_string(), n.clone())) else {
                break;
            };
            if v.len() > 32 {
                break;
            }
            v.push(s);
            cur = s.parent.clone().filter(|p| *p != n);
        }
        if let Some(d) = self.defaults.get(fam) {
            v.push(d);
        }
        v.reverse();
        v
    }

    fn para(&self, name: Option<&str>) -> (Pp, Tp) {
        let mut pp = Pp::default();
        let mut tp = Tp::default();
        for s in self.chain("paragraph", name) {
            pp.over(&s.pp);
            tp.over(&s.tp);
        }
        (pp, tp)
    }

    /// A text style over `base` (the paragraph's text properties). The text family's
    /// default isn't applied: it would undo the paragraph's own properties.
    fn text(&self, name: &str, base: &Tp) -> Tp {
        let mut v = Vec::new();
        let mut cur = Some(name.to_string());
        while let Some(n) = cur {
            let Some(s) = self.map.get(&("text".to_string(), n.clone())) else {
                break;
            };
            if v.len() > 32 {
                break;
            }
            v.push(s);
            cur = s.parent.clone().filter(|p| *p != n);
        }
        let mut tp = base.clone();
        for s in v.iter().rev() {
            tp.over(&s.tp);
        }
        tp
    }

    fn get(&self, fam: &str, name: Option<&str>) -> Option<&Style> {
        self.map.get(&(fam.to_string(), name?.to_string()))
    }

    /// The list style a paragraph style names, through its parents.
    fn para_list(&self, name: Option<&str>) -> Option<String> {
        self.chain("paragraph", name)
            .iter()
            .rev()
            .find_map(|s| s.list.clone())
    }

    /// The nearest style in the chain the user named (automatic styles are hard
    /// formatting): what contextual spacing compares, as LibreOffice does.
    fn named(&self, name: Option<&str>) -> String {
        let mut cur = name.map(str::to_string);
        for _ in 0..32 {
            let Some(n) = cur else { break };
            match self.map.get(&("paragraph".to_string(), n.clone())) {
                Some(s) if s.auto => cur = s.parent.clone().filter(|p| *p != n),
                _ => return n,
            }
        }
        name.unwrap_or_default().to_string()
    }

    /// Whether a list paragraph takes its indents from the list level (label-alignment
    /// mode) rather than from its paragraph style. LibreOffice's rule: indents set as
    /// hard formatting win; then a list applied directly (an enclosing `text:list`, or
    /// an automatic style naming one) wins; then the nearest named style that sets either
    /// an indent or a list decides.
    fn list_indents_apply(&self, name: Option<&str>, in_list: bool) -> bool {
        let mut chain = self.chain("paragraph", name);
        chain.reverse();
        let sets_indent = |s: &Style| s.pp.ml.is_some() || s.pp.indent.is_some();
        let (auto, named): (Vec<&Style>, Vec<&Style>) = chain.iter().partition(|s| s.auto);
        if auto.iter().any(|s| sets_indent(s)) {
            return false;
        }
        if in_list || auto.iter().any(|s| s.list.is_some()) {
            return true;
        }
        for s in named {
            if s.list.is_some() {
                return true;
            }
            if sets_indent(s) {
                return false;
            }
        }
        true
    }
}

fn run_style(tp: &Tp) -> RunStyle {
    let font = tp
        .font
        .clone()
        .unwrap_or_else(|| "Liberation Serif".to_string());
    let sz = tp.size.unwrap_or(12.0).clamp(1.0, 1600.0);
    RunStyle {
        fonts: [font.clone(), font.clone(), font.clone(), font],
        b: tp.b.unwrap_or(false),
        i: tp.i.unwrap_or(false),
        b_cs: tp.b.unwrap_or(false),
        i_cs: tp.i.unwrap_or(false),
        sz,
        sz_cs: sz,
        color: tp.color.unwrap_or(Rgb(0, 0, 0)),
        auto_color: tp.color.is_none(),
        u: if tp.u == Some(true) {
            Underline::Single
        } else {
            Underline::None
        },
        u_color: None,
        strike: tp.strike.unwrap_or(false),
        dstrike: false,
        caps: tp.caps.unwrap_or(false),
        small_caps: tp.small_caps.unwrap_or(false),
        vanish: false,
        vert: tp.vert.unwrap_or_default(),
        position: 0.0,
        spacing: 0.0,
        scale: 1.0,
        kern: None,
        highlight: tp.highlight,
        shd: None,
        bdr: None,
        rtl: false,
        cs: false,
    }
}

fn para_style(pp: &Pp, name: &str) -> ParaStyle {
    let b = pp.borders.unwrap_or([None; 4]);
    ParaStyle {
        align: pp.align.unwrap_or(Align::Left),
        left: pp.ml.unwrap_or(0.0),
        right: pp.mr.unwrap_or(0.0),
        first: pp.indent.unwrap_or(0.0),
        before: pp.mt.unwrap_or(0.0),
        after: pp.mb.unwrap_or(0.0),
        line: pp.line.unwrap_or(LineRule::Auto(1.0)),
        contextual: pp.contextual.unwrap_or(false),
        keep_next: pp.keep_next.unwrap_or(false),
        keep_lines: false,
        page_break_before: pp.break_before.unwrap_or(false),
        widow: true,
        tabs: pp.tabs.clone().unwrap_or_default(),
        shd: pp.bg.flatten(),
        // Pp keeps ODF's top, left, bottom, right; the model wants top, bottom, left, right.
        borders: [b[0], b[2], b[1], b[3], None],
        bidi: false,
        style: name.to_string(),
    }
}

// ------------------------------------------------------------ reading

struct Reader<'a> {
    pkg: Package<'a>,
    st: Styles,
    media: HashMap<String, Option<Arc<Media>>>,
    /// List counters by (list style, level).
    counters: HashMap<(String, usize), u32>,
    notes: usize,
    footnotes: HashMap<String, Vec<Block>>,
    endnotes: HashMap<String, Vec<Block>>,
    uses_numpages: bool,
    depth: u32,
}

/// The list context of a paragraph: style name and level (0-based).
#[derive(Clone)]
struct ListCtx {
    style: String,
    level: usize,
    /// Only the first paragraph of a list item gets the label.
    first: bool,
}

pub fn read(bytes: &[u8]) -> Result<Document, EngineError> {
    let mut pkg = Package::open(bytes)?;
    let content = pkg
        .xml("content.xml")
        .ok_or_else(|| EngineError::Decode("not an OpenDocument file (no content.xml)".into()))?;
    let styles_el = pkg.xml("styles.xml");
    let mut st = Styles {
        tab: 36.0,
        ..Styles::default()
    };
    for root in [styles_el.as_ref(), Some(&content)].into_iter().flatten() {
        if let Some(fd) = root.child("font-face-decls") {
            for f in fd.all("font-face") {
                if let Some(n) = f.attr("name") {
                    let fam = f
                        .attr("font-family")
                        .unwrap_or(n)
                        .trim_matches(['\'', '"'])
                        .to_string();
                    let generic = match f.attr("font-family-generic") {
                        Some("roman") => Generic::Serif,
                        Some("modern") => Generic::Mono,
                        _ if f.attr("font-pitch") == Some("fixed") => Generic::Mono,
                        _ => Generic::Sans,
                    };
                    st.generics.insert(fam.to_ascii_lowercase(), generic);
                    st.fonts.insert(n.to_string(), fam);
                }
            }
        }
    }
    if let Some(s) = &styles_el {
        for k in ["styles", "automatic-styles"] {
            if let Some(x) = s.child(k) {
                st.add(x, k == "automatic-styles");
            }
        }
    }
    if let Some(x) = content.child("automatic-styles") {
        st.add(x, true);
    }
    let mut r = Reader {
        pkg,
        st,
        media: HashMap::new(),
        counters: HashMap::new(),
        notes: 0,
        footnotes: HashMap::new(),
        endnotes: HashMap::new(),
        uses_numpages: false,
        depth: 0,
    };
    let mut doc = Document {
        default_tab: r.st.tab.max(1.0),
        compat15: true,
        expand_shift_return: true,
        ..Document::default()
    };
    // Master pages → section properties and header/footer parts.
    let masters: Vec<El> = styles_el
        .as_ref()
        .and_then(|s| s.child("master-styles"))
        .map(|m| m.all("master-page").cloned().collect())
        .unwrap_or_default();
    let layouts: HashMap<String, El> = styles_el
        .as_ref()
        .and_then(|s| s.child("automatic-styles"))
        .map(|a| {
            a.all("page-layout")
                .filter_map(|l| Some((l.attr("name")?.to_string(), l.clone())))
                .collect()
        })
        .unwrap_or_default();
    let mut sect_of: HashMap<String, SectPr> = HashMap::new();
    for m in &masters {
        let Some(name) = m.attr("name") else { continue };
        let mut sp = SectPr::default();
        let layout = m.attr("page-layout-name").and_then(|n| layouts.get(n));
        let props = layout.and_then(|l| l.child("page-layout-properties"));
        let a = |k: &str| props.and_then(|p| p.attr(k)).and_then(len);
        sp.w = a("page-width").unwrap_or(595.3);
        sp.h = a("page-height").unwrap_or(841.9);
        let (mt, mb) = (
            a("margin-top").unwrap_or(56.7),
            a("margin-bottom").unwrap_or(56.7),
        );
        sp.left = a("margin-left").unwrap_or(56.7);
        sp.right = a("margin-right").unwrap_or(56.7);
        // The room a header or footer takes from the body. `fo:min-height` already counts
        // the spacing to the body (`fo:margin-bottom`/`-top`), as LibreOffice writes it.
        // With dynamic spacing (what DOCX imports get) the spacing gives way to the text,
        // as in Word: the body starts at margin + min-height, and only text reaching past
        // that pushes it down, which the layout does. Without it, text and spacing add up.
        let hf = |k: &str| -> Option<f32> {
            let p = layout?.child(k)?.child("header-footer-properties")?;
            let gap = p
                .attr("margin-bottom")
                .or(p.attr("margin-top"))
                .and_then(len)
                .unwrap_or(0.0);
            let dynamic = p.attr("dynamic-spacing") == Some("true");
            Some(match p.attr("min-height").and_then(len) {
                Some(h) if dynamic => h,
                Some(h) => h.max(gap + 14.0),
                None => gap + 14.0,
            })
        };
        // ODF margins run to the header; the body starts below it.
        let has_header = m.child("header").is_some();
        let has_footer = m.child("footer").is_some();
        sp.header = mt;
        sp.footer = mb;
        sp.top = mt
            + if has_header {
                hf("header-style").unwrap_or(14.0).max(14.0)
            } else {
                0.0
            };
        sp.bottom = mb
            + if has_footer {
                hf("footer-style").unwrap_or(14.0).max(14.0)
            } else {
                0.0
            };
        if let Some(c) = props.and_then(|p| p.child("columns")) {
            let n = c
                .attr("column-count")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let gap = c.attr("column-gap").and_then(len).unwrap_or(0.0);
            let w = ((sp.w - sp.left - sp.right) - gap * (n as f32 - 1.0)) / n as f32;
            sp.cols = Cols {
                cols: (0..n)
                    .map(|i| (w, if i + 1 < n { gap } else { 0.0 }))
                    .collect(),
                sep: c.child("column-sep").is_some(),
            };
        } else {
            let w = sp.w - sp.left - sp.right;
            sp.cols = Cols {
                cols: vec![(w, 0.0)],
                sep: false,
            };
        }
        for (k, slot, first) in [
            ("header", 0, false),
            ("header-first", 1, true),
            ("header-left", 2, false),
        ] {
            if let Some(h) = m.child(k) {
                let id = format!("{name}#{k}");
                let blocks = r.blocks(h, None);
                doc.parts.insert(id.clone(), blocks);
                sp.headers[slot] = Some(id);
                if first {
                    sp.title_pg = true;
                }
            }
        }
        for (k, slot, first) in [
            ("footer", 0, false),
            ("footer-first", 1, true),
            ("footer-left", 2, false),
        ] {
            if let Some(f) = m.child(k) {
                let id = format!("{name}#{k}");
                let blocks = r.blocks(f, None);
                doc.parts.insert(id.clone(), blocks);
                sp.footers[slot] = Some(id);
                if first {
                    sp.title_pg = true;
                }
            }
        }
        sect_of.insert(name.to_string(), sp);
    }
    // A master page that hands over to another after one page ("First Page" → "Standard")
    // is the other one with a different first page.
    for m in &masters {
        let (Some(name), Some(next)) = (m.attr("name"), m.attr("next-style-name")) else {
            continue;
        };
        if name == next {
            continue;
        }
        let (Some(first), Some(rest)) = (sect_of.get(name).cloned(), sect_of.get(next).cloned())
        else {
            continue;
        };
        let mut sp = rest;
        sp.title_pg = true;
        sp.headers[1] = first.headers[0].clone();
        sp.footers[1] = first.footers[0].clone();
        sect_of.insert(name.to_string(), sp);
    }
    let default_master = masters
        .first()
        .and_then(|m| m.attr("name"))
        .unwrap_or("Standard")
        .to_string();
    let text = content
        .child("body")
        .and_then(|b| b.child("text"))
        .ok_or_else(|| EngineError::Decode("this OpenDocument file has no text body".into()))?;
    // Body: split into sections where a paragraph style names a master page, and around
    // text sections with columns (continuous section breaks, as Word has them).
    let mut body = Body {
        current: sect_of.get(&default_master).cloned().unwrap_or_default(),
        blocks: Vec::new(),
        sections: Vec::new(),
    };
    walk_body(&mut r, text, &sect_of, &mut body, 0);
    let mut sections = body.sections;
    sections.push(Section {
        sect: body.current,
        blocks: body.blocks,
    });
    doc.sections = sections;
    doc.footnotes = std::mem::take(&mut r.footnotes);
    doc.endnotes = std::mem::take(&mut r.endnotes);
    doc.uses_numpages = r.uses_numpages;
    doc.generics = std::mem::take(&mut r.st.generics);
    if let Some(meta) = r.pkg.xml("meta.xml")
        && let Some(m) = meta.child("meta")
    {
        let get = |k: &str| {
            m.child(k)
                .map(|e| e.text().trim().to_string())
                .filter(|s| !s.is_empty())
        };
        doc.meta = Meta {
            title: get("title"),
            author: get("creator").or_else(|| get("initial-creator")),
            subject: get("subject"),
            keywords: get("keyword"),
            language: get("language"),
        };
    }
    Ok(doc)
}

struct Body {
    current: SectPr,
    blocks: Vec<Block>,
    sections: Vec<Section>,
}

impl Body {
    /// End the current section (if it has anything) and continue with `next`.
    fn switch(&mut self, next: SectPr) {
        if !self.blocks.is_empty() {
            self.sections.push(Section {
                sect: self.current.clone(),
                blocks: std::mem::take(&mut self.blocks),
            });
        }
        self.current = next;
    }
}

/// Columns of a text section's style, if it has more than one.
fn section_cols(r: &Reader<'_>, el: &El, page: &SectPr) -> Option<Cols> {
    let props =
        r.st.get("section", el.attr("style-name"))?
            .section
            .clone()?;
    let c = props.child("columns")?;
    let n = c
        .attr("column-count")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 16);
    if n < 2 {
        return None;
    }
    let gap = c.attr("column-gap").and_then(len).unwrap_or(0.0);
    let w = ((page.w - page.left - page.right) - gap * (n as f32 - 1.0)) / n as f32;
    Some(Cols {
        cols: (0..n)
            .map(|i| (w.max(18.0), if i + 1 < n { gap } else { 0.0 }))
            .collect(),
        sep: c.child("column-sep").is_some(),
    })
}

fn walk_body(
    r: &mut Reader<'_>,
    parent: &El,
    sect_of: &HashMap<String, SectPr>,
    body: &mut Body,
    depth: usize,
) {
    for el in parent.children() {
        if &*el.name == "section" && depth < 16 {
            match section_cols(r, el, &body.current) {
                Some(cols) => {
                    let outer = body.current.clone();
                    let mut inner = outer.clone();
                    inner.cols = cols;
                    inner.kind = SectKind::Continuous;
                    // A first-page header belongs to the page the section began on.
                    inner.title_pg = false;
                    body.switch(inner);
                    walk_body(r, el, sect_of, body, depth + 1);
                    // Back to the page's columns after it.
                    let mut after = body.current.clone();
                    after.cols = outer.cols;
                    after.kind = SectKind::Continuous;
                    after.title_pg = false;
                    body.switch(after);
                }
                None => walk_body(r, el, sect_of, body, depth + 1),
            }
            continue;
        }
        let style = el.attr("style-name");
        let master =
            r.st.get("paragraph", style)
                .and_then(|s| s.master.clone())
                .or_else(|| r.st.get("table", style).and_then(|s| s.master.clone()));
        if let Some(m) = master
            && let Some(sp) = sect_of.get(&m)
        {
            body.switch(sp.clone());
        }
        r.block(el, None, &mut body.blocks);
    }
}

impl Reader<'_> {
    fn blocks(&mut self, parent: &El, list: Option<&ListCtx>) -> Vec<Block> {
        let mut out = Vec::new();
        for el in parent.children() {
            self.block(el, list, &mut out);
        }
        out
    }

    fn block(&mut self, el: &El, list: Option<&ListCtx>, out: &mut Vec<Block>) {
        if self.depth > 24 {
            return;
        }
        match &*el.name {
            "p" | "h" => out.push(Block::Para(Box::new(self.para(el, list)))),
            "list" => {
                self.depth += 1;
                let style = el
                    .attr("style-name")
                    .map(str::to_string)
                    .or_else(|| list.map(|l| l.style.clone()))
                    .unwrap_or_default();
                let level = list.map_or(0, |l| l.level + 1).min(9);
                if list.is_none() && el.attr("continue-numbering") != Some("true") {
                    self.counters.retain(|(s, _), _| *s != style);
                }
                for item in el.children() {
                    if !matches!(&*item.name, "list-item" | "list-header") {
                        continue;
                    }
                    let header = &*item.name == "list-header";
                    if !header {
                        *self.counters.entry((style.clone(), level)).or_insert(0) += 1;
                        // Deeper levels restart.
                        self.counters.retain(|(s, l), _| *s != style || *l <= level);
                    }
                    let mut first = !header;
                    for child in item.children() {
                        let ctx = ListCtx {
                            style: style.clone(),
                            level,
                            first,
                        };
                        if &*child.name == "list" {
                            self.block(
                                child,
                                Some(&ListCtx {
                                    first: false,
                                    ..ctx
                                }),
                                out,
                            );
                        } else {
                            self.block(child, Some(&ctx), out);
                            first = false;
                        }
                    }
                }
                self.depth -= 1;
            }
            "table" => {
                self.depth += 1;
                if let Some(t) = self.table(el) {
                    out.push(Block::Table(Box::new(t)));
                }
                self.depth -= 1;
            }
            "section" | "index-body" | "table-of-content" | "alphabetical-index"
            | "illustration-index" | "user-index" | "bibliography" => {
                self.depth += 1;
                let inner = if &*el.name == "table-of-content" {
                    el.child("index-body").unwrap_or(el)
                } else {
                    el
                };
                for c in inner.children() {
                    self.block(c, list, out);
                }
                self.depth -= 1;
            }
            _ => {}
        }
    }

    /// The label, the level's left indent and first-line indent, and whether the level
    /// uses label-alignment mode (indents replace the paragraph's) rather than the older
    /// position mode (indents add to them).
    fn label(&self, list: &ListCtx, tp: &Tp) -> Option<(Label, f32, f32, bool)> {
        let ls = self.st.lists.get(&list.style)?;
        let lvl = ls.children().find(|l| {
            l.attr("level").and_then(|v| v.parse::<usize>().ok()) == Some(list.level + 1)
        })?;
        let props = lvl.child("list-level-properties");
        let align = props.and_then(|p| p.child("list-level-label-alignment"));
        let (left, first, suffix) = match align {
            Some(a) => (
                a.attr("margin-left").and_then(len).unwrap_or(0.0),
                a.attr("text-indent").and_then(len).unwrap_or(0.0),
                match a.attr("label-followed-by") {
                    Some("space") => Suffix::Space,
                    Some("nothing") => Suffix::Nothing,
                    _ => Suffix::Tab,
                },
            ),
            None => {
                let before = props
                    .and_then(|p| p.attr("space-before"))
                    .and_then(len)
                    .unwrap_or(0.0);
                let min = props
                    .and_then(|p| p.attr("min-label-width"))
                    .and_then(len)
                    .unwrap_or(18.0);
                (before + min, -min, Suffix::Tab)
            }
        };
        let mut ltp = tp.clone();
        if let Some(t) = lvl.child("text-properties") {
            ltp.over(&Tp::parse(t, &self.st.fonts));
        }
        if let Some(sn) = lvl.attr("text-style-name") {
            ltp = self.st.text(sn, &ltp);
        }
        let text = match &*lvl.name {
            "list-level-style-bullet" => lvl.attr("bullet-char").unwrap_or("•").to_string(),
            "list-level-style-number" => {
                let fmt = match lvl.attr("num-format") {
                    Some("a") => NumFmt::LowerLetter,
                    Some("A") => NumFmt::UpperLetter,
                    Some("i") => NumFmt::LowerRoman,
                    Some("I") => NumFmt::UpperRoman,
                    Some("") => NumFmt::None,
                    _ => NumFmt::Decimal,
                };
                if fmt == NumFmt::None {
                    return None;
                }
                let start = lvl
                    .attr("start-value")
                    .and_then(|v| v.parse::<u32>().ok())
                    .unwrap_or(1);
                let display = lvl
                    .attr("display-levels")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                let mut parts = Vec::new();
                for l in (list.level + 1).saturating_sub(display)..=list.level {
                    let n = self
                        .counters
                        .get(&(list.style.clone(), l))
                        .copied()
                        .unwrap_or(1);
                    parts.push(format_number(
                        n + start - 1,
                        if l == list.level {
                            fmt
                        } else {
                            NumFmt::Decimal
                        },
                    ));
                }
                format!(
                    "{}{}{}",
                    lvl.attr("num-prefix").unwrap_or(""),
                    parts.join("."),
                    lvl.attr("num-suffix").unwrap_or("")
                )
            }
            _ => return None,
        };
        Some((
            Label {
                text,
                rs: Arc::new(run_style(&ltp)),
                suffix,
                jc: Align::Left,
            },
            left,
            first,
            align.is_some(),
        ))
    }

    fn para(&mut self, el: &El, list: Option<&ListCtx>) -> Para {
        let style = el.attr("style-name");
        let (pp, tp) = self.st.para(style);
        let mut ps = para_style(&pp, &self.st.named(style));
        let mark = Arc::new(run_style(&tp));
        let mut items = Vec::new();
        self.inline(el, &tp, None, &mut items, &mut true);
        // Lists: the paragraph's own list style, or the enclosing list's.
        let own = self.st.para_list(style);
        let mut label = None;
        let in_list = list.is_some();
        let list = list.cloned().or_else(|| {
            own.map(|s| ListCtx {
                style: s,
                level: 0,
                first: true,
            })
        });
        if let Some(l) = &list
            && let Some((lab, left, first, aligned)) = self.label(l, &tp)
        {
            if !aligned {
                // The older position mode: the label sits inside the paragraph's indent.
                ps.left += left;
                ps.first = first;
            } else if self.st.list_indents_apply(style, in_list) {
                ps.left = left;
                ps.first = first;
            }
            if ps.tabs.iter().all(|t| t.pos != ps.left) {
                ps.tabs.push(TabDef {
                    pos: ps.left,
                    kind: TabKind::Left,
                    leader: Leader::None,
                });
            }
            if l.first {
                label = Some(lab);
            }
        }
        let outline = (&*el.name == "h").then(|| {
            el.attr("outline-level")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(1)
                .saturating_sub(1)
        });
        Para {
            p: ps,
            mark,
            items,
            label,
            outline,
        }
    }

    /// Inline content. `space` tracks whether the last character was a space (ODF
    /// collapses runs of spaces outside `text:s`).
    fn inline(
        &mut self,
        el: &El,
        tp: &Tp,
        link: Option<&Arc<Link>>,
        items: &mut Vec<Item>,
        space: &mut bool,
    ) {
        let rs = Arc::new(run_style(tp));
        for n in &el.kids {
            match n {
                Node::Text(t) => {
                    let mut s = String::with_capacity(t.len());
                    for c in t.chars() {
                        let c = if matches!(c, '\n' | '\r' | '\t') {
                            ' '
                        } else {
                            c
                        };
                        if c == ' ' {
                            if *space {
                                continue;
                            }
                            *space = true;
                        } else {
                            *space = false;
                        }
                        s.push(c);
                    }
                    if !s.is_empty() {
                        items.push(Item::Text {
                            text: s,
                            rs: rs.clone(),
                            link: link.cloned(),
                        });
                    }
                }
                Node::El(e) => match &*e.name {
                    "span" => {
                        let t = e
                            .attr("style-name")
                            .map_or_else(|| tp.clone(), |n| self.st.text(n, tp));
                        self.inline(e, &t, link, items, space);
                    }
                    "a" => {
                        let l = e.attr("href").map(|h| {
                            Arc::new(if let Some(a) = h.strip_prefix('#') {
                                Link::Anchor(a.to_string())
                            } else {
                                Link::Url(h.to_string())
                            })
                        });
                        let t = e
                            .attr("style-name")
                            .map_or_else(|| tp.clone(), |n| self.st.text(n, tp));
                        self.inline(e, &t, l.as_ref().or(link), items, space);
                    }
                    "s" => {
                        let c = e
                            .attr("c")
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(1)
                            .min(200);
                        items.push(Item::Text {
                            text: " ".repeat(c),
                            rs: rs.clone(),
                            link: link.cloned(),
                        });
                        *space = true;
                    }
                    "tab" => {
                        items.push(Item::Tab { rs: rs.clone() });
                        *space = false;
                    }
                    "line-break" => {
                        items.push(Item::Break(BreakKind::Line));
                        *space = false;
                    }
                    "page-number" => items.push(Item::Field {
                        kind: FieldKind::Page,
                        fmt: None,
                        rs: rs.clone(),
                    }),
                    "page-count" => {
                        self.uses_numpages = true;
                        items.push(Item::Field {
                            kind: FieldKind::NumPages,
                            fmt: None,
                            rs: rs.clone(),
                        });
                    }
                    "bookmark" | "bookmark-start" => {
                        if let Some(n) = e.attr("name") {
                            items.push(Item::Bookmark(n.to_string()));
                        }
                    }
                    "note" => {
                        self.notes += 1;
                        let id = format!("odt{}", self.notes);
                        let endnote = e.attr("note-class") == Some("endnote");
                        let mut body = e
                            .child("note-body")
                            .map(|b| self.blocks(b, None))
                            .unwrap_or_default();
                        if let Some(Block::Para(p)) = body.first_mut() {
                            let mut sup = (*p.mark).clone();
                            sup.vert = Vert::Super;
                            p.items.insert(0, Item::NoteMark { rs: Arc::new(sup) });
                        }
                        if endnote {
                            self.endnotes.insert(id.clone(), body);
                        } else {
                            self.footnotes.insert(id.clone(), body);
                        }
                        let mut sup = (*rs).clone();
                        sup.vert = Vert::Super;
                        items.push(Item::NoteRef {
                            id,
                            endnote,
                            rs: Arc::new(sup),
                        });
                    }
                    // Frames, and drawing shapes with their text (a Word text box saved
                    // by LibreOffice is a custom shape).
                    "frame" | "custom-shape" | "rect" | "ellipse" => {
                        if let Some(d) = self.frame(e) {
                            items.push(Item::Drawing(Box::new(d)));
                        }
                    }
                    "soft-page-break" | "bookmark-end" | "reference-mark-end" | "annotation"
                    | "annotation-end" | "change-start" | "change-end" | "change" => {}
                    // Fields and other inline elements: their text.
                    _ => self.inline(e, tp, link, items, space),
                },
            }
        }
    }

    fn picture(&mut self, href: &str) -> Option<Arc<Media>> {
        if href.contains("://") || href.starts_with('/') || href.starts_with("..") {
            // Linked files are never loaded (docs/16 §6a).
            return None;
        }
        if let Some(m) = self.media.get(href) {
            return m.clone();
        }
        let m = self
            .pkg
            .bytes(href)
            .filter(|b| crate::emf::drawable(b))
            .map(|b| Arc::new(Media { bytes: Arc::new(b) }));
        self.media.insert(href.to_string(), m.clone());
        m
    }

    fn frame(&mut self, e: &El) -> Option<Drawing> {
        let w = e.attr("width").and_then(len).unwrap_or(72.0);
        let h = e
            .attr("height")
            .and_then(len)
            .or_else(|| e.attr("min-height").and_then(len))
            .unwrap_or(36.0);
        let style = self.st.get("graphic", e.attr("style-name"));
        let g = style.and_then(|s| s.graphic.clone());
        let ga = |k: &str| g.as_ref().and_then(|g| g.attr(k).map(str::to_string));
        let shape = matches!(&*e.name, "custom-shape" | "rect" | "ellipse");
        let kind = if shape {
            self.depth += 1;
            let text = self.blocks(e, None);
            self.depth -= 1;
            let geom = match e
                .child("enhanced-geometry")
                .and_then(|g| g.attr("type"))
                .unwrap_or(if &*e.name == "ellipse" {
                    "ellipse"
                } else {
                    "rect"
                }) {
                "ellipse" | "ooxml-ellipse" => Geom::Ellipse,
                "round-rectangle" | "ooxml-roundRect" => Geom::RoundRect,
                _ => Geom::Rect,
            };
            let pad = |k: &str| ga(k).and_then(|p| len(&p));
            DrawKind::Shape {
                geom,
                fill: (ga("fill").as_deref() != Some("none"))
                    .then(|| ga("fill-color").and_then(|c| color(&c)))
                    .flatten(),
                line: (ga("stroke").as_deref() != Some("none"))
                    .then(|| {
                        let c = ga("stroke-color").and_then(|c| color(&c))?;
                        let w = ga("stroke-width").and_then(|w| len(&w)).unwrap_or(0.75);
                        Some((c, w.max(0.25)))
                    })
                    .flatten(),
                text,
                // Word's text box insets unless the style says otherwise.
                ins: [
                    pad("padding-left").unwrap_or(7.2),
                    pad("padding-top").unwrap_or(3.6),
                    pad("padding-right").unwrap_or(7.2),
                    pad("padding-bottom").unwrap_or(3.6),
                ],
                valign: match ga("textarea-vertical-align").as_deref() {
                    Some("middle") => VAlign::Center,
                    Some("bottom") => VAlign::Bottom,
                    _ => VAlign::Top,
                },
            }
        } else if let Some(img) = e.child("image") {
            DrawKind::Picture {
                media: img.attr("href").and_then(|h| self.picture(h)),
                crop: [0.0; 4],
            }
        } else if let Some(tb) = e.child("text-box") {
            self.depth += 1;
            let text = self.blocks(tb, None);
            self.depth -= 1;
            let pad = ga("padding").and_then(|p| len(&p)).unwrap_or(0.0);
            DrawKind::Shape {
                geom: Geom::Rect,
                fill: (ga("fill").as_deref() == Some("solid"))
                    .then(|| ga("fill-color").and_then(|c| color(&c)))
                    .flatten()
                    .or_else(|| ga("background-color").and_then(|c| color(&c))),
                line: ga("border")
                    .and_then(|b| border(&b))
                    .map(|b| (b.color, b.width)),
                text,
                ins: [pad; 4],
                valign: match ga("textarea-vertical-align").as_deref() {
                    Some("middle") => VAlign::Center,
                    Some("bottom") => VAlign::Bottom,
                    _ => VAlign::Top,
                },
            }
        } else {
            DrawKind::Placeholder
        };
        let anchor = match e.attr("anchor-type") {
            Some("as-char") | None => None,
            Some(t) => {
                let page = t == "page";
                let hrel = ga("horizontal-rel").unwrap_or_default();
                let vrel = ga("vertical-rel").unwrap_or_default();
                // `page-content` is the area between the margins; plain `page` the sheet.
                let h_rel = match hrel.as_str() {
                    "page-content" => HRel::Margin,
                    "page-start-margin" => HRel::LeftMargin,
                    "page-end-margin" => HRel::RightMargin,
                    "char" => HRel::Character,
                    r if page || r.starts_with("page") => HRel::Page,
                    _ => HRel::Column,
                };
                let v_rel = match vrel.as_str() {
                    "page-content" => VRel::Margin,
                    r if page || r.starts_with("page") => VRel::Page,
                    _ => VRel::Paragraph,
                };
                let h = match ga("horizontal-pos").as_deref() {
                    Some("center") => Pos::Align(1),
                    Some("right") | Some("outside") => Pos::Align(2),
                    Some("left") | Some("inside") => Pos::Align(0),
                    _ => Pos::Offset(e.attr("x").and_then(len).unwrap_or(0.0)),
                };
                let v = match ga("vertical-pos").as_deref() {
                    Some("middle") | Some("center") => Pos::Align(1),
                    Some("bottom") => Pos::Align(2),
                    Some("top") => Pos::Align(0),
                    _ => Pos::Offset(e.attr("y").and_then(len).unwrap_or(0.0)),
                };
                let wrap = match ga("wrap").as_deref() {
                    Some("none") => Wrap::TopBottom,
                    Some("run-through") => Wrap::None,
                    _ => Wrap::Square,
                };
                Some(Anchor {
                    h_rel,
                    h,
                    v_rel,
                    v,
                    wrap,
                    behind: ga("run-through").as_deref() == Some("background"),
                    dist: [
                        ga("margin-top").and_then(|v| len(&v)).unwrap_or(0.0),
                        ga("margin-bottom").and_then(|v| len(&v)).unwrap_or(0.0),
                        ga("margin-left").and_then(|v| len(&v)).unwrap_or(0.0),
                        ga("margin-right").and_then(|v| len(&v)).unwrap_or(0.0),
                    ],
                })
            }
        };
        // svg:desc, else svg:title, as children of the frame.
        let alt = ["desc", "title"].iter().find_map(|k| {
            e.children()
                .find(|c| &*c.name == *k)
                .map(|c| c.text().trim().to_string())
                .filter(|t| !t.is_empty())
        });
        Some(Drawing {
            w,
            h,
            kind,
            anchor,
            alt,
        })
    }

    fn table(&mut self, el: &El) -> Option<Table> {
        let tstyle = self
            .st
            .get("table", el.attr("style-name"))
            .and_then(|s| s.table.clone());
        let ta = |k: &str| tstyle.as_ref().and_then(|t| t.attr(k));
        // Columns (repeated ones expanded).
        let mut grid = Vec::new();
        let mut col_styles: Vec<Option<String>> = Vec::new();
        let mut col_els: Vec<&El> = el.all("table-column").collect();
        if let Some(g) = el.child("table-columns") {
            col_els.extend(g.all("table-column"));
        }
        for c in col_els {
            let n = c
                .attr("number-columns-repeated")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .min(512);
            let w = self
                .st
                .get("table-column", c.attr("style-name"))
                .and_then(|s| s.column.as_ref())
                .and_then(|p| p.attr("column-width"))
                .and_then(len)
                .unwrap_or(72.0);
            for _ in 0..n {
                grid.push(w);
                col_styles.push(c.attr("default-cell-style-name").map(str::to_string));
            }
        }
        // Rows: header rows first.
        let mut row_els: Vec<(&El, bool)> = Vec::new();
        for c in el.children() {
            match &*c.name {
                "table-header-rows" => row_els.extend(c.all("table-row").map(|r| (r, true))),
                "table-row" => row_els.push((c, false)),
                "table-rows" => row_els.extend(c.all("table-row").map(|r| (r, false))),
                _ => {}
            }
        }
        if grid.is_empty() || row_els.is_empty() {
            return None;
        }
        // Vertical merges: column → rows still covered.
        let mut covering: HashMap<usize, (usize, usize)> = HashMap::new();
        let mut rows = Vec::new();
        for (r, header) in row_els {
            let height = self
                .st
                .get("table-row", r.attr("style-name"))
                .and_then(|s| s.row.as_ref())
                .and_then(|p| {
                    p.attr("row-height")
                        .and_then(len)
                        .map(|h| (h, true))
                        .or_else(|| p.attr("min-row-height").and_then(len).map(|h| (h, false)))
                });
            let mut cells = Vec::new();
            // Each cell element, covered or not, is one grid column. A spanning cell is
            // followed by covered cells for the columns it takes (`hcover` is where its
            // span ends); a writer that leaves those out is handled too.
            let mut col = 0usize;
            let mut hcover = 0usize;
            for c in r.children() {
                let rep = c
                    .attr("number-columns-repeated")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .min(512);
                match &*c.name {
                    "covered-table-cell" => {
                        for _ in 0..rep {
                            if col < hcover {
                                col += 1;
                                continue;
                            }
                            if let Some((span, left)) = covering.get(&col).copied() {
                                cells.push(Cell {
                                    col,
                                    span,
                                    vmerge: Some(VMerge::Continue),
                                    width: None,
                                    borders: [None; 4],
                                    shd: None,
                                    valign: VAlign::Top,
                                    mar: [2.0; 4],
                                    blocks: Vec::new(),
                                });
                                if left <= 1 {
                                    covering.remove(&col);
                                } else {
                                    covering.insert(col, (span, left - 1));
                                }
                                hcover = col + span;
                            }
                            col += 1;
                        }
                    }
                    "table-cell" => {
                        for _ in 0..rep {
                            col = col.max(hcover);
                            let span = c
                                .attr("number-columns-spanned")
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(1)
                                .max(1);
                            let rspan = c
                                .attr("number-rows-spanned")
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(1)
                                .max(1);
                            let sname = c
                                .attr("style-name")
                                .map(str::to_string)
                                .or_else(|| col_styles.get(col).cloned().flatten());
                            let cp = self
                                .st
                                .get("table-cell", sname.as_deref())
                                .and_then(|s| s.cell.clone());
                            let ca = |k: &str| cp.as_ref().and_then(|p| p.attr(k));
                            let all = ca("border").and_then(border);
                            let side = |k: &str| ca(k).map(border).unwrap_or(all);
                            let pad_all = ca("padding").and_then(len).unwrap_or(2.0);
                            let pad = |k: &str| ca(k).and_then(len).unwrap_or(pad_all);
                            self.depth += 1;
                            let blocks = self.blocks(c, None);
                            self.depth -= 1;
                            if rspan > 1 {
                                covering.insert(col, (span, rspan - 1));
                            }
                            cells.push(Cell {
                                col,
                                span,
                                vmerge: (rspan > 1).then_some(VMerge::Restart),
                                width: None,
                                borders: [
                                    side("border-top"),
                                    side("border-left"),
                                    side("border-bottom"),
                                    side("border-right"),
                                ],
                                shd: ca("background-color").and_then(color),
                                valign: match ca("vertical-align") {
                                    Some("middle") => VAlign::Center,
                                    Some("bottom") => VAlign::Bottom,
                                    _ => VAlign::Top,
                                },
                                mar: [
                                    pad("padding-top"),
                                    pad("padding-left"),
                                    pad("padding-bottom"),
                                    pad("padding-right"),
                                ],
                                blocks,
                            });
                            hcover = col + span;
                            col += 1;
                        }
                    }
                    _ => {}
                }
            }
            rows.push(Row {
                height,
                header,
                cant_split: false,
                cells,
            });
        }
        Some(Table {
            align: match ta("align") {
                Some("center") => Align::Center,
                Some("right") => Align::Right,
                _ => Align::Left,
            },
            indent: ta("margin-left").and_then(len).unwrap_or(0.0),
            width: ta("width").and_then(len).map(Width::Pt),
            grid,
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths() {
        assert_eq!(len("1in"), Some(72.0));
        assert!((len("2.54cm").unwrap() - 72.0).abs() < 0.01);
        assert_eq!(len("12pt"), Some(12.0));
        assert_eq!(
            border("0.5pt solid #ff0000").map(|b| b.color),
            Some(Rgb(255, 0, 0))
        );
        assert!(border("none").is_none());
    }

    fn odt(content: &str, styles: &str) -> Vec<u8> {
        use std::io::Write as _;
        let ns = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0""#;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("mimetype", o).unwrap();
            z.write_all(b"application/vnd.oasis.opendocument.text")
                .unwrap();
            z.start_file("content.xml", o).unwrap();
            z.write_all(
                format!(r#"<office:document-content {ns}>{content}</office:document-content>"#)
                    .as_bytes(),
            )
            .unwrap();
            z.start_file("styles.xml", o).unwrap();
            z.write_all(
                format!(r#"<office:document-styles {ns}>{styles}</office:document-styles>"#)
                    .as_bytes(),
            )
            .unwrap();
            z.finish().unwrap();
        }
        buf
    }

    /// Found on LibreOffice exports: a title page ("First Page" master handing over to
    /// "Standard"), a two-column text section, covered cells after a column span, and a
    /// Word text box saved as a custom shape.
    #[test]
    fn first_page_columns_spans_and_shapes() {
        let styles = r#"<office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-left="2cm" fo:margin-right="2cm"/></style:page-layout></office:automatic-styles>
<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>Running head</text:p></style:header></style:master-page><style:master-page style:name="First" style:page-layout-name="pm1" style:next-style-name="Standard"/></office:master-styles>"#;
        let content = r##"<office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:master-page-name="First"/><style:style style:name="S2" style:family="section"><style:section-properties><style:columns fo:column-count="2" fo:column-gap="1cm"/></style:section-properties></style:style><style:style style:name="gr1" style:family="graphic"><style:graphic-properties draw:fill="solid" draw:fill-color="#1f3a5f" draw:stroke="none"/></style:style></office:automatic-styles>
<office:body><office:text><text:section text:name="All"><text:p text:style-name="P1">Title<draw:custom-shape text:anchor-type="char" draw:style-name="gr1" svg:width="6cm" svg:height="10cm" svg:x="0cm" svg:y="0cm"><text:p>Sidebar</text:p><draw:enhanced-geometry draw:type="ooxml-rect"/></draw:custom-shape></text:p>
<table:table><table:table-column table:number-columns-repeated="4"/><table:table-row><table:table-cell table:number-columns-spanned="2"><text:p>AB</text:p></table:table-cell><table:covered-table-cell/><table:table-cell><text:p>C</text:p></table:table-cell><table:table-cell><text:p>D</text:p></table:table-cell></table:table-row></table:table>
<text:section text:style-name="S2" text:name="Two"><text:p>In two columns</text:p></text:section><text:p>After</text:p></text:section></office:text></office:body>"##;
        let doc = read(&odt(content, styles)).unwrap();
        let s = &doc.sections;
        assert_eq!(s.len(), 3, "title section, two columns, back to one");
        assert!(
            s[0].sect.title_pg && s[0].sect.headers[1].is_none() && s[0].sect.headers[0].is_some()
        );
        assert_eq!(
            (s[1].sect.cols.cols.len(), s[1].sect.kind),
            (2, SectKind::Continuous)
        );
        assert_eq!(s[2].sect.cols.cols.len(), 1);
        let Block::Table(t) = &s[0].blocks[1] else {
            panic!("a table")
        };
        let cols: Vec<(usize, usize)> = t.rows[0].cells.iter().map(|c| (c.col, c.span)).collect();
        assert_eq!(cols, [(0, 2), (2, 1), (3, 1)]);
        let Block::Para(p) = &s[0].blocks[0] else {
            panic!("a paragraph")
        };
        assert!(p.items.iter().any(|i| matches!(i, Item::Drawing(d) if matches!(&d.kind, DrawKind::Shape { fill: Some(_), text, .. } if !text.is_empty()))), "the sidebar is a filled shape with its text");
    }

    /// Found on the `wordlike` export, which came out a page longer than LibreOffice's PDF:
    /// list levels in label-alignment mode replace the "List Paragraph" indent instead of
    /// adding to it, contextual spacing removes the gap between list items, a header's
    /// `min-height` already includes its spacing, `page-content` frames are placed from
    /// the margins, and a left border stays on the left.
    #[test]
    fn list_indents_spacing_header_room_and_frames() {
        let styles = r##"<office:styles><style:style style:name="List_20_Paragraph" style:family="paragraph"><style:paragraph-properties fo:margin-left="1.27cm" fo:margin-top="0cm" fo:margin-bottom="0.282cm" style:contextual-spacing="true" fo:border-left="1pt solid #4472c4"/></style:style>
<text:list-style style:name="L1"><text:list-level-style-number text:level="1" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.635cm" fo:margin-left="0.635cm"/></style:list-level-properties></text:list-level-style-number></text:list-style></office:styles>
<office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-top="1.27cm" fo:margin-bottom="1.27cm" fo:margin-left="2cm" fo:margin-right="2cm"/><style:header-style><style:header-footer-properties fo:min-height="1.27cm" fo:margin-bottom="1.169cm" style:dynamic-spacing="true"/></style:header-style></style:page-layout></office:automatic-styles>
<office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"><style:header><text:p>Head</text:p></style:header></style:master-page></office:master-styles>"##;
        let content = r##"<office:automatic-styles><style:style style:name="P1" style:family="paragraph" style:parent-style-name="List_20_Paragraph" style:list-style-name="L1"/><style:style style:name="P2" style:family="paragraph" style:parent-style-name="List_20_Paragraph"><style:paragraph-properties fo:margin-left="3cm"/></style:style><style:style style:name="fr1" style:family="graphic"><style:graphic-properties style:wrap="parallel" style:horizontal-pos="right" style:horizontal-rel="page-content" style:vertical-pos="from-top" style:vertical-rel="paragraph"/></style:style></office:automatic-styles>
<office:body><office:text><text:list text:style-name="L1"><text:list-item><text:p text:style-name="P1">One</text:p></text:list-item><text:list-item><text:p text:style-name="P1">Two</text:p></text:list-item><text:list-item><text:p text:style-name="P2">Own indent</text:p></text:list-item></text:list>
<text:p><draw:frame text:anchor-type="char" draw:style-name="fr1" svg:width="4cm" svg:height="2cm"><draw:text-box><text:p>Box</text:p></draw:text-box></draw:frame>After</text:p></office:text></office:body>"##;
        let doc = read(&odt(content, styles)).unwrap();
        let s = &doc.sections[0];
        let cm = |v: f32| v * 72.0 / 2.54;
        assert!((s.sect.header - cm(1.27)).abs() < 0.1, "{}", s.sect.header);
        assert!(
            (s.sect.top - cm(2.54)).abs() < 0.1,
            "the body starts at margin + min-height, not + spacing again: {}",
            s.sect.top
        );
        let paras: Vec<&Para> = s
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Para(p) => Some(&**p),
                _ => None,
            })
            .collect();
        let (one, two, own) = (&paras[0].p, &paras[1].p, &paras[2].p);
        assert!((one.left - cm(0.635)).abs() < 0.1, "{}", one.left);
        assert!((one.first + cm(0.635)).abs() < 0.1, "{}", one.first);
        assert!(one.contextual && one.style == two.style);
        assert_eq!(one.style, "List_20_Paragraph");
        assert!(
            (own.left - cm(3.0)).abs() < 0.1,
            "hard-formatted indents win: {}",
            own.left
        );
        assert!(one.borders[2].is_some() && one.borders[1].is_none());
        let anchor = paras[3].items.iter().find_map(|i| match i {
            Item::Drawing(d) => d.anchor.as_ref(),
            _ => None,
        });
        assert_eq!(
            anchor.map(|a| (a.h_rel, a.h)),
            Some((HRel::Margin, Pos::Align(2)))
        );
    }
}
