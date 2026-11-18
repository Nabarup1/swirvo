//! WordprocessingML properties as written (every field optional), their parsers, and the
//! merge rule styles use: a later layer overrides only what it sets.
//!
//! Units are converted on read: lengths are points (`f32`), colors are sRGB.

use super::theme::Theme;
use super::xml::El;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

pub const BLACK: Rgb = Rgb(0, 0, 0);

impl Rgb {
    pub fn hex(s: &str) -> Option<Rgb> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(s, 16).ok()?;
        Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
    }

    /// DrawingML/Word tint (towards white) and shade (towards black), as 0..=255 factors.
    pub fn tint(self, f: f32) -> Rgb {
        let t = |c: u8| (c as f32 * f + 255.0 * (1.0 - f)).round().clamp(0.0, 255.0) as u8;
        Rgb(t(self.0), t(self.1), t(self.2))
    }

    pub fn shade(self, f: f32) -> Rgb {
        let s = |c: u8| (c as f32 * f).round().clamp(0.0, 255.0) as u8;
        Rgb(s(self.0), s(self.1), s(self.2))
    }
}

/// Twips (1/20 pt) or an ISO universal measure ("2.5cm", "1in", "12pt").
pub fn twips(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Ok(v) = s.parse::<f32>() {
        return Some(v / 20.0);
    }
    universal(s)
}

fn universal(s: &str) -> Option<f32> {
    let split = s.find(|c: char| c.is_ascii_alphabetic())?;
    let (n, unit) = s.split_at(split);
    let n: f32 = n.trim().parse().ok()?;
    Some(match unit {
        "pt" => n,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "pc" | "pi" => n * 12.0,
        _ => return None,
    })
}

/// English Metric Units: 12 700 per point.
pub fn emu(s: &str) -> Option<f32> {
    s.trim()
        .parse::<f64>()
        .ok()
        .map(|v| (v / 12_700.0) as f32)
        .or_else(|| universal(s))
}

fn half_points(s: &str) -> Option<f32> {
    s.trim()
        .parse::<f32>()
        .ok()
        .map(|v| v / 2.0)
        .or_else(|| universal(s))
}

fn color_of(e: &El, theme: &Theme) -> Option<Option<Rgb>> {
    let tinted = |c: Rgb| {
        let mut c = c;
        if let Some(t) = e
            .attr("themeTint")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.tint(t as f32 / 255.0);
        }
        if let Some(s) = e
            .attr("themeShade")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.shade(s as f32 / 255.0);
        }
        c
    };
    if let Some(t) = e.attr("themeColor")
        && let Some(c) = theme.color(t)
    {
        return Some(Some(tinted(c)));
    }
    match e.val() {
        Some("auto") => Some(None),
        Some(v) => Rgb::hex(v).map(Some),
        None => None,
    }
}

/// `w:shd`: the fill wins; a solid pattern means the pattern color.
fn shading(e: &El, theme: &Theme) -> Option<Option<Rgb>> {
    let val = e.val().unwrap_or("clear");
    if val == "solid" {
        return match e.attr("color") {
            Some("auto") | None => Some(Some(BLACK)),
            Some(c) => Some(Rgb::hex(c)),
        };
    }
    if let Some(t) = e.attr("themeFill")
        && let Some(c) = theme.color(t)
    {
        let mut c = c;
        if let Some(v) = e
            .attr("themeFillTint")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.tint(v as f32 / 255.0);
        }
        if let Some(v) = e
            .attr("themeFillShade")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.shade(v as f32 / 255.0);
        }
        return Some(Some(c));
    }
    match e.attr("fill") {
        Some("auto") => Some(None),
        Some(f) => Some(Rgb::hex(f)),
        None if val == "nil" => Some(None),
        None => None,
    }
}

/// A border's color: `w:themeColor` (with tint/shade), else `w:color`; "auto" is black.
fn border_color(e: &El, theme: &Theme) -> Option<Rgb> {
    if let Some(t) = e.attr("themeColor")
        && let Some(mut c) = theme.color(t)
    {
        if let Some(v) = e
            .attr("themeTint")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.tint(v as f32 / 255.0);
        }
        if let Some(v) = e
            .attr("themeShade")
            .and_then(|v| u8::from_str_radix(v, 16).ok())
        {
            c = c.shade(v as f32 / 255.0);
        }
        return Some(c);
    }
    e.attr("color").and_then(Rgb::hex)
}

fn highlight(name: &str) -> Option<Rgb> {
    Some(match name {
        "yellow" => Rgb(255, 255, 0),
        "green" => Rgb(0, 255, 0),
        "cyan" => Rgb(0, 255, 255),
        "magenta" => Rgb(255, 0, 255),
        "blue" => Rgb(0, 0, 255),
        "red" => Rgb(255, 0, 0),
        "darkBlue" => Rgb(0, 0, 139),
        "darkCyan" => Rgb(0, 139, 139),
        "darkGreen" => Rgb(0, 100, 0),
        "darkMagenta" => Rgb(139, 0, 139),
        "darkRed" => Rgb(139, 0, 0),
        "darkYellow" => Rgb(128, 128, 0),
        "darkGray" => Rgb(169, 169, 169),
        "lightGray" => Rgb(211, 211, 211),
        "black" => BLACK,
        "white" => Rgb(255, 255, 255),
        _ => return None,
    })
}

// ---------------------------------------------------------------- run properties

#[derive(Debug, Clone, PartialEq)]
pub enum FontRef {
    Name(String),
    Theme(ThemeFont),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeFont {
    MajorLatin,
    MinorLatin,
    MajorEa,
    MinorEa,
    MajorCs,
    MinorCs,
}

fn theme_font(v: &str) -> Option<ThemeFont> {
    Some(match v {
        "majorHAnsi" | "majorAscii" => ThemeFont::MajorLatin,
        "minorHAnsi" | "minorAscii" => ThemeFont::MinorLatin,
        "majorEastAsia" => ThemeFont::MajorEa,
        "minorEastAsia" => ThemeFont::MinorEa,
        "majorBidi" => ThemeFont::MajorCs,
        "minorBidi" => ThemeFont::MinorCs,
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Underline {
    #[default]
    None,
    Single,
    Words,
    Double,
    Thick,
    Dotted,
    Dash,
    Wave,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Vert {
    #[default]
    Baseline,
    Super,
    Sub,
}

#[derive(Debug, Clone, Default)]
pub struct RunProps {
    pub style: Option<String>,
    pub ascii: Option<FontRef>,
    pub hansi: Option<FontRef>,
    pub ea: Option<FontRef>,
    pub cs_font: Option<FontRef>,
    pub b: Option<bool>,
    pub i: Option<bool>,
    pub b_cs: Option<bool>,
    pub i_cs: Option<bool>,
    pub sz: Option<f32>,
    pub sz_cs: Option<f32>,
    pub color: Option<Option<Rgb>>,
    pub u: Option<Underline>,
    pub u_color: Option<Option<Rgb>>,
    pub strike: Option<bool>,
    pub dstrike: Option<bool>,
    pub caps: Option<bool>,
    pub small_caps: Option<bool>,
    pub vanish: Option<bool>,
    pub vert: Option<Vert>,
    pub position: Option<f32>,
    pub spacing: Option<f32>,
    pub scale: Option<f32>,
    pub kern: Option<f32>,
    pub highlight: Option<Option<Rgb>>,
    pub shd: Option<Option<Rgb>>,
    pub bdr: Option<Option<Border>>,
    pub rtl: Option<bool>,
    pub cs: Option<bool>,
}

macro_rules! over {
    ($dst:ident, $src:ident; $($f:ident),* $(,)?) => {
        $( if $src.$f.is_some() { $dst.$f = $src.$f.clone(); } )*
    };
}

impl RunProps {
    pub fn parse(e: &El, theme: &Theme) -> RunProps {
        let mut r = RunProps::default();
        for c in e.children() {
            match &*c.name {
                "rStyle" => r.style = c.val().map(str::to_string),
                "rFonts" => {
                    let pick = |name: &str, theme_attr: &str| {
                        c.attr(theme_attr)
                            .and_then(theme_font)
                            .map(FontRef::Theme)
                            .or_else(|| c.attr(name).map(|v| FontRef::Name(v.to_string())))
                    };
                    r.ascii = pick("ascii", "asciiTheme").or(r.ascii);
                    r.hansi = pick("hAnsi", "hAnsiTheme").or(r.hansi);
                    r.ea = pick("eastAsia", "eastAsiaTheme").or(r.ea);
                    r.cs_font = pick("cs", "cstheme").or(r.cs_font);
                }
                "b" => r.b = Some(c.on()),
                "i" => r.i = Some(c.on()),
                "bCs" => r.b_cs = Some(c.on()),
                "iCs" => r.i_cs = Some(c.on()),
                "sz" => r.sz = c.val().and_then(half_points),
                "szCs" => r.sz_cs = c.val().and_then(half_points),
                "color" => r.color = color_of(c, theme),
                "u" => {
                    r.u = Some(match c.val().unwrap_or("single") {
                        "none" | "0" => Underline::None,
                        "words" => Underline::Words,
                        "double" => Underline::Double,
                        "thick" => Underline::Thick,
                        v if v.contains("otted") || v == "dotDash" || v == "dotDotDash" => {
                            Underline::Dotted
                        }
                        v if v.contains("ash") => Underline::Dash,
                        v if v.contains("ave") => Underline::Wave,
                        _ => Underline::Single,
                    });
                    if c.attr("color").is_some() {
                        r.u_color = match c.attr("color") {
                            Some("auto") => Some(None),
                            Some(v) => Some(Rgb::hex(v)),
                            None => None,
                        };
                    }
                }
                "strike" => r.strike = Some(c.on()),
                "dstrike" => r.dstrike = Some(c.on()),
                "caps" => r.caps = Some(c.on()),
                "smallCaps" => r.small_caps = Some(c.on()),
                "vanish" | "specVanish" => r.vanish = Some(c.on()),
                "vertAlign" => {
                    r.vert = Some(match c.val() {
                        Some("superscript") => Vert::Super,
                        Some("subscript") => Vert::Sub,
                        _ => Vert::Baseline,
                    })
                }
                "position" => r.position = c.val().and_then(half_points),
                "spacing" => r.spacing = c.val().and_then(twips),
                "w" => {
                    r.scale = c
                        .val()
                        .and_then(|v| v.trim_end_matches('%').parse::<f32>().ok())
                }
                "kern" => r.kern = c.val().and_then(half_points),
                "highlight" => r.highlight = Some(c.val().and_then(highlight)),
                "shd" => r.shd = shading(c, theme),
                "bdr" => {
                    let b = Border::parse(c, theme);
                    r.bdr = Some(Some(b).filter(Border::visible));
                }
                "rtl" => r.rtl = Some(c.on()),
                "cs" => r.cs = Some(c.on()),
                _ => {}
            }
        }
        r
    }

    /// Lay `o` over `self`.
    pub fn apply(&mut self, o: &RunProps) {
        over!(self, o; ascii, hansi, ea, cs_font, b, i, b_cs, i_cs, sz, sz_cs, color, u, u_color,
            strike, dstrike, caps, small_caps, vanish, vert, position, spacing, scale, kern,
            highlight, shd, bdr, rtl, cs);
    }
}

// ---------------------------------------------------------------- paragraph properties

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Both,
    Distribute,
}

fn align(v: &str) -> Align {
    match v {
        "center" => Align::Center,
        "right" | "end" => Align::Right,
        "both" | "lowKashida" | "mediumKashida" | "highKashida" | "thaiDistribute" => Align::Both,
        "distribute" => Align::Distribute,
        _ => Align::Left,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineRule {
    /// A multiple of single spacing.
    Auto(f32),
    Exact(f32),
    AtLeast(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Left,
    Center,
    Right,
    Decimal,
    Bar,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Leader {
    #[default]
    None,
    Dot,
    Hyphen,
    Underscore,
    Heavy,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TabDef {
    pub pos: f32,
    pub kind: TabKind,
    pub leader: Leader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    #[default]
    None,
    Single,
    Double,
    Dotted,
    Dashed,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Border {
    pub style: BorderStyle,
    pub width: f32,
    pub color: Rgb,
    pub space: f32,
}

impl Border {
    pub fn parse(e: &El, theme: &Theme) -> Border {
        let style = match e.val().unwrap_or("none") {
            "nil" | "none" => BorderStyle::None,
            "double" | "triple" | "thinThickSmallGap" | "thickThinSmallGap"
            | "thinThickMediumGap" | "thickThinMediumGap" | "thinThickLargeGap"
            | "thickThinLargeGap" => BorderStyle::Double,
            "dotted" | "dotDash" | "dotDotDash" => BorderStyle::Dotted,
            "dashed" | "dashSmallGap" | "dashDotStroked" => BorderStyle::Dashed,
            _ => BorderStyle::Single,
        };
        let width = e
            .attr("sz")
            .and_then(|v| v.parse::<f32>().ok())
            .map_or(0.5, |v| (v / 8.0).max(0.25));
        let color = border_color(e, theme).unwrap_or(BLACK);
        let space = e
            .attr("space")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.0);
        Border {
            style,
            width: if style == BorderStyle::Double {
                width.max(0.75)
            } else {
                width
            },
            color,
            space,
        }
    }

    pub fn visible(&self) -> bool {
        self.style != BorderStyle::None && self.width > 0.0
    }
}

#[derive(Debug, Clone, Default)]
pub struct ParaProps {
    pub style: Option<String>,
    pub jc: Option<Align>,
    pub left: Option<f32>,
    pub right: Option<f32>,
    /// First-line indent; negative for a hanging indent.
    pub first: Option<f32>,
    pub before: Option<f32>,
    pub after: Option<f32>,
    pub before_auto: Option<bool>,
    pub after_auto: Option<bool>,
    pub line: Option<LineRule>,
    pub contextual: Option<bool>,
    pub keep_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub page_break_before: Option<bool>,
    pub widow: Option<bool>,
    pub tabs: Vec<TabDef>,
    pub shd: Option<Option<Rgb>>,
    pub borders: [Option<Border>; 5],
    pub num_id: Option<u32>,
    pub ilvl: Option<u8>,
    pub outline: Option<u8>,
    pub bidi: Option<bool>,
}

impl ParaProps {
    pub fn parse(e: &El, theme: &Theme) -> ParaProps {
        let mut p = ParaProps::default();
        for c in e.children() {
            match &*c.name {
                "pStyle" => p.style = c.val().map(str::to_string),
                "jc" => p.jc = c.val().map(align),
                "ind" => {
                    let a = |k: &str| c.attr(k).and_then(twips);
                    p.left = a("left").or_else(|| a("start")).or(p.left);
                    p.right = a("right").or_else(|| a("end")).or(p.right);
                    if let Some(h) = a("hanging") {
                        p.first = Some(-h);
                    } else if let Some(f) = a("firstLine") {
                        p.first = Some(f);
                    }
                }
                "spacing" => {
                    let a = |k: &str| c.attr(k).and_then(twips);
                    p.before = a("before").or(p.before);
                    p.after = a("after").or(p.after);
                    if let Some(v) = c.attr("beforeAutospacing") {
                        p.before_auto = Some(!matches!(v, "0" | "false" | "off"));
                    }
                    if let Some(v) = c.attr("afterAutospacing") {
                        p.after_auto = Some(!matches!(v, "0" | "false" | "off"));
                    }
                    if let Some(line) = c.attr("line").and_then(|v| v.parse::<f32>().ok()) {
                        p.line = Some(match c.attr("lineRule").unwrap_or("auto") {
                            "exact" => LineRule::Exact((line / 20.0).abs()),
                            "atLeast" => LineRule::AtLeast(line / 20.0),
                            _ => LineRule::Auto((line / 240.0).max(0.06)),
                        });
                    }
                }
                "contextualSpacing" => p.contextual = Some(c.on()),
                "keepNext" => p.keep_next = Some(c.on()),
                "keepLines" => p.keep_lines = Some(c.on()),
                "pageBreakBefore" => p.page_break_before = Some(c.on()),
                "widowControl" => p.widow = Some(c.on()),
                "tabs" => {
                    for t in c.all("tab") {
                        let Some(pos) = t.attr("pos").and_then(twips) else {
                            continue;
                        };
                        let kind = match t.val().unwrap_or("left") {
                            "center" => TabKind::Center,
                            "right" | "end" => TabKind::Right,
                            "decimal" => TabKind::Decimal,
                            "bar" => TabKind::Bar,
                            "clear" => TabKind::Clear,
                            _ => TabKind::Left,
                        };
                        let leader = match t.attr("leader").unwrap_or("none") {
                            "dot" => Leader::Dot,
                            "hyphen" => Leader::Hyphen,
                            "underscore" => Leader::Underscore,
                            "heavy" => Leader::Heavy,
                            "middleDot" => Leader::Middle,
                            _ => Leader::None,
                        };
                        p.tabs.push(TabDef { pos, kind, leader });
                    }
                }
                "shd" => p.shd = shading(c, theme),
                "pBdr" => {
                    for (i, side) in ["top", "bottom", "left", "right", "between"]
                        .iter()
                        .enumerate()
                    {
                        if let Some(b) = c.child(side) {
                            p.borders[i] = Some(Border::parse(b, theme));
                        }
                    }
                    if p.borders[2].is_none()
                        && let Some(b) = c.child("start")
                    {
                        p.borders[2] = Some(Border::parse(b, theme));
                    }
                    if p.borders[3].is_none()
                        && let Some(b) = c.child("end")
                    {
                        p.borders[3] = Some(Border::parse(b, theme));
                    }
                }
                "numPr" => {
                    if let Some(id) = c.child("numId").and_then(|n| n.val()?.parse().ok()) {
                        p.num_id = Some(id);
                    }
                    if let Some(l) = c.child("ilvl").and_then(|n| n.val()?.parse().ok()) {
                        p.ilvl = Some(l);
                    }
                }
                "outlineLvl" => p.outline = c.val().and_then(|v| v.parse().ok()),
                "bidi" => p.bidi = Some(c.on()),
                _ => {}
            }
        }
        p
    }

    pub fn apply(&mut self, o: &ParaProps) {
        over!(self, o; jc, left, right, first, before, after, before_auto, after_auto, line,
            contextual, keep_next, keep_lines, page_break_before, widow, shd, num_id, ilvl,
            outline, bidi);
        for (d, s) in self.borders.iter_mut().zip(o.borders.iter()) {
            if s.is_some() {
                *d = *s;
            }
        }
        // Tabs accumulate; a "clear" removes an inherited stop at that position.
        for t in &o.tabs {
            self.tabs.retain(|x| (x.pos - t.pos).abs() > 0.5);
            self.tabs.push(*t);
        }
    }
}

// ---------------------------------------------------------------- tables

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Width {
    Auto,
    Pt(f32),
    /// A fraction of the available width.
    Pct(f32),
}

fn width(e: &El) -> Option<Width> {
    let w = e.attr("w")?;
    match e.attr("type").unwrap_or("dxa") {
        "pct" => {
            let f = if let Some(p) = w.strip_suffix('%') {
                p.parse::<f32>().ok()? / 100.0
            } else {
                w.parse::<f32>().ok()? / 5000.0
            };
            Some(Width::Pct(f))
        }
        "auto" | "nil" => Some(Width::Auto),
        _ => twips(w).map(Width::Pt),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VMerge {
    Restart,
    Continue,
}

/// Table and cell borders: top, left, bottom, right, inside horizontal, inside vertical.
pub type Edges = [Option<Border>; 6];

fn edges(e: &El, theme: &Theme) -> Edges {
    let mut out: Edges = Default::default();
    for (i, names) in [
        ["top", "top"],
        ["left", "start"],
        ["bottom", "bottom"],
        ["right", "end"],
        ["insideH", "insideH"],
        ["insideV", "insideV"],
    ]
    .iter()
    .enumerate()
    {
        if let Some(b) = e.child(names[0]).or_else(|| e.child(names[1])) {
            out[i] = Some(Border::parse(b, theme));
        }
    }
    out
}

fn margins(e: &El) -> [Option<f32>; 4] {
    let m = |a: &str, b: &str| {
        e.child(a)
            .or_else(|| e.child(b))
            .and_then(|c| c.attr("w"))
            .and_then(twips)
    };
    [
        m("top", "top"),
        m("left", "start"),
        m("bottom", "bottom"),
        m("right", "end"),
    ]
}

fn merge_edges(d: &mut Edges, s: &Edges) {
    for (a, b) in d.iter_mut().zip(s.iter()) {
        if b.is_some() {
            *a = *b;
        }
    }
}

fn merge4(d: &mut [Option<f32>; 4], s: &[Option<f32>; 4]) {
    for (a, b) in d.iter_mut().zip(s.iter()) {
        if b.is_some() {
            *a = *b;
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Look {
    pub first_row: bool,
    pub last_row: bool,
    pub first_col: bool,
    pub last_col: bool,
    pub no_hband: bool,
    pub no_vband: bool,
}

#[derive(Debug, Clone, Default)]
pub struct TblProps {
    pub style: Option<String>,
    pub width: Option<Width>,
    pub jc: Option<Align>,
    pub ind: Option<f32>,
    pub borders: Edges,
    pub mar: [Option<f32>; 4],
    pub shd: Option<Option<Rgb>>,
    pub look: Option<Look>,
    pub spacing: Option<f32>,
    pub row_band: Option<u32>,
    pub col_band: Option<u32>,
}

impl TblProps {
    pub fn parse(e: &El, theme: &Theme) -> TblProps {
        let mut t = TblProps::default();
        for c in e.children() {
            match &*c.name {
                "tblStyle" => t.style = c.val().map(str::to_string),
                "tblW" => t.width = width(c),
                "jc" => t.jc = c.val().map(align),
                "tblInd" => t.ind = c.attr("w").and_then(twips),
                "tblBorders" => t.borders = edges(c, theme),
                "tblCellMar" => t.mar = margins(c),
                "shd" => t.shd = shading(c, theme),
                "tblCellSpacing" => t.spacing = c.attr("w").and_then(twips),
                "tblStyleRowBandSize" => t.row_band = c.val().and_then(|v| v.parse().ok()),
                "tblStyleColBandSize" => t.col_band = c.val().and_then(|v| v.parse().ok()),
                "tblLook" => {
                    let bits = c.val().and_then(|v| u32::from_str_radix(v, 16).ok());
                    let flag = |name: &str, bit: u32| match c.attr(name) {
                        Some(v) => !matches!(v, "0" | "false" | "off"),
                        None => bits.is_some_and(|b| b & bit != 0),
                    };
                    t.look = Some(Look {
                        first_row: flag("firstRow", 0x0020),
                        last_row: flag("lastRow", 0x0040),
                        first_col: flag("firstColumn", 0x0080),
                        last_col: flag("lastColumn", 0x0100),
                        no_hband: flag("noHBand", 0x0200),
                        no_vband: flag("noVBand", 0x0400),
                    });
                }
                _ => {}
            }
        }
        t
    }

    pub fn apply(&mut self, o: &TblProps) {
        over!(self, o; width, jc, ind, shd, look, spacing, row_band, col_band);
        merge_edges(&mut self.borders, &o.borders);
        merge4(&mut self.mar, &o.mar);
    }
}

#[derive(Debug, Clone, Default)]
pub struct CellProps {
    pub width: Option<Width>,
    pub span: Option<usize>,
    pub vmerge: Option<VMerge>,
    pub borders: Edges,
    pub shd: Option<Option<Rgb>>,
    pub valign: Option<VAlign>,
    pub mar: [Option<f32>; 4],
}

impl CellProps {
    pub fn parse(e: &El, theme: &Theme) -> CellProps {
        let mut t = CellProps::default();
        for c in e.children() {
            match &*c.name {
                "tcW" => t.width = width(c),
                "gridSpan" => t.span = c.val().and_then(|v| v.parse().ok()),
                "vMerge" => {
                    t.vmerge = Some(match c.val() {
                        Some("restart") => VMerge::Restart,
                        _ => VMerge::Continue,
                    })
                }
                "tcBorders" => t.borders = edges(c, theme),
                "shd" => t.shd = shading(c, theme),
                "vAlign" => {
                    t.valign = Some(match c.val() {
                        Some("center") => VAlign::Center,
                        Some("bottom") => VAlign::Bottom,
                        _ => VAlign::Top,
                    })
                }
                "tcMar" => t.mar = margins(c),
                _ => {}
            }
        }
        t
    }

    pub fn apply(&mut self, o: &CellProps) {
        over!(self, o; width, span, vmerge, shd, valign);
        merge_edges(&mut self.borders, &o.borders);
        merge4(&mut self.mar, &o.mar);
    }
}

#[derive(Debug, Clone, Default)]
pub struct RowProps {
    /// Height and whether it is exact (else at least).
    pub height: Option<(f32, bool)>,
    pub header: Option<bool>,
    pub cant_split: Option<bool>,
    pub grid_before: Option<usize>,
}

impl RowProps {
    pub fn parse(e: &El) -> RowProps {
        let mut r = RowProps::default();
        for c in e.children() {
            match &*c.name {
                "trHeight" => {
                    if let Some(h) = c.val().and_then(twips) {
                        r.height = Some((h, c.attr("hRule") == Some("exact")));
                    }
                }
                "tblHeader" => r.header = Some(c.on()),
                "cantSplit" => r.cant_split = Some(c.on()),
                "gridBefore" => r.grid_before = c.val().and_then(|v| v.parse().ok()),
                _ => {}
            }
        }
        r
    }

    pub fn apply(&mut self, o: &RowProps) {
        over!(self, o; height, header, cant_split, grid_before);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(twips("1440"), Some(72.0));
        assert_eq!(twips("1in"), Some(72.0));
        assert!((twips("2.54cm").unwrap() - 72.0).abs() < 0.01);
        assert_eq!(emu("914400"), Some(72.0));
        assert_eq!(Rgb::hex("FF8000"), Some(Rgb(255, 128, 0)));
        assert_eq!(Rgb(200, 100, 0).shade(0.5), Rgb(100, 50, 0));
    }
}
