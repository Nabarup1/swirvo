//! The portable page model (docs/13 §3): what analysis produces and writers consume.
//! Units are PDF points; y grows downward from the top of the page.

pub type Rgb = [u8; 3];

pub const BLACK: Rgb = [0, 0, 0];

/// Axis-aligned box. `x0 <= x1` and `y0 <= y1` once built with [`BBox::new`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    /// The identity for [`BBox::union`].
    pub const EMPTY: BBox = BBox {
        x0: f32::INFINITY,
        y0: f32::INFINITY,
        x1: f32::NEG_INFINITY,
        y1: f32::NEG_INFINITY,
    };

    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> BBox {
        BBox {
            x0: x0.min(x1),
            y0: y0.min(y1),
            x1: x0.max(x1),
            y1: y0.max(y1),
        }
    }

    pub fn w(&self) -> f32 {
        (self.x1 - self.x0).max(0.0)
    }

    pub fn h(&self) -> f32 {
        (self.y1 - self.y0).max(0.0)
    }

    pub fn area(&self) -> f32 {
        self.w() * self.h()
    }

    pub fn cx(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    pub fn cy(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }

    /// True when nothing has been added (EMPTY) or the box is inverted.
    pub fn is_void(&self) -> bool {
        !(self.x1 >= self.x0 && self.y1 >= self.y0)
    }

    pub fn union(self, o: BBox) -> BBox {
        BBox {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }

    /// Intersection; inverted (see [`BBox::is_void`]) when the boxes don't meet.
    pub fn intersect(self, o: BBox) -> BBox {
        BBox {
            x0: self.x0.max(o.x0),
            y0: self.y0.max(o.y0),
            x1: self.x1.min(o.x1),
            y1: self.y1.min(o.y1),
        }
    }

    pub fn overlap(&self, o: &BBox) -> f32 {
        let i = self.intersect(*o);
        if i.is_void() { 0.0 } else { i.area() }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    pub fn inflate(self, d: f32) -> BBox {
        BBox {
            x0: self.x0 - d,
            y0: self.y0 - d,
            x1: self.x1 + d,
            y1: self.y1 + d,
        }
    }

    pub fn touches(&self, o: &BBox, d: f32) -> bool {
        !self.inflate(d).intersect(*o).is_void()
    }
}

/// A font as Word should see it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FontInfo {
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    pub serif: bool,
    pub mono: bool,
}

/// An encoded picture for the package (PNG or JPEG).
#[derive(Clone, Debug)]
pub struct Media {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Script {
    #[default]
    Normal,
    Super,
    Sub,
}

/// A run of identically styled text.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub font: u16,
    /// Displayed size in points (a superscript's own size, not its line's).
    pub size: f32,
    pub color: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: Option<Rgb>,
    pub strike: bool,
    pub highlight: Option<Rgb>,
    pub script: Script,
    pub link: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Run(Run),
    Tab,
    /// The page number, as a field (headers and footers).
    PageNumber(Run),
    PageCount(Run),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabStop {
    /// Absolute x on the page.
    pub x: f32,
    pub right: bool,
    /// Dot leader (tables of contents).
    pub dots: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bullet {
    pub glyph: char,
    pub font: u16,
    pub size: f32,
    pub color: Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Para {
    /// Absolute x of the body lines, of the first line, and the right edge.
    pub left: f32,
    pub first: f32,
    pub right: f32,
    pub first_base: f32,
    pub last_base: f32,
    /// Baseline-to-baseline distance.
    pub pitch: f32,
    pub align: Align,
    pub tabs: Vec<TabStop>,
    /// Lines as they were in the PDF; writers that reflow join them.
    pub lines: Vec<Vec<Piece>>,
    pub bullet: Option<Bullet>,
    /// Dominant size and boldness (for headings and styles).
    pub size: f32,
    pub bold: bool,
    pub heading: Option<u8>,
    pub numbered: bool,
}

impl Para {
    /// Top of the first line box, as Word places an exact-height line.
    pub fn top(&self) -> f32 {
        self.first_base - 0.8 * self.pitch
    }

    pub fn bottom(&self) -> f32 {
        self.last_base + 0.2 * self.pitch
    }

    pub fn text(&self) -> String {
        let mut s = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            for p in l {
                match p {
                    Piece::Run(r) | Piece::PageNumber(r) | Piece::PageCount(r) => {
                        s.push_str(&r.text)
                    }
                    Piece::Tab => s.push('\t'),
                }
            }
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Border {
    pub width: f32,
    pub color: Rgb,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Center,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum VMerge {
    #[default]
    None,
    Start,
    Continue,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Cell {
    pub span: usize,
    pub vmerge: VMerge,
    /// Top, left, bottom, right.
    pub borders: [Option<Border>; 4],
    pub fill: Option<Rgb>,
    pub valign: VAlign,
    /// Distance from the cell top to the first line box (for `VAlign::Top`).
    pub pad_top: f32,
    pub paras: Vec<Para>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub height: f32,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub bbox: BBox,
    pub cols: Vec<f32>,
    pub rows: Vec<Row>,
    /// Horizontal text inset inside cells.
    pub pad: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Picture {
    pub media: u32,
    pub bbox: BBox,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Para(Para),
    Table(Table),
    Picture(Picture),
}

impl Block {
    pub fn top(&self) -> f32 {
        match self {
            Block::Para(p) => p.top(),
            Block::Table(t) => t.bbox.y0,
            Block::Picture(p) => p.bbox.y0,
        }
    }

    pub fn bottom(&self) -> f32 {
        match self {
            Block::Para(p) => p.bottom(),
            Block::Table(t) => t.bbox.y1,
            Block::Picture(p) => p.bbox.y1,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub x0: f32,
    pub x1: f32,
    pub blocks: Vec<Block>,
}

/// A horizontal slice of the page: one column, or several side by side.
#[derive(Clone, Debug, PartialEq)]
pub struct Band {
    pub cols: Vec<Column>,
}

impl Band {
    pub fn top(&self) -> f32 {
        self.cols
            .iter()
            .filter_map(|c| c.blocks.first())
            .map(Block::top)
            .fold(f32::INFINITY, f32::min)
    }

    pub fn bottom(&self) -> f32 {
        self.cols
            .iter()
            .filter_map(|c| c.blocks.last())
            .map(Block::bottom)
            .fold(f32::NEG_INFINITY, f32::max)
    }
}

/// Something positioned on the page outside the text flow.
#[derive(Clone, Debug, PartialEq)]
pub enum Float {
    Picture {
        media: u32,
        bbox: BBox,
        behind: bool,
    },
    Rect {
        bbox: BBox,
        fill: Option<Rgb>,
        stroke: Option<Border>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Margins {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub w: f32,
    pub h: f32,
    pub margins: Margins,
    pub bands: Vec<Band>,
    pub floats: Vec<Float>,
    pub header: bool,
    pub footer: bool,
}

/// Repeated page furniture: paragraphs plus where they sit.
#[derive(Clone, Debug, PartialEq)]
pub struct Furniture {
    pub paras: Vec<Para>,
    /// Header: distance from the page top to the first line box.
    /// Footer: distance from the page bottom to the last line box.
    pub offset: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub pages: Vec<Page>,
    pub fonts: Vec<FontInfo>,
    pub media: Vec<Media>,
    pub links: Vec<String>,
    pub header: Option<Furniture>,
    pub footer: Option<Furniture>,
    /// Dominant body font and size.
    pub body_font: u16,
    pub body_size: f32,
    pub title: Option<String>,
    /// False for scans without a text layer (callers fall back to OCR).
    pub has_text: bool,
}
