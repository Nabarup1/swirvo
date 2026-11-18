//! Documents: one model, many readers and writers (docs/05 §3).
//!
//! Every reader (Markdown, HTML, TXT, DOCX, ODT, RTF, EPUB, PDF text, spreadsheets as a
//! table) produces a [`Doc`]; every writer (Markdown, HTML, TXT, DOCX, ODT, EPUB, PDF)
//! consumes one. N readers + M writers give N×M conversions without N×M code paths.
//!
//! What survives: headings, paragraphs, bold/italic/code/links, nested lists, quotes, code
//! blocks, tables, rules, page breaks and pictures (read from DOCX, ODT, RTF, EPUB, PDF and
//! pictures embedded in HTML; written to HTML, embedded in the file, and to EPUB, DOCX, ODT
//! and the typeset PDF). What doesn't: exact layout, fonts and colors, pictures in table
//! cells, and pictures in Markdown and plain text. Each
//! capability says so in its note, and the planner prefers a more faithful engine
//! (LibreOffice; `pdflayout` for PDF → DOCX) when there is one. PDFs are read through
//! `pdflayout` too, so reading order, headings, lists and tables come from layout.
//!
//! Pictures are only ever taken from inside the document (its package, or bytes embedded
//! in it). A picture that links to a file elsewhere on the PC is never loaded.

mod pdf;
mod picture;
mod read;
mod write;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

pub use read::read_path;
pub(crate) use write::esc;
pub use write::write_path;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inline {
    pub text: String,
    pub style: Style,
}

/// A picture, kept as its file's bytes (PNG, JPEG, GIF, WebP, SVG, BMP, TIFF, EMF, WMF…).
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub bytes: std::sync::Arc<Vec<u8>>,
    /// Alternative text from the source, or empty.
    pub alt: String,
    /// Width as placed in the source, in points, when it says.
    pub width_pt: Option<u32>,
}

// By hand: the bytes would flood logs and the EPUB identifier (built from `{:?}`).
impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.bytes.hash(&mut h);
        f.debug_struct("Picture")
            .field(
                "bytes",
                &format_args!("{} bytes #{:016x}", self.bytes.len(), h.finish()),
            )
            .field("alt", &self.alt)
            .field("width_pt", &self.width_pt)
            .finish()
    }
}

/// A picture bigger than this is left out (per picture), and so is anything past
/// [`PICTURES_LIMIT`] in one document.
pub(crate) const PICTURE_LIMIT: u64 = 64 << 20;
pub(crate) const PICTURES_LIMIT: u64 = 512 << 20;

/// A picture in a table cell, shown after the cell's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPicture {
    pub row: usize,
    pub col: usize,
    pub picture: Picture,
}

/// The pictures in cell (`row`, `col`), in order.
pub fn cell_pictures(
    pictures: &[CellPicture],
    row: usize,
    col: usize,
) -> impl Iterator<Item = &Picture> {
    pictures
        .iter()
        .filter(move |p| p.row == row && p.col == col)
        .map(|p| &p.picture)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Para(Vec<Inline>),
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Code(String),
    Table {
        rows: Vec<Vec<String>>,
        header: bool,
        /// Pictures inside cells (cells otherwise hold text).
        pictures: Vec<CellPicture>,
    },
    Rule,
    PageBreak,
    Image(Picture),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    pub title: Option<String>,
    pub blocks: Vec<Block>,
}

pub fn plain(inl: &[Inline]) -> String {
    inl.iter().map(|i| i.text.as_str()).collect()
}

impl Doc {
    /// Title for metadata: the explicit one, else the first heading, else `fallback`.
    pub fn title_or(&self, fallback: &str) -> String {
        if let Some(t) = self.title.as_ref().filter(|t| !t.trim().is_empty()) {
            return t.trim().to_string();
        }
        self.blocks
            .iter()
            .find_map(|b| match b {
                Block::Heading(_, i) => Some(plain(i).trim().to_string()),
                _ => None,
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| fallback.to_string())
    }
}

/// Containers that hold blocks while a reader walks nested structure.
enum Frame {
    Blocks(Vec<Block>),
    Quote(Vec<Block>),
    List {
        ordered: bool,
        items: Vec<Vec<Block>>,
    },
    Item(Vec<Block>),
    Table {
        rows: Vec<Vec<String>>,
        row: Vec<String>,
        header: bool,
        pictures: Vec<CellPicture>,
    },
}

/// Shared by every reader: collects styled text into paragraphs and blocks, with a stack for
/// nesting (lists, quotes, tables) and a flat queue for "level-numbered" lists (DOCX).
pub(crate) struct Builder {
    stack: Vec<Frame>,
    inl: Vec<Inline>,
    styles: Vec<Style>,
    /// DOCX-style list paragraphs: (level, ordered, text), nested on flush.
    flat_list: Vec<(usize, bool, Vec<Inline>)>,
    in_cell: bool,
}

impl Builder {
    pub fn new() -> Builder {
        Builder {
            stack: vec![Frame::Blocks(Vec::new())],
            inl: Vec::new(),
            styles: vec![Style::default()],
            flat_list: Vec::new(),
            in_cell: false,
        }
    }

    fn style(&self) -> Style {
        self.styles.last().cloned().unwrap_or_default()
    }

    pub fn push_style(&mut self, f: impl FnOnce(&mut Style)) {
        let mut s = self.style();
        f(&mut s);
        self.styles.push(s);
    }

    pub fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    /// Append text in the current style, merging with the previous run when it matches.
    pub fn text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        let style = self.style();
        match self.inl.last_mut() {
            Some(last) if last.style == style => last.text.push_str(s),
            _ => self.inl.push(Inline {
                text: s.to_string(),
                style,
            }),
        }
    }

    fn take_inlines(&mut self) -> Vec<Inline> {
        let mut inl = std::mem::take(&mut self.inl);
        // Trim outer whitespace (but keep inner spacing and line breaks).
        if let Some(first) = inl.first_mut() {
            first.text = first
                .text
                .trim_start_matches([' ', '\t', '\n', '\r'])
                .to_string();
        }
        if let Some(last) = inl.last_mut() {
            last.text = last
                .text
                .trim_end_matches([' ', '\t', '\n', '\r'])
                .to_string();
        }
        inl.retain(|i| !i.text.is_empty());
        inl
    }

    fn push_block(&mut self, b: Block) {
        match self.stack.last_mut() {
            Some(Frame::Blocks(v) | Frame::Quote(v) | Frame::Item(v)) => v.push(b),
            Some(Frame::List { items, .. }) => items.push(vec![b]),
            Some(Frame::Table { .. }) | None => {}
        }
    }

    fn flush_flat_list(&mut self) {
        if self.flat_list.is_empty() {
            return;
        }
        let entries = std::mem::take(&mut self.flat_list);
        let mut i = 0;
        while i < entries.len() {
            let (block, next) = nest(&entries, i, entries[i].0);
            self.push_block(block);
            i = next;
        }
    }

    /// End the current paragraph (if it has any text).
    pub fn para(&mut self) {
        if self.in_cell {
            return;
        }
        let inl = self.take_inlines();
        if !inl.is_empty() {
            self.flush_flat_list();
            self.push_block(Block::Para(inl));
        }
    }

    pub fn heading(&mut self, level: u8) {
        let inl = self.take_inlines();
        if !inl.is_empty() {
            self.flush_flat_list();
            self.push_block(Block::Heading(level.clamp(1, 6), inl));
        }
    }

    /// A paragraph that is item `level` of a numbered (`ordered`) or bulleted list.
    pub fn list_para(&mut self, level: usize, ordered: bool) {
        let inl = self.take_inlines();
        if !inl.is_empty() {
            self.flat_list.push((level.min(8), ordered, inl));
        }
    }

    pub fn block(&mut self, b: Block) {
        self.para();
        self.flush_flat_list();
        self.push_block(b);
    }

    /// A picture where the text has got to; in a table cell, it goes after the cell's text.
    pub fn image(&mut self, p: Picture) {
        if !self.in_cell {
            self.block(Block::Image(p));
        } else if let Some(Frame::Table {
            rows,
            row,
            pictures,
            ..
        }) = self.stack.last_mut()
        {
            pictures.push(CellPicture {
                row: rows.len(),
                col: row.len(),
                picture: p,
            });
        }
    }

    pub fn open_quote(&mut self) {
        self.para();
        self.flush_flat_list();
        self.stack.push(Frame::Quote(Vec::new()));
    }

    pub fn close_quote(&mut self) {
        self.para();
        self.flush_flat_list();
        if let Some(Frame::Quote(v)) = self.stack.pop()
            && !v.is_empty()
        {
            self.push_block(Block::Quote(v));
        }
    }

    pub fn open_list(&mut self, ordered: bool) {
        self.para();
        self.flush_flat_list();
        self.stack.push(Frame::List {
            ordered,
            items: Vec::new(),
        });
    }

    pub fn close_list(&mut self) {
        self.para();
        if matches!(self.stack.last(), Some(Frame::Item(_))) {
            self.close_item();
        }
        if let Some(Frame::List { ordered, items }) = self.stack.pop()
            && !items.is_empty()
        {
            self.push_block(Block::List { ordered, items });
        }
    }

    pub fn open_item(&mut self) {
        self.para();
        if matches!(self.stack.last(), Some(Frame::Item(_))) {
            self.close_item();
        }
        if matches!(self.stack.last(), Some(Frame::List { .. })) {
            self.stack.push(Frame::Item(Vec::new()));
        }
    }

    pub fn close_item(&mut self) {
        self.para();
        self.flush_flat_list();
        if matches!(self.stack.last(), Some(Frame::Item(_)))
            && let Some(Frame::Item(v)) = self.stack.pop()
            && let Some(Frame::List { items, .. }) = self.stack.last_mut()
        {
            items.push(v);
        }
    }

    pub fn open_table(&mut self) {
        self.para();
        self.flush_flat_list();
        self.stack.push(Frame::Table {
            rows: Vec::new(),
            row: Vec::new(),
            header: false,
            pictures: Vec::new(),
        });
    }

    pub fn set_table_header(&mut self) {
        if let Some(Frame::Table { header, rows, .. }) = self.stack.last_mut()
            && rows.is_empty()
        {
            *header = true;
        }
    }

    pub fn open_cell(&mut self) {
        self.inl.clear();
        self.in_cell = true;
    }

    pub fn close_cell(&mut self) {
        self.in_cell = false;
        let text = plain(&self.take_inlines());
        if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
            row.push(text.trim().to_string());
        }
    }

    pub fn close_row(&mut self) {
        if let Some(Frame::Table {
            rows,
            row,
            pictures,
            ..
        }) = self.stack.last_mut()
        {
            if row.is_empty() {
                // A row with no cells: pictures noted for it have nowhere to go.
                let r = rows.len();
                pictures.retain(|p| p.row < r);
            } else {
                rows.push(std::mem::take(row));
            }
        }
    }

    pub fn close_table(&mut self) {
        self.close_row();
        if let Some(Frame::Table {
            rows,
            header,
            mut pictures,
            ..
        }) = self.stack.pop()
            && !rows.is_empty()
        {
            pictures.retain(|p| rows.get(p.row).is_some_and(|r| p.col < r.len()));
            self.push_block(Block::Table {
                rows,
                header,
                pictures,
            });
        }
    }

    pub fn in_table_cell(&self) -> bool {
        self.in_cell
    }

    pub fn finish(mut self) -> Vec<Block> {
        self.para();
        self.flush_flat_list();
        while self.stack.len() > 1 {
            match self.stack.last() {
                Some(Frame::Quote(_)) => self.close_quote(),
                Some(Frame::Item(_)) => self.close_item(),
                Some(Frame::List { .. }) => self.close_list(),
                Some(Frame::Table { .. }) => self.close_table(),
                _ => {
                    self.stack.pop();
                }
            }
        }
        match self.stack.pop() {
            Some(Frame::Blocks(v)) => v,
            _ => Vec::new(),
        }
    }
}

impl Default for Builder {
    fn default() -> Self {
        Builder::new()
    }
}

/// Turn level-numbered list paragraphs starting at `i` (all at `level` or deeper) into one
/// nested list. Returns the list and the index after it.
fn nest(entries: &[(usize, bool, Vec<Inline>)], mut i: usize, level: usize) -> (Block, usize) {
    let ordered = entries[i].1;
    let mut items: Vec<Vec<Block>> = Vec::new();
    while i < entries.len() && entries[i].0 >= level {
        if entries[i].0 == level {
            items.push(vec![Block::Para(entries[i].2.clone())]);
            i += 1;
        } else {
            let (sub, next) = nest(entries, i, entries[i].0);
            match items.last_mut() {
                Some(item) => item.push(sub),
                None => items.push(vec![sub]),
            }
            i = next;
        }
    }
    (Block::List { ordered, items }, i)
}

const READ: &[&str] = &[
    "md", "html", "txt", "docx", "odt", "rtf", "epub", "pdf", "csv", "tsv", "xlsx", "xls", "ods",
];
const WRITE: &[&str] = &["md", "html", "txt", "docx", "odt", "epub", "pdf"];

pub struct DocEngine;

impl Engine for DocEngine {
    fn id(&self) -> &'static str {
        "doc"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in READ {
            for &to in WRITE {
                let table = matches!(from, "csv" | "tsv" | "xlsx" | "xls" | "ods");
                // Same format; Markdown → HTML/TXT has a direct engine; a table isn't a book.
                if from == to
                    || (from == "md" && matches!(to, "html" | "txt"))
                    || (table && to == "epub")
                    || (from == "pdf" && to == "docx")
                {
                    continue;
                }
                let cap = Capability::new("doc", from, to)
                    .time(40, if from == "pdf" { 400 } else { 150 });
                // Every writer but plain text carries the source's pictures (Markdown: small ones).
                let pictures = matches!(to, "html" | "epub" | "docx" | "odt" | "pdf" | "md")
                    && matches!(
                        from,
                        "docx" | "odt" | "rtf" | "epub" | "pdf" | "html" | "md"
                    );
                caps.push(match (from, to) {
                    // PDF → DOCX keeps the layout (pdflayout); other targets reflow.
                    ("pdf", _) if pictures => cap.loss(20).note(
                        "headings, lists, tables, emphasis and pictures; the layout isn't kept",
                    ),
                    ("pdf", _) => cap.loss(25).note(
                        "headings, lists, tables and emphasis; layout and images aren't kept",
                    ),
                    (_, "pdf") if matches!(from, "md" | "txt") => {
                        cap.loss(5).note("typeset with system fonts")
                    }
                    (_, "pdf") if table => cap
                        .loss(8)
                        .note("first sheet as a table, typeset with system fonts"),
                    (_, "pdf") if pictures => cap
                        .loss(12)
                        .note("typeset with system fonts, with pictures; the exact layout isn't kept"),
                    (_, "pdf") => cap
                        .loss(15)
                        .note("typeset with system fonts; images and exact layout aren't kept"),
                    _ if table => cap.loss(5).note("first sheet as a table"),
                    ("txt", _) => cap,
                    ("md", _) => cap
                        .loss(3)
                        .note("headings, lists, tables, emphasis, links and pictures"),
                    (_, "md") if pictures => cap.loss(9).note(
                        "headings, lists, tables, bold/italic, links and pictures (up to 256 KB each, inside the file)",
                    ),
                    _ if pictures => cap.loss(8).note(
                        "headings, lists, tables, bold/italic, links and pictures (inside the file)",
                    ),
                    _ => cap
                        .loss(10)
                        .note("headings, lists, tables, bold/italic and links; images aren't kept"),
                });
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        if req.from == "pdf" {
            // Taking the text out is what a "no copying" PDF forbids.
            crate::pdfedit::secure::permits(req.input, crate::pdfedit::secure::Need::Copy)?;
        }
        let doc = read_path(req.input, req.from)?;
        req.cancel.check()?;
        write_path(&doc, req.to, req.output)
    }
}

#[cfg(test)]
mod tests;
