//! The resolved document the layout engine consumes: styles are already flattened into
//! concrete values, lists carry their labels, and pictures carry their bytes.

use std::collections::HashMap;
use std::sync::Arc;

use super::numbering::{NumFmt, Suffix};
use super::props::{Align, Border, LineRule, Rgb, TabDef, Underline, VAlign, VMerge, Vert, Width};

#[derive(Debug, Default)]
pub struct Document {
    pub sections: Vec<Section>,
    /// Header and footer parts by relationship id.
    pub parts: HashMap<String, Vec<Block>>,
    pub footnotes: HashMap<String, Vec<Block>>,
    pub endnotes: HashMap<String, Vec<Block>>,
    pub default_tab: f32,
    pub even_odd: bool,
    /// Word 2013+ layout (compatibility mode 15): tables start at the margin.
    pub compat15: bool,
    pub expand_shift_return: bool,
    pub uses_numpages: bool,
    pub meta: Meta,
    /// Font table hints: family name → generic class, for substitution.
    pub generics: HashMap<String, Generic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Generic {
    Serif,
    #[default]
    Sans,
    Mono,
}

#[derive(Debug, Default, Clone)]
pub struct Meta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SectKind {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
    NextColumn,
}

#[derive(Debug, Clone)]
pub struct Cols {
    /// Width and the gap after each column.
    pub cols: Vec<(f32, f32)>,
    pub sep: bool,
}

#[derive(Debug, Clone)]
pub struct SectPr {
    pub w: f32,
    pub h: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub header: f32,
    pub footer: f32,
    pub gutter: f32,
    pub cols: Cols,
    pub kind: SectKind,
    pub title_pg: bool,
    /// Header and footer relationship ids: default, first, even.
    pub headers: [Option<String>; 3],
    pub footers: [Option<String>; 3],
    pub pg_start: Option<u32>,
    pub pg_fmt: NumFmt,
    pub v_align: VAlign,
}

impl Default for SectPr {
    /// Word's defaults: Letter, one-inch margins, half-inch header and footer.
    fn default() -> Self {
        SectPr {
            w: 612.0,
            h: 792.0,
            top: 72.0,
            right: 72.0,
            bottom: 72.0,
            left: 72.0,
            header: 36.0,
            footer: 36.0,
            gutter: 0.0,
            cols: Cols {
                cols: vec![(468.0, 0.0)],
                sep: false,
            },
            kind: SectKind::NextPage,
            title_pg: false,
            headers: Default::default(),
            footers: Default::default(),
            pg_start: None,
            pg_fmt: NumFmt::Decimal,
            v_align: VAlign::Top,
        }
    }
}

#[derive(Debug)]
pub struct Section {
    pub sect: SectPr,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum Block {
    Para(Box<Para>),
    Table(Box<Table>),
}

/// Paragraph properties with every inherited value filled in.
#[derive(Debug, Clone)]
pub struct ParaStyle {
    pub align: Align,
    pub left: f32,
    pub right: f32,
    pub first: f32,
    pub before: f32,
    pub after: f32,
    pub line: LineRule,
    pub contextual: bool,
    pub keep_next: bool,
    pub keep_lines: bool,
    pub page_break_before: bool,
    pub widow: bool,
    pub tabs: Vec<TabDef>,
    pub shd: Option<Rgb>,
    pub borders: [Option<Border>; 5],
    pub bidi: bool,
    pub style: String,
}

#[derive(Debug)]
pub struct Label {
    pub text: String,
    pub rs: Arc<RunStyle>,
    pub suffix: Suffix,
    pub jc: Align,
}

#[derive(Debug)]
pub struct Para {
    pub p: ParaStyle,
    /// The paragraph mark's run style: an empty paragraph's line height comes from it.
    pub mark: Arc<RunStyle>,
    pub items: Vec<Item>,
    pub label: Option<Label>,
    pub outline: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Link {
    Url(String),
    Anchor(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    Line,
    Page,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Page,
    NumPages,
    SectionPages,
}

#[derive(Debug)]
pub enum Item {
    Text {
        text: String,
        rs: Arc<RunStyle>,
        link: Option<Arc<Link>>,
    },
    Tab {
        rs: Arc<RunStyle>,
    },
    Break(BreakKind),
    Field {
        kind: FieldKind,
        /// A `\\* roman`-style switch; else the section's page number format.
        fmt: Option<NumFmt>,
        rs: Arc<RunStyle>,
    },
    Drawing(Box<Drawing>),
    NoteRef {
        id: String,
        endnote: bool,
        rs: Arc<RunStyle>,
    },
    /// Inside a footnote: its own number.
    NoteMark {
        rs: Arc<RunStyle>,
    },
    Bookmark(String),
}

/// Run formatting with every inherited value filled in.
#[derive(Debug, Clone, PartialEq)]
pub struct RunStyle {
    /// Font families for ASCII, other Latin, East Asian and complex-script text.
    pub fonts: [String; 4],
    pub b: bool,
    pub i: bool,
    pub b_cs: bool,
    pub i_cs: bool,
    pub sz: f32,
    pub sz_cs: f32,
    pub color: Rgb,
    /// The color is "automatic": black, or white on a dark background.
    pub auto_color: bool,
    pub u: Underline,
    pub u_color: Option<Rgb>,
    pub strike: bool,
    pub dstrike: bool,
    pub caps: bool,
    pub small_caps: bool,
    pub vanish: bool,
    pub vert: Vert,
    pub position: f32,
    pub spacing: f32,
    pub scale: f32,
    pub kern: Option<f32>,
    pub highlight: Option<Rgb>,
    pub shd: Option<Rgb>,
    /// A border around the text (character border).
    pub bdr: Option<Border>,
    pub rtl: bool,
    pub cs: bool,
}

#[derive(Debug)]
pub struct Table {
    pub align: Align,
    pub indent: f32,
    pub width: Option<Width>,
    pub grid: Vec<f32>,
    pub rows: Vec<Row>,
}

#[derive(Debug)]
pub struct Row {
    pub height: Option<(f32, bool)>,
    pub header: bool,
    pub cant_split: bool,
    pub cells: Vec<Cell>,
}

#[derive(Debug)]
pub struct Cell {
    pub col: usize,
    pub span: usize,
    pub vmerge: Option<VMerge>,
    pub width: Option<Width>,
    /// Top, left, bottom, right, after table borders are applied.
    pub borders: [Option<Border>; 4],
    pub shd: Option<Rgb>,
    pub valign: VAlign,
    /// Top, left, bottom, right.
    pub mar: [f32; 4],
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub struct Media {
    pub bytes: Arc<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geom {
    Rect,
    RoundRect,
    Ellipse,
    Line,
}

#[derive(Debug)]
pub enum DrawKind {
    Picture {
        media: Option<Arc<Media>>,
        /// Fractions cut from left, top, right, bottom.
        crop: [f32; 4],
    },
    Shape {
        geom: Geom,
        fill: Option<Rgb>,
        line: Option<(Rgb, f32)>,
        text: Vec<Block>,
        /// Text insets: left, top, right, bottom.
        ins: [f32; 4],
        valign: VAlign,
    },
    /// A chart, drawn from its cached values.
    Chart(Box<crate::chart::Chart>),
    /// Something we don't draw (an OLE object without preview): its space is kept so
    /// the layout around it stays right.
    Placeholder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HRel {
    Page,
    Margin,
    Column,
    Character,
    LeftMargin,
    RightMargin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VRel {
    Page,
    Margin,
    Paragraph,
    Line,
    TopMargin,
    BottomMargin,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pos {
    Offset(f32),
    /// left/top = 0, center = 1, right/bottom = 2 (inside/outside map to left/right).
    Align(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wrap {
    /// In front of or behind the text.
    None,
    Square,
    TopBottom,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub h_rel: HRel,
    pub h: Pos,
    pub v_rel: VRel,
    pub v: Pos,
    pub wrap: Wrap,
    pub behind: bool,
    /// Distance from text: top, bottom, left, right.
    pub dist: [f32; 4],
}

#[derive(Debug)]
pub struct Drawing {
    pub w: f32,
    pub h: f32,
    pub kind: DrawKind,
    pub anchor: Option<Anchor>,
    /// Alternative text for readers who can't see it (Word's "descr", ODT's svg:desc).
    pub alt: Option<String>,
}
