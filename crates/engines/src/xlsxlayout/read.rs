//! Reading a workbook for printing: styles, shared strings, and per sheet the cells,
//! column widths, row heights, merges, page setup, print area and titles, page breaks,
//! headers and footers, and pictures.

use std::collections::HashMap;
use std::sync::Arc;

use swirvo_core::EngineError;

use crate::docxlayout::model::{Media, Meta};
use crate::docxlayout::props::{BorderStyle, Rgb, Underline, Vert};
use crate::docxlayout::theme::{Theme, from_hsl, hsl};
use crate::docxlayout::xml::El;
use crate::opc::{Package, Rels};

/// Cells per workbook we lay out at most (a hostile file can declare millions).
const MAX_CELLS: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub name: String,
    pub sz: f32,
    pub b: bool,
    pub i: bool,
    pub strike: bool,
    pub u: Underline,
    pub color: Option<Rgb>,
    pub vert: Vert,
}

impl Default for Font {
    fn default() -> Self {
        Font {
            name: "Calibri".into(),
            sz: 11.0,
            b: false,
            i: false,
            strike: false,
            u: Underline::None,
            color: None,
            vert: Vert::Baseline,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub style: BorderStyle,
    pub width: f32,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Borders {
    /// Left, right, top, bottom.
    pub e: [Option<Edge>; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    General,
    Left,
    Center,
    Right,
    Fill,
    Justify,
    CenterAcross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    Top,
    Center,
    #[default]
    Bottom,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Xf {
    pub num_fmt: u32,
    pub font: usize,
    pub fill: usize,
    pub border: usize,
    pub h: HAlign,
    pub v: VAlign,
    pub wrap: bool,
    pub indent: u32,
    pub rot: i32,
    pub shrink: bool,
}

#[derive(Debug, Default)]
pub struct Styles {
    pub xfs: Vec<Xf>,
    pub fonts: Vec<Font>,
    pub fills: Vec<Option<Rgb>>,
    pub borders: Vec<Borders>,
    pub numfmts: HashMap<u32, String>,
}

impl Styles {
    pub fn xf(&self, i: usize) -> &Xf {
        static DEFAULT: std::sync::OnceLock<Xf> = std::sync::OnceLock::new();
        self.xfs
            .get(i)
            .unwrap_or_else(|| DEFAULT.get_or_init(Xf::default))
    }

    pub fn font(&self, i: usize) -> Font {
        self.fonts
            .get(i)
            .or(self.fonts.first())
            .cloned()
            .unwrap_or_default()
    }

    pub fn fmt(&self, id: u32) -> &str {
        self.numfmts
            .get(&id)
            .map(String::as_str)
            .or_else(|| super::numfmt::builtin(id))
            .unwrap_or("General")
    }
}

/// Text with optional per-run fonts (rich text in shared strings).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Rich {
    pub runs: Vec<(String, Option<Font>)>,
}

impl Rich {
    pub fn plain(&self) -> String {
        self.runs.iter().map(|r| r.0.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Num(f64),
    Str(Arc<Rich>),
    Bool(bool),
    Err(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub v: Value,
    pub xf: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup {
    /// Paper width and height in points (portrait).
    pub paper: (f32, f32),
    pub landscape: bool,
    /// Percent of actual size.
    pub scale: f32,
    /// Fit to (pages wide, pages tall); 0 = no limit in that direction.
    pub fit: Option<(u32, u32)>,
    /// Left, right, top, bottom, header, footer, in points.
    pub margins: [f32; 6],
    pub grid: bool,
    pub center_h: bool,
    pub center_v: bool,
    pub over_then_down: bool,
    pub first_page: Option<u32>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            paper: (612.0, 792.0),
            landscape: false,
            scale: 100.0,
            fit: None,
            margins: [50.4, 50.4, 54.0, 54.0, 21.6, 21.6],
            grid: false,
            center_h: false,
            center_v: false,
            over_then_down: false,
            first_page: None,
        }
    }
}

/// A picture anchored to cells: (col, x offset, row, y offset) for its corners, in
/// points within the cell.
#[derive(Debug, Clone)]
pub struct Pic {
    pub from: (u32, f32, u32, f32),
    pub to: Option<(u32, f32, u32, f32)>,
    /// Size when only the top-left corner is anchored.
    pub ext: (f32, f32),
    pub what: Drawn,
}

/// What a drawing anchor holds.
#[derive(Debug, Clone)]
pub enum Drawn {
    Picture(Arc<Media>),
    Chart(Box<crate::chart::Chart>),
}

#[derive(Debug, Default)]
pub struct Sheet {
    pub name: String,
    pub cells: HashMap<(u32, u32), Cell>,
    /// Last row and column with something printable (0-based), if any.
    pub used: Option<(u32, u32, u32, u32)>,
    /// Column widths in points and hidden flags, per column index.
    pub col_w: HashMap<u32, (f32, bool)>,
    pub default_col_w: f32,
    /// Row heights in points (explicit) and hidden flags.
    pub row_h: HashMap<u32, (Option<f32>, bool)>,
    pub default_row_h: f32,
    pub merges: Vec<[u32; 4]>,
    pub setup: Setup,
    pub print_area: Option<[u32; 4]>,
    pub title_rows: Option<(u32, u32)>,
    pub title_cols: Option<(u32, u32)>,
    /// Manual page breaks: after these rows / columns.
    pub row_breaks: Vec<u32>,
    pub col_breaks: Vec<u32>,
    pub header: Option<String>,
    pub footer: Option<String>,
    pub links: Vec<([u32; 4], String)>,
    pub pics: Vec<Pic>,
}

pub struct Book {
    pub sheets: Vec<Sheet>,
    pub styles: Styles,
    pub date1904: bool,
    pub meta: Meta,
}

// ------------------------------------------------------------ colours

const INDEXED: [u32; 64] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x000000,
    0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000, 0x008000,
    0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC,
    0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF,
    0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99, 0x99CCFF,
    0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600,
    0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399,
    0x333333,
];

/// An Excel colour (`rgb` ARGB, `theme` + `tint`, `indexed`, `auto`).
pub fn color(e: &El, theme: &Theme) -> Option<Rgb> {
    let base = if let Some(rgb) = e.attr("rgb") {
        let h = rgb.trim();
        Rgb::hex(&h[h.len().saturating_sub(6)..])?
    } else if let Some(t) = e.attr("theme").and_then(|v| v.parse::<usize>().ok()) {
        // Excel's theme index order puts the light colours first.
        let name = [
            "lt1", "dk1", "lt2", "dk2", "accent1", "accent2", "accent3", "accent4", "accent5",
            "accent6", "hlink", "folHlink",
        ]
        .get(t)?;
        // Without a theme part, Excel's default Office theme applies.
        theme.color(name).or_else(|| {
            let v = match *name {
                "lt1" => 0xFFFFFF,
                "dk1" => 0x000000,
                "lt2" => 0xE7E6E6,
                "dk2" => 0x44546A,
                "accent1" => 0x4472C4,
                "accent2" => 0xED7D31,
                "accent3" => 0xA5A5A5,
                "accent4" => 0xFFC000,
                "accent5" => 0x5B9BD5,
                "accent6" => 0x70AD47,
                "hlink" => 0x0563C1,
                _ => 0x954F72,
            };
            Some(Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
        })?
    } else if let Some(i) = e.attr("indexed").and_then(|v| v.parse::<usize>().ok()) {
        match i {
            64 => Rgb(0, 0, 0),
            65 => Rgb(255, 255, 255),
            i => {
                let v = *INDEXED.get(i)?;
                Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
            }
        }
    } else if e.attr("auto").is_some() {
        Rgb(0, 0, 0)
    } else {
        return None;
    };
    let tint = e
        .attr("tint")
        .and_then(|t| t.parse::<f32>().ok())
        .unwrap_or(0.0);
    if tint == 0.0 {
        return Some(base);
    }
    let (h, s, l) = hsl(base);
    let l = if tint < 0.0 {
        l * (1.0 + tint)
    } else {
        l * (1.0 - tint) + tint
    };
    Some(from_hsl(h, s, l.clamp(0.0, 1.0)))
}

// ------------------------------------------------------------ styles

fn font(e: &El, theme: &Theme) -> Font {
    let on = |k: &str| e.child(k).is_some_and(|x| x.on());
    Font {
        name: e
            .child("name")
            .or_else(|| e.child("rFont"))
            .and_then(|n| n.val())
            .unwrap_or("Calibri")
            .to_string(),
        sz: e
            .child("sz")
            .and_then(|s| s.val())
            .and_then(|v| v.parse().ok())
            .unwrap_or(11.0),
        b: on("b"),
        i: on("i"),
        strike: on("strike"),
        u: match e.child("u").map(|u| u.val().unwrap_or("single")) {
            Some("double" | "doubleAccounting") => Underline::Double,
            Some("none") | None => Underline::None,
            Some(_) => Underline::Single,
        },
        color: e.child("color").and_then(|c| color(c, theme)),
        vert: match e.child("vertAlign").and_then(|v| v.val()) {
            Some("superscript") => Vert::Super,
            Some("subscript") => Vert::Sub,
            _ => Vert::Baseline,
        },
    }
}

fn edge(e: &El, theme: &Theme) -> Option<Edge> {
    let style = e.attr("style")?;
    let (style, width) = match style {
        "hair" => (BorderStyle::Dotted, 0.25),
        "thin" => (BorderStyle::Single, 0.5),
        "medium" => (BorderStyle::Single, 1.25),
        "thick" => (BorderStyle::Single, 2.0),
        "double" => (BorderStyle::Double, 1.75),
        "dotted" => (BorderStyle::Dotted, 0.5),
        "dashed" | "dashDot" | "dashDotDot" => (BorderStyle::Dashed, 0.5),
        "mediumDashed" | "mediumDashDot" | "mediumDashDotDot" | "slantDashDot" => {
            (BorderStyle::Dashed, 1.25)
        }
        _ => return None,
    };
    Some(Edge {
        style,
        width,
        color: e
            .child("color")
            .and_then(|c| color(c, theme))
            .unwrap_or(Rgb(0, 0, 0)),
    })
}

fn read_styles(root: &El, theme: &Theme) -> Styles {
    let mut s = Styles::default();
    if let Some(n) = root.child("numFmts") {
        for f in n.all("numFmt") {
            if let (Some(id), Some(code)) = (
                f.attr("numFmtId").and_then(|v| v.parse().ok()),
                f.attr("formatCode"),
            ) {
                s.numfmts.insert(id, code.to_string());
            }
        }
    }
    if let Some(f) = root.child("fonts") {
        s.fonts = f.all("font").map(|x| font(x, theme)).collect();
    }
    if let Some(f) = root.child("fills") {
        s.fills = f
            .all("fill")
            .map(|x| {
                let p = x.child("patternFill")?;
                match p.attr("patternType") {
                    None | Some("none") => None,
                    // Patterns print as their foreground colour (close enough for 50 %
                    // grey and friends).
                    Some(_) => p
                        .child("fgColor")
                        .and_then(|c| color(c, theme))
                        .or_else(|| p.child("bgColor").and_then(|c| color(c, theme))),
                }
                .or_else(|| {
                    // Gradient fills: their first stop.
                    x.child("gradientFill")
                        .and_then(|g| g.child("stop"))
                        .and_then(|st| st.child("color"))
                        .and_then(|c| color(c, theme))
                })
            })
            .collect();
    }
    if let Some(b) = root.child("borders") {
        s.borders = b
            .all("border")
            .map(|x| Borders {
                e: [
                    x.child("left")
                        .or_else(|| x.child("start"))
                        .and_then(|e| edge(e, theme)),
                    x.child("right")
                        .or_else(|| x.child("end"))
                        .and_then(|e| edge(e, theme)),
                    x.child("top").and_then(|e| edge(e, theme)),
                    x.child("bottom").and_then(|e| edge(e, theme)),
                ],
            })
            .collect();
    }
    if let Some(x) = root.child("cellXfs") {
        s.xfs = x
            .all("xf")
            .map(|xf| {
                let num = |k: &str| {
                    xf.attr(k)
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0)
                };
                let al = xf.child("alignment");
                let a = |k: &str| al.and_then(|a| a.attr(k));
                Xf {
                    num_fmt: num("numFmtId") as u32,
                    font: num("fontId"),
                    fill: num("fillId"),
                    border: num("borderId"),
                    h: match a("horizontal") {
                        Some("left") => HAlign::Left,
                        Some("center") => HAlign::Center,
                        Some("right") => HAlign::Right,
                        Some("fill") => HAlign::Fill,
                        Some("justify" | "distributed") => HAlign::Justify,
                        Some("centerContinuous") => HAlign::CenterAcross,
                        _ => HAlign::General,
                    },
                    v: match a("vertical") {
                        Some("top") => VAlign::Top,
                        Some("center" | "justify" | "distributed") => VAlign::Center,
                        _ => VAlign::Bottom,
                    },
                    wrap: a("wrapText").is_some_and(|v| v == "1" || v == "true"),
                    indent: a("indent").and_then(|v| v.parse().ok()).unwrap_or(0),
                    rot: a("textRotation").and_then(|v| v.parse().ok()).unwrap_or(0),
                    shrink: a("shrinkToFit").is_some_and(|v| v == "1" || v == "true"),
                }
            })
            .collect();
    }
    s
}

fn shared_strings(root: &El, theme: &Theme) -> Vec<Arc<Rich>> {
    root.all("si")
        .map(|si| {
            let mut r = Rich::default();
            if let Some(t) = si.child("t") {
                r.runs.push((t.text(), None));
            }
            for run in si.all("r") {
                let f = run.child("rPr").map(|p| font(p, theme));
                if let Some(t) = run.child("t") {
                    r.runs.push((t.text(), f));
                }
            }
            Arc::new(r)
        })
        .collect()
}

// ------------------------------------------------------------ references

/// `B12` → (row 11, col 1).
pub fn cell_ref(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().trim_start_matches('$');
    let letters: String = s
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '$')
        .filter(|c| *c != '$')
        .collect();
    let digits: String = s
        .chars()
        .skip_while(|c| c.is_ascii_alphabetic() || *c == '$')
        .filter(|c| *c != '$')
        .collect();
    let col = letters.chars().try_fold(0u32, |a, c| {
        Some(a.checked_mul(26)? + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1))
    })?;
    let row: u32 = digits.parse().ok()?;
    (col >= 1 && row >= 1).then(|| (row - 1, col - 1))
}

/// `A1:F40` → [r0, c0, r1, c1]; a single cell is a 1×1 range.
pub fn range_ref(s: &str) -> Option<[u32; 4]> {
    let (a, b) = s.split_once(':').unwrap_or((s, s));
    let (r0, c0) = cell_ref(a)?;
    let (r1, c1) = cell_ref(b)?;
    Some([r0.min(r1), c0.min(c1), r0.max(r1), c0.max(c1)])
}

fn col_letters_to_index(s: &str) -> Option<u32> {
    s.trim_matches('$')
        .chars()
        .try_fold(0u32, |a, c| {
            c.is_ascii_alphabetic()
                .then(|| a * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1))
        })
        .filter(|v| *v > 0)
        .map(|v| v - 1)
}

/// A first..last range of rows or columns.
type Span = Option<(u32, u32)>;

/// `Sheet1!$1:$2,Sheet1!$A:$B` → (rows, cols).
fn print_titles(v: &str) -> (Span, Span) {
    let (mut rows, mut cols) = (None, None);
    for part in v.split(',') {
        let r = part.rsplit('!').next().unwrap_or(part);
        let (a, b) = r.split_once(':').unwrap_or((r, r));
        let (a, b) = (a.trim_matches('$'), b.trim_matches('$'));
        if let (Ok(x), Ok(y)) = (a.parse::<u32>(), b.parse::<u32>()) {
            rows = Some((x.saturating_sub(1), y.saturating_sub(1)));
        } else if let (Some(x), Some(y)) = (col_letters_to_index(a), col_letters_to_index(b)) {
            cols = Some((x, y));
        }
    }
    (rows, cols)
}

// ------------------------------------------------------------ the workbook

pub fn read(bytes: &[u8]) -> Result<Book, EngineError> {
    let mut pkg = Package::open(bytes)?;
    let main = pkg.main_part("xl/workbook.xml");
    let wb = pkg
        .xml(&main)
        .ok_or_else(|| EngineError::Decode("not an Excel workbook (no workbook part)".into()))?;
    let rels = pkg.rels(&main);
    let theme = rels
        .of_type("/theme")
        .map(|r| r.target.clone())
        .and_then(|t| pkg.xml(&t))
        .map(|t| Theme::parse(&t))
        .unwrap_or_default();
    let styles = rels
        .of_type("/styles")
        .map(|r| r.target.clone())
        .and_then(|t| pkg.xml(&t))
        .map(|s| read_styles(&s, &theme))
        .unwrap_or_default();
    let strings = rels
        .of_type("/sharedStrings")
        .map(|r| r.target.clone())
        .and_then(|t| pkg.xml(&t))
        .map(|s| shared_strings(&s, &theme))
        .unwrap_or_default();
    let date1904 = wb
        .child("workbookPr")
        .and_then(|p| p.attr("date1904"))
        .is_some_and(|v| v == "1" || v == "true");
    // Defined names: print areas and titles, per sheet index.
    let mut areas: HashMap<usize, String> = HashMap::new();
    let mut titles: HashMap<usize, String> = HashMap::new();
    if let Some(dn) = wb.child("definedNames") {
        for n in dn.all("definedName") {
            let Some(i) = n.attr("localSheetId").and_then(|v| v.parse::<usize>().ok()) else {
                continue;
            };
            match n.attr("name") {
                Some("_xlnm.Print_Area") => {
                    areas.insert(i, n.text());
                }
                Some("_xlnm.Print_Titles") => {
                    titles.insert(i, n.text());
                }
                _ => {}
            }
        }
    }
    let mut sheets = Vec::new();
    let mut budget = MAX_CELLS;
    for (i, s) in wb
        .child("sheets")
        .map(|s| s.all("sheet").collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        if matches!(s.attr("state"), Some("hidden" | "veryHidden")) {
            continue;
        }
        // The relationship id is the `id` attribute that resolves.
        let Some(part) = s
            .attrs
            .iter()
            .filter(|(k, _)| &**k == "id")
            .find_map(|(_, v)| rels.target(v))
            .map(str::to_string)
        else {
            continue;
        };
        let Some(root) = pkg.xml(&part) else { continue };
        let srels = pkg.rels(&part);
        let mut sheet = read_sheet(
            &mut pkg,
            &root,
            &srels,
            &styles,
            &strings,
            &theme,
            &mut budget,
        );
        sheet.name = s.attr("name").unwrap_or("Sheet").to_string();
        if let Some(a) = areas.get(&i) {
            let first = a.split(',').next().unwrap_or("");
            sheet.print_area = range_ref(first.rsplit('!').next().unwrap_or(first));
        }
        if let Some(t) = titles.get(&i) {
            (sheet.title_rows, sheet.title_cols) = print_titles(t);
        }
        sheets.push(sheet);
        if budget == 0 {
            break;
        }
    }
    let meta = pkg
        .xml("docProps/core.xml")
        .map(|core| {
            let get = |k: &str| {
                core.child(k)
                    .map(|e| e.text().trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            Meta {
                title: get("title"),
                author: get("creator"),
                subject: get("subject"),
                keywords: get("keywords"),
                language: None,
            }
        })
        .unwrap_or_default();
    Ok(Book {
        sheets,
        styles,
        date1904,
        meta,
    })
}

/// Excel column width (characters of the default font's widest digit, with padding) →
/// points. `mdw` is that digit's width in pixels (7 for Calibri 11).
fn chars_to_pt(w: f32, mdw: f32) -> f32 {
    let px = ((256.0 * w + (128.0 / mdw).floor()) / 256.0 * mdw).floor();
    px * 0.75
}

fn paper(code: u32) -> (f32, f32) {
    match code {
        5 => (612.0, 1008.0),
        8 => (841.89, 1190.55),
        9 => (595.28, 841.89),
        11 => (419.53, 595.28),
        12 => (708.66, 1000.63),
        13 => (498.9, 708.66),
        _ => (612.0, 792.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_sheet(
    pkg: &mut Package<'_>,
    root: &El,
    rels: &Rels,
    styles: &Styles,
    strings: &[Arc<Rich>],
    theme: &Theme,
    budget: &mut usize,
) -> Sheet {
    let mut s = Sheet::default();
    let base_font = styles.font(0);
    let mdw = (7.0 * base_font.sz / 11.0).round().max(4.0);
    let fmt_pr = root.child("sheetFormatPr");
    let base_cw = fmt_pr
        .and_then(|f| f.attr("baseColWidth"))
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(8.0);
    s.default_col_w = fmt_pr
        .and_then(|f| f.attr("defaultColWidth"))
        .and_then(|v| v.parse::<f32>().ok())
        .map(|w| chars_to_pt(w, mdw))
        // Base width plus 5 px of padding, rounded up to a multiple of 8 px.
        .unwrap_or_else(|| ((base_cw * mdw + 5.0) / 8.0).ceil() * 8.0 * 0.75);
    s.default_row_h = fmt_pr
        .and_then(|f| f.attr("defaultRowHeight"))
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or((base_font.sz * 1.36).round().max(12.75));
    if let Some(cols) = root.child("cols") {
        for c in cols.all("col") {
            let (Some(min), Some(max)) = (
                c.attr("min").and_then(|v| v.parse::<u32>().ok()),
                c.attr("max").and_then(|v| v.parse::<u32>().ok()),
            ) else {
                continue;
            };
            let w = c
                .attr("width")
                .and_then(|v| v.parse::<f32>().ok())
                .map(|w| chars_to_pt(w, mdw))
                .unwrap_or(s.default_col_w);
            let hidden = c.attr("hidden").is_some_and(|v| v == "1" || v == "true");
            // A style-only range to the last column (16384) is common; skip huge spans.
            for i in min.max(1)..=max.min(min.max(1) + 2048) {
                s.col_w.insert(i - 1, (w, hidden));
            }
        }
    }
    let mut used: Option<(u32, u32, u32, u32)> = None;
    let mut grow = |r: u32, c: u32| {
        used = Some(match used {
            None => (r, c, r, c),
            Some((r0, c0, r1, c1)) => (r0.min(r), c0.min(c), r1.max(r), c1.max(c)),
        });
    };
    if let Some(data) = root.child("sheetData") {
        let mut next_row = 0u32;
        for row in data.all("row") {
            let r = row
                .attr("r")
                .and_then(|v| v.parse::<u32>().ok())
                .map_or(next_row, |v| v.saturating_sub(1));
            next_row = r + 1;
            let ht = row.attr("ht").and_then(|v| v.parse::<f32>().ok());
            let custom = row
                .attr("customHeight")
                .is_some_and(|v| v == "1" || v == "true");
            let hidden = row.attr("hidden").is_some_and(|v| v == "1" || v == "true");
            if ht.is_some() || hidden {
                s.row_h
                    .insert(r, (ht.filter(|_| custom || ht.is_some()), hidden));
            }
            let mut next_col = 0u32;
            for c in row.all("c") {
                if *budget == 0 {
                    break;
                }
                let (rr, cc) = c.attr("r").and_then(cell_ref).unwrap_or((r, next_col));
                next_col = cc + 1;
                let xf = c
                    .attr("s")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(0);
                let raw = c.child("v").map(El::text);
                let v = match (c.attr("t"), raw) {
                    (Some("s"), Some(i)) => i
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| strings.get(i).cloned())
                        .map_or(Value::Empty, Value::Str),
                    (Some("inlineStr"), _) => {
                        let is = c.child("is");
                        let mut rich = Rich::default();
                        if let Some(is) = is {
                            if let Some(t) = is.child("t") {
                                rich.runs.push((t.text(), None));
                            }
                            for run in is.all("r") {
                                if let Some(t) = run.child("t") {
                                    rich.runs
                                        .push((t.text(), run.child("rPr").map(|p| font(p, theme))));
                                }
                            }
                        }
                        Value::Str(Arc::new(rich))
                    }
                    (Some("str"), Some(t)) => Value::Str(Arc::new(Rich {
                        runs: vec![(t, None)],
                    })),
                    (Some("b"), Some(t)) => Value::Bool(t.trim() == "1"),
                    (Some("e"), Some(t)) => Value::Err(t),
                    (_, Some(t)) => t.trim().parse::<f64>().map_or(Value::Empty, Value::Num),
                    (_, None) => Value::Empty,
                };
                let styled = {
                    let x = styles.xf(xf);
                    styles.fills.get(x.fill).copied().flatten().is_some()
                        || styles
                            .borders
                            .get(x.border)
                            .is_some_and(|b| b.e.iter().any(Option::is_some))
                };
                if v != Value::Empty || styled {
                    grow(rr, cc);
                    s.cells.insert((rr, cc), Cell { v, xf });
                    *budget -= 1;
                }
            }
        }
    }
    if let Some(m) = root.child("mergeCells") {
        for mc in m.all("mergeCell") {
            if let Some(r) = mc.attr("ref").and_then(range_ref) {
                grow(r[0], r[1]);
                grow(r[2], r[3]);
                s.merges.push(r);
            }
        }
    }
    // Page setup.
    let mut st = Setup::default();
    if let Some(m) = root.child("pageMargins") {
        let inch = |k: &str, d: f32| {
            m.attr(k)
                .and_then(|v| v.parse::<f32>().ok())
                .map_or(d, |v| v * 72.0)
        };
        st.margins = [
            inch("left", 50.4),
            inch("right", 50.4),
            inch("top", 54.0),
            inch("bottom", 54.0),
            inch("header", 21.6),
            inch("footer", 21.6),
        ];
    }
    if let Some(p) = root.child("pageSetup") {
        st.paper = paper(
            p.attr("paperSize")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1),
        );
        st.landscape = p.attr("orientation") == Some("landscape");
        st.scale = p
            .attr("scale")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(100.0)
            .clamp(10.0, 400.0);
        st.over_then_down = p.attr("pageOrder") == Some("overThenDown");
        if p.attr("useFirstPageNumber")
            .is_some_and(|v| v == "1" || v == "true")
        {
            st.first_page = p.attr("firstPageNumber").and_then(|v| v.parse().ok());
        }
        let fit_on = root
            .child("sheetPr")
            .and_then(|sp| sp.child("pageSetUpPr"))
            .and_then(|p| p.attr("fitToPage"))
            .is_some_and(|v| v == "1" || v == "true");
        if fit_on {
            let n = |k: &str| p.attr(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
            st.fit = Some((n("fitToWidth"), n("fitToHeight")));
        }
    } else if root
        .child("sheetPr")
        .and_then(|sp| sp.child("pageSetUpPr"))
        .and_then(|p| p.attr("fitToPage"))
        .is_some_and(|v| v == "1" || v == "true")
    {
        st.fit = Some((1, 1));
    }
    if let Some(po) = root.child("printOptions") {
        let on = |k: &str| po.attr(k).is_some_and(|v| v == "1" || v == "true");
        st.grid = on("gridLines");
        st.center_h = on("horizontalCentered");
        st.center_v = on("verticalCentered");
    }
    s.setup = st;
    let breaks = |name: &str| -> Vec<u32> {
        root.child(name)
            .map(|b| {
                b.all("brk")
                    .filter_map(|x| x.attr("id").and_then(|v| v.parse::<u32>().ok()))
                    .collect()
            })
            .unwrap_or_default()
    };
    // `brk id` is the 1-based index of the first row/column after the break.
    s.row_breaks = breaks("rowBreaks")
        .into_iter()
        .map(|v| v.saturating_sub(1))
        .collect();
    s.col_breaks = breaks("colBreaks")
        .into_iter()
        .map(|v| v.saturating_sub(1))
        .collect();
    if let Some(hf) = root.child("headerFooter") {
        s.header = hf
            .child("oddHeader")
            .map(El::text)
            .filter(|t| !t.is_empty());
        s.footer = hf
            .child("oddFooter")
            .map(El::text)
            .filter(|t| !t.is_empty());
    }
    if let Some(hl) = root.child("hyperlinks") {
        for h in hl.all("hyperlink") {
            let (Some(r), Some(url)) = (
                h.attr("ref").and_then(range_ref),
                h.attrs
                    .iter()
                    .filter(|(k, _)| &**k == "id")
                    .find_map(|(_, v)| {
                        rels.get(v).filter(|x| x.external).map(|x| x.target.clone())
                    }),
            ) else {
                continue;
            };
            s.links.push((r, url));
        }
    }
    // Pictures from the drawing part.
    if let Some(d) = root.child("drawing").and_then(|d| {
        d.attrs
            .iter()
            .filter(|(k, _)| &**k == "id")
            .find_map(|(_, v)| rels.target(v))
            .map(str::to_string)
    }) && let Some(droot) = pkg.xml(&d)
    {
        let drels = pkg.rels(&d);
        let pos = |e: &El| -> Option<(u32, f32, u32, f32)> {
            let n = |k: &str| {
                e.child(k)
                    .map(|x| x.text().trim().parse::<f64>().unwrap_or(0.0))
            };
            Some((
                n("col")? as u32,
                (n("colOff").unwrap_or(0.0) / 12_700.0) as f32,
                n("row")? as u32,
                (n("rowOff").unwrap_or(0.0) / 12_700.0) as f32,
            ))
        };
        let colors = crate::pptxlayout::style::Colors {
            theme: theme.clone(),
            fmt: Default::default(),
            map: Default::default(),
        };
        for anchor in droot.children() {
            let what = if let Some(pic) = anchor.child("pic") {
                let Some(id) = pic
                    .child("blipFill")
                    .and_then(|b| b.child("blip"))
                    .and_then(|b| b.attr("embed"))
                else {
                    continue;
                };
                let Some(bytes) = drels
                    .target(id)
                    .map(str::to_string)
                    .and_then(|t| pkg.bytes(&t))
                else {
                    continue;
                };
                if !crate::emf::drawable(&bytes) {
                    continue;
                }
                Drawn::Picture(Arc::new(Media {
                    bytes: Arc::new(bytes),
                }))
            } else if let Some(ch) = anchor.child("graphicFrame").and_then(|g| g.find("chart")) {
                let Some(root) = ch
                    .attr("id")
                    .and_then(|id| drels.target(id))
                    .map(str::to_string)
                    .and_then(|t| pkg.xml(&t))
                else {
                    continue;
                };
                let font = if theme.minor[0].is_empty() {
                    "Calibri"
                } else {
                    &theme.minor[0]
                };
                let Some(mut c) = crate::chart::parse(&root, &colors, font) else {
                    continue;
                };
                // Excel's automatic chart area: white, with a light gray border.
                if root.child("spPr").is_none() {
                    c.background = Some((Rgb(255, 255, 255), 1.0));
                    c.border = Some(((Rgb(0xD9, 0xD9, 0xD9), 1.0), 0.75));
                }
                Drawn::Chart(Box::new(c))
            } else {
                continue;
            };
            let Some(from) = anchor.child("from").and_then(pos) else {
                continue;
            };
            let to = anchor.child("to").and_then(pos);
            let ext = anchor
                .child("ext")
                .map(|e| {
                    let n = |k: &str| {
                        e.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0) / 12_700.0
                    };
                    (n("cx"), n("cy"))
                })
                .unwrap_or((0.0, 0.0));
            grow(from.2, from.0);
            if let Some(t) = to {
                grow(t.2, t.0);
            }
            s.pics.push(Pic {
                from,
                to,
                ext,
                what,
            });
        }
    }
    s.used = used;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references() {
        assert_eq!(cell_ref("A1"), Some((0, 0)));
        assert_eq!(cell_ref("$AB$12"), Some((11, 27)));
        assert_eq!(range_ref("B2:D10"), Some([1, 1, 9, 3]));
        assert_eq!(
            print_titles("Sheet1!$1:$2,Sheet1!$A:$B"),
            (Some((0, 1)), Some((0, 1)))
        );
        // Stored widths include the padding: the default 8.43 is stored as 9.140625 (64 px).
        assert_eq!(chars_to_pt(9.140625, 7.0), 48.0);
    }

    #[test]
    fn excel_tints() {
        let theme = Theme::default();
        let mut e = El::default();
        e.attrs.push(("rgb".into(), "FF4472C4".into()));
        e.attrs.push(("tint".into(), "0.7999816888943144".into()));
        let c = color(&e, &theme).unwrap();
        assert!(c.0 > 210 && c.2 > 235, "{c:?}");
    }
}
