//! RTF → the layout model, so RTF gets the same page layout as DOCX.
//!
//! A single pass over the token stream with a stack of group states. Word writes full
//! direct formatting on every paragraph, so the style sheet isn't needed to look right.

use std::collections::HashMap;
use std::sync::Arc;

use swirvo_core::EngineError;

use super::model::*;
use super::numbering::Suffix;
use super::props::{
    Align, Border, BorderStyle, Leader, LineRule, Rgb, TabDef, TabKind, Underline, VAlign, VMerge,
    Vert, Width,
};

const TW: f32 = 20.0; // twips per point

#[derive(Debug, Clone, PartialEq)]
enum Tok<'a> {
    Open,
    Close,
    /// Control word and optional numeric parameter.
    Word(&'a str, Option<i32>),
    /// A control symbol (`\~`, `\-`, `\*`, `\'hh` gives the byte).
    Sym(char),
    Byte(u8),
    Text(&'a [u8]),
}

fn tokens(src: &[u8]) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        match src[i] {
            b'{' => {
                out.push(Tok::Open);
                i += 1;
            }
            b'}' => {
                out.push(Tok::Close);
                i += 1;
            }
            b'\\' => {
                i += 1;
                let Some(&c) = src.get(i) else { break };
                if c.is_ascii_alphabetic() {
                    let s = i;
                    while i < src.len() && src[i].is_ascii_alphabetic() && i - s < 32 {
                        i += 1;
                    }
                    let word = std::str::from_utf8(&src[s..i]).unwrap_or("");
                    let ns = i;
                    if i < src.len() && (src[i] == b'-' || src[i].is_ascii_digit()) {
                        i += 1;
                        while i < src.len() && src[i].is_ascii_digit() && i - ns < 11 {
                            i += 1;
                        }
                    }
                    let num = std::str::from_utf8(&src[ns..i])
                        .ok()
                        .and_then(|n| n.parse::<i32>().ok());
                    if i < src.len() && src[i] == b' ' {
                        i += 1;
                    }
                    out.push(Tok::Word(word, num));
                } else if c == b'\'' {
                    let h = src
                        .get(i + 1..i + 3)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok());
                    out.push(Tok::Byte(h.unwrap_or(b'?')));
                    i += 3;
                } else if c == b'\r' || c == b'\n' {
                    out.push(Tok::Word("par", None));
                    i += 1;
                } else {
                    out.push(Tok::Sym(c as char));
                    i += 1;
                }
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                let s = i;
                while i < src.len() && !matches!(src[i], b'{' | b'}' | b'\\' | b'\r' | b'\n') {
                    i += 1;
                }
                out.push(Tok::Text(&src[s..i]));
            }
        }
    }
    out
}

/// Windows-1252 for bytes 0x80–0x9F; Latin-1 otherwise.
pub(crate) fn cp1252(b: u8) -> char {
    const HI: [u16; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013,
        0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    if (0x80..0xA0).contains(&b) {
        char::from_u32(u32::from(HI[(b - 0x80) as usize])).unwrap_or('?')
    } else {
        b as char
    }
}

#[derive(Debug, Clone)]
struct Chr {
    font: i32,
    sz: f32,
    b: bool,
    i: bool,
    u: bool,
    strike: bool,
    color: i32,
    highlight: i32,
    vert: Vert,
    caps: bool,
    hidden: bool,
}

impl Default for Chr {
    fn default() -> Self {
        Chr {
            font: 0,
            sz: 12.0,
            b: false,
            i: false,
            u: false,
            strike: false,
            color: 0,
            highlight: 0,
            vert: Vert::Baseline,
            caps: false,
            hidden: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct Par {
    align: Align,
    li: f32,
    ri: f32,
    fi: f32,
    sb: f32,
    sa: f32,
    sl: Option<(i32, bool)>,
    keep_next: bool,
    page_before: bool,
    in_table: bool,
    tabs: Vec<TabDef>,
    pending_tab: (Option<TabKind>, Leader),
    shd: i32,
}

/// Where text goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dest {
    Body,
    Skip,
    FontTable,
    ColorTable,
    Info(InfoField),
    Pict,
    FieldInst,
    FieldResult,
    ListText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InfoField {
    None,
    Title,
    Author,
    Subject,
    Keywords,
}

#[derive(Debug, Clone)]
struct State {
    chr: Chr,
    par: Par,
    dest: Dest,
    uc: usize,
}

/// Cell definitions of the current table row.
#[derive(Debug, Clone, Default)]
struct CellDef {
    right: f32,
    borders: [Option<Border>; 4],
    shd: i32,
    vmerge: Option<VMerge>,
    valign: VAlign,
}

/// A sink for paragraphs: the body, a header/footer or a footnote.
#[derive(Default)]
struct Flow {
    blocks: Vec<Block>,
    items: Vec<Item>,
    label: Option<String>,
    // Table under construction.
    rows: Vec<Row>,
    cell_blocks: Vec<Vec<Block>>,
    defs: Vec<CellDef>,
    row_left: f32,
    row_height: Option<(f32, bool)>,
    header_row: bool,
    /// Cell borders: the side being defined (0 top, 1 left, 2 bottom, 3 right).
    def_side: Option<usize>,
    cur_def: CellDef,
}

struct Rd {
    fonts: HashMap<i32, (String, Generic)>,
    colors: Vec<Option<Rgb>>,
    flows: Vec<Flow>,
    /// Text being collected for the current destination (font names, field codes…).
    scratch: String,
    font_num: Option<i32>,
    font_family: Generic,
    color_parts: [u8; 3],
    color_set: bool,
    pict: (Vec<u8>, String, f32, f32, f32, f32, i32, i32),
    link: Option<Arc<Link>>,
    field_inst: String,
    doc: Document,
    sect: SectPr,
    notes: usize,
    pending_border: Option<Border>,
    border_target: BorderTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorderTarget {
    None,
    Cell,
}

impl Rd {
    fn flow(&mut self) -> &mut Flow {
        if self.flows.is_empty() {
            self.flows.push(Flow::default());
        }
        let n = self.flows.len() - 1;
        &mut self.flows[n]
    }

    fn color(&self, i: i32) -> Option<Rgb> {
        self.colors.get(i as usize).copied().flatten()
    }

    fn run_style(&self, c: &Chr) -> RunStyle {
        let font = self
            .fonts
            .get(&c.font)
            .map_or("Times New Roman".to_string(), |f| f.0.clone());
        let color = if c.color > 0 {
            self.color(c.color)
        } else {
            None
        };
        RunStyle {
            fonts: [font.clone(), font.clone(), font.clone(), font],
            b: c.b,
            i: c.i,
            b_cs: c.b,
            i_cs: c.i,
            sz: c.sz,
            sz_cs: c.sz,
            color: color.unwrap_or(Rgb(0, 0, 0)),
            auto_color: color.is_none(),
            u: if c.u {
                Underline::Single
            } else {
                Underline::None
            },
            u_color: None,
            strike: c.strike,
            dstrike: false,
            caps: c.caps,
            small_caps: false,
            vanish: c.hidden,
            vert: c.vert,
            position: 0.0,
            spacing: 0.0,
            scale: 1.0,
            kern: None,
            highlight: if c.highlight > 0 {
                self.color(c.highlight)
            } else {
                None
            },
            shd: None,
            bdr: None,
            rtl: false,
            cs: false,
        }
    }

    fn text(&mut self, s: &str, st: &State) {
        match st.dest {
            Dest::Body | Dest::FieldResult => {
                if s.is_empty() || st.chr.hidden {
                    return;
                }
                let rs = Arc::new(self.run_style(&st.chr));
                let link = self.link.clone();
                let items = &mut self.flow().items;
                if let Some(Item::Text {
                    text,
                    rs: prs,
                    link: pl,
                }) = items.last_mut()
                    && **prs == *rs
                    && *pl == link
                {
                    text.push_str(s);
                    return;
                }
                items.push(Item::Text {
                    text: s.to_string(),
                    rs,
                    link,
                });
            }
            Dest::ListText => {
                let f = self.flow();
                f.label.get_or_insert_with(String::new).push_str(s);
            }
            Dest::FontTable | Dest::Info(_) | Dest::FieldInst => self.scratch.push_str(s),
            _ => {}
        }
    }

    fn end_para(&mut self, st: &State) {
        let rs = Arc::new(self.run_style(&st.chr));
        let p = &st.par;
        let line = match p.sl {
            Some((v, true)) if v > 0 => LineRule::Auto(v as f32 / 240.0),
            Some((v, false)) if v > 0 => LineRule::AtLeast(v as f32 / TW),
            Some((v, false)) if v < 0 => LineRule::Exact(-v as f32 / TW),
            _ => LineRule::Auto(1.0),
        };
        let shd = if p.shd > 0 { self.color(p.shd) } else { None };
        let f = self.flow();
        let items = std::mem::take(&mut f.items);
        let label = f.label.take().map(|t| {
            let t = t.trim_end_matches('\t').to_string();
            Label {
                text: t,
                rs: rs.clone(),
                suffix: Suffix::Tab,
                jc: Align::Left,
            }
        });
        let mut tabs = p.tabs.clone();
        if label.is_some() && p.fi < 0.0 && tabs.iter().all(|t| t.pos != p.li) {
            tabs.push(TabDef {
                pos: p.li,
                kind: TabKind::Left,
                leader: Leader::None,
            });
        }
        let para = Para {
            p: ParaStyle {
                align: p.align,
                left: p.li,
                right: p.ri,
                first: p.fi,
                before: p.sb,
                after: p.sa,
                line,
                contextual: false,
                keep_next: p.keep_next,
                keep_lines: false,
                page_break_before: p.page_before,
                widow: true,
                tabs,
                shd,
                borders: [None; 5],
                bidi: false,
                style: String::new(),
            },
            mark: rs,
            items,
            label,
            outline: None,
        };
        if p.in_table {
            let f = self.flow();
            if f.cell_blocks.is_empty() {
                f.cell_blocks.push(Vec::new());
            }
            let n = f.cell_blocks.len() - 1;
            f.cell_blocks[n].push(Block::Para(Box::new(para)));
        } else {
            self.close_table();
            self.flow().blocks.push(Block::Para(Box::new(para)));
        }
    }

    fn end_cell(&mut self) {
        let f = self.flow();
        if !f.items.is_empty() {
            // A cell's last paragraph ends with \cell, not \par.
            let items = std::mem::take(&mut f.items);
            let mark = items
                .iter()
                .find_map(|i| match i {
                    Item::Text { rs, .. } => Some(rs.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| Arc::new(RunStyle::plain()));
            let para = Para {
                p: ParaStyle::plain(),
                mark,
                items,
                label: None,
                outline: None,
            };
            if f.cell_blocks.is_empty() {
                f.cell_blocks.push(Vec::new());
            }
            let n = f.cell_blocks.len() - 1;
            f.cell_blocks[n].push(Block::Para(Box::new(para)));
        }
        f.cell_blocks.push(Vec::new());
    }

    fn end_row(&mut self) {
        let shades: Vec<Option<Rgb>> = self
            .flow()
            .defs
            .iter()
            .map(|d| d.shd)
            .collect::<Vec<_>>()
            .into_iter()
            .map(|s| if s > 0 { self.color(s) } else { None })
            .collect();
        let f = self.flow();
        let mut blocks = std::mem::take(&mut f.cell_blocks);
        // The trailing empty cell opened by the last \cell.
        if blocks.last().is_some_and(Vec::is_empty) && blocks.len() > f.defs.len() {
            blocks.pop();
        }
        let mut cells = Vec::new();
        let mut contents = blocks.into_iter();
        for (i, d) in f.defs.iter().enumerate() {
            cells.push(Cell {
                col: i,
                span: 1,
                vmerge: d.vmerge,
                width: None,
                borders: d.borders,
                shd: shades.get(i).copied().flatten(),
                valign: d.valign,
                mar: [0.0, 5.4, 0.0, 5.4],
                blocks: contents.next().unwrap_or_default(),
            });
        }
        f.rows.push(Row {
            height: f.row_height,
            header: f.header_row,
            cant_split: false,
            cells,
        });
    }

    fn close_table(&mut self) {
        let f = self.flow();
        if f.rows.is_empty() {
            return;
        }
        // Grid from the last row's cell boundaries.
        let mut grid = Vec::new();
        let mut x = f.row_left;
        for d in &f.defs {
            grid.push((d.right - x).max(4.0));
            x = d.right;
        }
        let rows = std::mem::take(&mut f.rows);
        let ncols = grid.len();
        let rows = rows
            .into_iter()
            .map(|mut r| {
                r.cells.truncate(ncols);
                r
            })
            .collect();
        f.blocks.push(Block::Table(Box::new(Table {
            align: Align::Left,
            indent: f.row_left,
            width: Some(Width::Pt(grid.iter().sum())),
            grid,
            rows,
        })));
    }
}

impl RunStyle {
    fn plain() -> RunStyle {
        RunStyle {
            fonts: std::array::from_fn(|_| "Times New Roman".to_string()),
            b: false,
            i: false,
            b_cs: false,
            i_cs: false,
            sz: 12.0,
            sz_cs: 12.0,
            color: Rgb(0, 0, 0),
            auto_color: true,
            u: Underline::None,
            u_color: None,
            strike: false,
            dstrike: false,
            caps: false,
            small_caps: false,
            vanish: false,
            vert: Vert::Baseline,
            position: 0.0,
            spacing: 0.0,
            scale: 1.0,
            kern: None,
            highlight: None,
            shd: None,
            bdr: None,
            rtl: false,
            cs: false,
        }
    }
}

impl ParaStyle {
    fn plain() -> ParaStyle {
        ParaStyle {
            align: Align::Left,
            left: 0.0,
            right: 0.0,
            first: 0.0,
            before: 0.0,
            after: 0.0,
            line: LineRule::Auto(1.0),
            contextual: false,
            keep_next: false,
            keep_lines: false,
            page_break_before: false,
            widow: true,
            tabs: Vec::new(),
            shd: None,
            borders: [None; 5],
            bidi: false,
            style: String::new(),
        }
    }
}

fn hex_bytes(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
    let (pairs, _) = digits.as_chunks::<2>();
    pairs
        .iter()
        .filter_map(|p| u8::from_str_radix(std::str::from_utf8(p).ok()?, 16).ok())
        .collect()
}

pub fn read(bytes: &[u8]) -> Result<Document, EngineError> {
    if !bytes.starts_with(b"{\\rtf") {
        return Err(EngineError::Decode("not an RTF file".into()));
    }
    let toks = tokens(bytes);
    let mut rd = Rd {
        fonts: HashMap::new(),
        colors: Vec::new(),
        flows: vec![Flow::default()],
        scratch: String::new(),
        font_num: None,
        font_family: Generic::Serif,
        color_parts: [0; 3],
        color_set: false,
        pict: (Vec::new(), String::new(), 0.0, 0.0, 0.0, 0.0, 100, 100),
        link: None,
        field_inst: String::new(),
        doc: Document {
            default_tab: 36.0,
            expand_shift_return: true,
            ..Document::default()
        },
        sect: SectPr {
            w: 612.0,
            h: 792.0,
            left: 90.0,
            right: 90.0,
            ..SectPr::default()
        },
        notes: 0,
        pending_border: None,
        border_target: BorderTarget::None,
    };
    let mut stack: Vec<State> = vec![State {
        chr: Chr::default(),
        par: Par::default(),
        dest: Dest::Body,
        uc: 1,
    }];
    // Groups that started a new flow (header, footer, footnote), by depth.
    let mut flow_groups: Vec<(usize, &str)> = Vec::new();
    let mut skip_chars = 0usize;
    let mut ignorable_next = false;
    let mut i = 0;
    while i < toks.len() {
        let depth = stack.len();
        let tok = toks[i].clone();
        i += 1;
        // Characters skipped after \u (the ANSI fallback).
        if skip_chars > 0 {
            match tok {
                Tok::Text(t) => {
                    let n = t.len().min(skip_chars);
                    skip_chars -= n;
                    if n < t.len() {
                        let st = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                        let rest: String = t[n..].iter().map(|&b| cp1252(b)).collect();
                        rd.text(&rest, &st);
                    }
                    continue;
                }
                Tok::Byte(_) => {
                    skip_chars -= 1;
                    continue;
                }
                _ => skip_chars = 0,
            }
        }
        match tok {
            Tok::Open => {
                let top = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                stack.push(top);
            }
            Tok::Close => {
                let Some(closed) = stack.pop() else { break };
                if stack.is_empty() {
                    break;
                }
                // Finish the destination this group was.
                match closed.dest {
                    Dest::FontTable if stack.last().is_some_and(|s| s.dest == Dest::FontTable) => {
                        if let Some(n) = rd.font_num.take() {
                            let name = rd.scratch.trim().trim_end_matches(';').trim().to_string();
                            rd.doc
                                .generics
                                .insert(name.to_ascii_lowercase(), rd.font_family);
                            rd.fonts.insert(n, (name, rd.font_family));
                        }
                        rd.scratch.clear();
                    }
                    Dest::Info(f) => {
                        let v = std::mem::take(&mut rd.scratch).trim().to_string();
                        if !v.is_empty() {
                            match f {
                                InfoField::Title => rd.doc.meta.title = Some(v),
                                InfoField::Author => rd.doc.meta.author = Some(v),
                                InfoField::Subject => rd.doc.meta.subject = Some(v),
                                InfoField::Keywords => rd.doc.meta.keywords = Some(v),
                                InfoField::None => {}
                            }
                        }
                    }
                    Dest::Pict if stack.last().is_none_or(|s| s.dest != Dest::Pict) => {
                        let (hex, kind, w, h, gw, gh, sx, sy) = std::mem::take(&mut rd.pict);
                        let bytes = hex_bytes(&String::from_utf8_lossy(&hex));
                        let (gw, gh) =
                            (if gw > 0.0 { gw } else { w }, if gh > 0.0 { gh } else { h });
                        let dw = gw * sx as f32 / 100.0;
                        let dh = gh * sy as f32 / 100.0;
                        let media = (match kind.as_str() {
                            "png" | "jpeg" => image::guess_format(&bytes).is_ok(),
                            "meta" => crate::emf::is_metafile(&bytes),
                            _ => false,
                        })
                        .then(|| {
                            Arc::new(Media {
                                bytes: Arc::new(bytes),
                            })
                        });
                        if dw > 0.0 && dh > 0.0 {
                            rd.flow().items.push(Item::Drawing(Box::new(Drawing {
                                w: dw,
                                h: dh,
                                kind: match media {
                                    Some(m) => DrawKind::Picture {
                                        media: Some(m),
                                        crop: [0.0; 4],
                                    },
                                    None => DrawKind::Placeholder,
                                },
                                anchor: None,
                                alt: None,
                            })));
                        }
                    }
                    Dest::FieldInst if stack.last().is_none_or(|s| s.dest != Dest::FieldInst) => {
                        let inst = std::mem::take(&mut rd.scratch);
                        rd.field_inst = inst.trim().to_string();
                        let up = rd.field_inst.to_ascii_uppercase();
                        if up.starts_with("HYPERLINK") {
                            let url = rd.field_inst.split('"').nth(1).unwrap_or("").to_string();
                            if !url.is_empty() {
                                rd.link = Some(Arc::new(Link::Url(url)));
                            }
                        }
                    }
                    Dest::FieldResult
                        if stack.last().is_none_or(|s| s.dest != Dest::FieldResult) =>
                    {
                        rd.link = None;
                    }
                    _ => {}
                }
                // A header, footer or footnote group ends.
                if let Some(&(d, kind)) = flow_groups.last()
                    && d == depth
                {
                    flow_groups.pop();
                    let st = closed.clone();
                    if !rd.flow().items.is_empty() {
                        rd.end_para(&st);
                    }
                    rd.close_table();
                    let f = rd.flows.pop().unwrap_or_default();
                    match kind {
                        "footnote" => {
                            rd.notes += 1;
                            let id = format!("rtf{}", rd.notes);
                            let mut blocks = f.blocks;
                            if let Some(Block::Para(p)) = blocks.first_mut() {
                                let mut sup = (*p.mark).clone();
                                sup.vert = Vert::Super;
                                p.items.insert(0, Item::NoteMark { rs: Arc::new(sup) });
                            }
                            rd.doc.footnotes.insert(id.clone(), blocks);
                            let mut sup = rd.run_style(
                                &stack.last().map(|s| s.chr.clone()).unwrap_or_default(),
                            );
                            sup.vert = Vert::Super;
                            rd.flow().items.push(Item::NoteRef {
                                id,
                                endnote: false,
                                rs: Arc::new(sup),
                            });
                        }
                        k => {
                            let id = format!("rtf-{k}");
                            rd.doc.parts.insert(id.clone(), f.blocks);
                            let slot = match k {
                                "headerf" | "footerf" => 1,
                                "headerl" | "footerl" => 2,
                                _ => 0,
                            };
                            if k.starts_with("header") {
                                rd.sect.headers[slot] = Some(id);
                            } else {
                                rd.sect.footers[slot] = Some(id);
                            }
                            if slot == 1 {
                                rd.sect.title_pg = true;
                            }
                        }
                    }
                }
            }
            Tok::Sym(c) => {
                let st = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                match c {
                    '*' => ignorable_next = true,
                    '~' => rd.text("\u{a0}", &st),
                    '_' => rd.text("\u{2011}", &st),
                    '-' => {}
                    '\\' | '{' | '}' => rd.text(&c.to_string(), &st),
                    _ => {}
                }
            }
            Tok::Byte(b) => {
                let st = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                if st.dest == Dest::Pict {
                    continue;
                }
                rd.text(&cp1252(b).to_string(), &st);
            }
            Tok::Text(t) => {
                let Some(st) = stack.last().cloned() else {
                    continue;
                };
                match st.dest {
                    Dest::Pict => rd.pict.0.extend_from_slice(t),
                    Dest::ColorTable => {
                        for &b in t {
                            if b == b';' {
                                rd.colors.push(rd.color_set.then(|| {
                                    Rgb(rd.color_parts[0], rd.color_parts[1], rd.color_parts[2])
                                }));
                                rd.color_parts = [0; 3];
                                rd.color_set = false;
                            }
                        }
                    }
                    _ => {
                        let s: String = t.iter().map(|&b| cp1252(b)).collect();
                        rd.text(&s, &st);
                    }
                }
            }
            Tok::Word(w, n) => {
                let ignorable = std::mem::take(&mut ignorable_next);
                let Some(st) = stack.last_mut() else { break };
                let v = n.unwrap_or(1);
                let pt = |x: i32| x as f32 / TW;
                // Destinations.
                match w {
                    "fonttbl" => {
                        st.dest = Dest::FontTable;
                        continue;
                    }
                    "colortbl" => {
                        st.dest = Dest::ColorTable;
                        continue;
                    }
                    "info" => {
                        st.dest = Dest::Info(InfoField::None);
                        continue;
                    }
                    "title" | "author" | "subject" | "keywords"
                        if matches!(st.dest, Dest::Info(_)) =>
                    {
                        st.dest = Dest::Info(match w {
                            "title" => InfoField::Title,
                            "author" => InfoField::Author,
                            "subject" => InfoField::Subject,
                            _ => InfoField::Keywords,
                        });
                        rd.scratch.clear();
                        continue;
                    }
                    "pict" => {
                        st.dest = Dest::Pict;
                        rd.pict = (Vec::new(), String::new(), 0.0, 0.0, 0.0, 0.0, 100, 100);
                        continue;
                    }
                    "fldinst" => {
                        st.dest = Dest::FieldInst;
                        rd.scratch.clear();
                        continue;
                    }
                    "fldrslt" => {
                        st.dest = Dest::FieldResult;
                        let up = rd.field_inst.to_ascii_uppercase();
                        let kind = if up.starts_with("PAGE") && !up.starts_with("PAGEREF") {
                            Some(FieldKind::Page)
                        } else if up.starts_with("NUMPAGES") {
                            Some(FieldKind::NumPages)
                        } else {
                            None
                        };
                        if let Some(k) = kind {
                            let rs = Arc::new(rd.run_style(&st.chr));
                            if k == FieldKind::NumPages {
                                rd.doc.uses_numpages = true;
                            }
                            rd.flow().items.push(Item::Field {
                                kind: k,
                                fmt: None,
                                rs,
                            });
                            // The cached result is replaced by the live field.
                            if let Some(s) = stack.last_mut() {
                                s.dest = Dest::Skip;
                            }
                        }
                        continue;
                    }
                    "listtext" | "pntext" => {
                        st.dest = Dest::ListText;
                        continue;
                    }
                    "header" | "footer" | "headerl" | "headerr" | "headerf" | "footerl"
                    | "footerr" | "footerf" | "footnote" => {
                        let kind: &str = match w {
                            "headerr" => "header",
                            "footerr" => "footer",
                            other => other,
                        };
                        st.dest = Dest::Body;
                        st.par = Par::default();
                        flow_groups.push((depth, kind));
                        rd.flows.push(Flow::default());
                        continue;
                    }
                    "stylesheet" | "listtable" | "listoverridetable" | "revtbl" | "rsidtbl"
                    | "generator" | "xmlnstbl" | "latentstyles" | "datastore" | "themedata"
                    | "colorschememapping" | "pgdsctbl" | "object" | "shp" | "shpinst"
                    | "nonshppict" | "bkmkstart" | "bkmkend" | "annotation" | "atnid"
                    | "atnauthor" | "comment" | "docvar" | "userprops" | "mmathPr" | "pnseclvl"
                    | "fldtype" | "ftnsep" | "ftnsepc" | "aftnsep" | "aftnsepc" => {
                        st.dest = Dest::Skip;
                        continue;
                    }
                    _ if ignorable && st.dest != Dest::Pict => {
                        // An unknown \* destination: skip the whole group.
                        if !matches!(w, "shppict" | "fldinst") {
                            st.dest = Dest::Skip;
                        }
                        continue;
                    }
                    _ => {}
                }
                if st.dest == Dest::Skip {
                    continue;
                }
                if st.dest == Dest::FontTable {
                    match w {
                        "f" => {
                            rd.font_num = Some(v);
                            rd.scratch.clear();
                        }
                        "froman" => rd.font_family = Generic::Serif,
                        "fswiss" => rd.font_family = Generic::Sans,
                        "fmodern" => rd.font_family = Generic::Mono,
                        _ => {}
                    }
                    continue;
                }
                if st.dest == Dest::ColorTable {
                    match w {
                        "red" => {
                            rd.color_parts[0] = v as u8;
                            rd.color_set = true;
                        }
                        "green" => {
                            rd.color_parts[1] = v as u8;
                            rd.color_set = true;
                        }
                        "blue" => {
                            rd.color_parts[2] = v as u8;
                            rd.color_set = true;
                        }
                        _ => {}
                    }
                    continue;
                }
                if st.dest == Dest::Pict {
                    match w {
                        "pngblip" => rd.pict.1 = "png".into(),
                        "jpegblip" => rd.pict.1 = "jpeg".into(),
                        "emfblip" | "wmetafile" => rd.pict.1 = "meta".into(),
                        "picw" => rd.pict.2 = v as f32 * 0.75,
                        "pich" => rd.pict.3 = v as f32 * 0.75,
                        "picwgoal" => rd.pict.4 = pt(v),
                        "pichgoal" => rd.pict.5 = pt(v),
                        "picscalex" => rd.pict.6 = v,
                        "picscaley" => rd.pict.7 = v,
                        "bin" => {} // binary pictures aren't supported; their hex never arrives
                        _ => {}
                    }
                    continue;
                }
                let c = &mut st.chr;
                let p = &mut st.par;
                let on = n.is_none_or(|x| x != 0);
                match w {
                    // Characters.
                    "plain" => *c = Chr::default(),
                    "f" => c.font = v,
                    "fs" => c.sz = (v as f32 / 2.0).max(1.0),
                    "b" => c.b = on,
                    "i" => c.i = on,
                    "ul" => c.u = on,
                    "ulnone" => c.u = false,
                    "strike" => c.strike = on,
                    "cf" => c.color = v,
                    "highlight" | "chcbpat" => c.highlight = v,
                    "super" => c.vert = Vert::Super,
                    "sub" => c.vert = Vert::Sub,
                    "nosupersub" => c.vert = Vert::Baseline,
                    "caps" => c.caps = on,
                    "v" => c.hidden = on,
                    "uc" => st.uc = v.max(0) as usize,
                    "u" => {
                        let code = if v < 0 { v + 65_536 } else { v } as u32;
                        if let Some(ch) = char::from_u32(code) {
                            let s = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                            rd.text(&ch.to_string(), &s);
                        }
                        skip_chars = stack.last().map_or(1, |s| s.uc);
                    }
                    // Paragraphs.
                    "pard" => {
                        let in_table = false;
                        *p = Par {
                            in_table,
                            ..Par::default()
                        };
                    }
                    "ql" => p.align = Align::Left,
                    "qc" => p.align = Align::Center,
                    "qr" => p.align = Align::Right,
                    "qj" => p.align = Align::Both,
                    "li" => p.li = pt(v),
                    "ri" => p.ri = pt(v),
                    "fi" => p.fi = pt(v),
                    "sb" => p.sb = pt(v),
                    "sa" => p.sa = pt(v),
                    "sl" => p.sl = Some((v, p.sl.is_some_and(|s| s.1))),
                    "slmult" => {
                        if let Some(s) = &mut p.sl {
                            s.1 = v == 1;
                        }
                    }
                    "keepn" => p.keep_next = true,
                    "pagebb" => p.page_before = true,
                    "intbl" => p.in_table = true,
                    "cbpat" => p.shd = v,
                    "tqr" => p.pending_tab.0 = Some(TabKind::Right),
                    "tqc" => p.pending_tab.0 = Some(TabKind::Center),
                    "tqdec" => p.pending_tab.0 = Some(TabKind::Decimal),
                    "tldot" => p.pending_tab.1 = Leader::Dot,
                    "tlhyph" => p.pending_tab.1 = Leader::Hyphen,
                    "tlul" => p.pending_tab.1 = Leader::Underscore,
                    "tx" => {
                        let (kind, leader) = std::mem::take(&mut p.pending_tab);
                        p.tabs.push(TabDef {
                            pos: pt(v),
                            kind: kind.unwrap_or(TabKind::Left),
                            leader,
                        });
                    }
                    "par" => {
                        let s = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                        rd.end_para(&s);
                    }
                    "line" => rd.flow().items.push(Item::Break(BreakKind::Line)),
                    "page" => rd.flow().items.push(Item::Break(BreakKind::Page)),
                    "column" => rd.flow().items.push(Item::Break(BreakKind::Column)),
                    "tab" => {
                        let rs = Arc::new(rd.run_style(c));
                        rd.flow().items.push(Item::Tab { rs });
                    }
                    "emdash" | "endash" | "bullet" | "lquote" | "rquote" | "ldblquote"
                    | "rdblquote" => {
                        let ch = match w {
                            "emdash" => "—",
                            "endash" => "–",
                            "bullet" => "•",
                            "lquote" => "‘",
                            "rquote" => "’",
                            "ldblquote" => "“",
                            _ => "”",
                        };
                        let s = stack.last().cloned().unwrap_or_else(|| stack[0].clone());
                        rd.text(ch, &s);
                    }
                    "chftn" => {}
                    // Tables.
                    "trowd" => {
                        let f = rd.flow();
                        f.defs.clear();
                        f.cur_def = CellDef::default();
                        f.row_left = 0.0;
                        f.row_height = None;
                        f.header_row = false;
                        rd.border_target = BorderTarget::None;
                    }
                    "trleft" => rd.flow().row_left = pt(v),
                    "trrh" => {
                        rd.flow().row_height = (v != 0).then(|| (pt(v.abs()), v < 0));
                    }
                    "trhdr" => rd.flow().header_row = true,
                    "clbrdrt" | "clbrdrl" | "clbrdrb" | "clbrdrr" => {
                        let side = match w {
                            "clbrdrt" => 0,
                            "clbrdrl" => 1,
                            "clbrdrb" => 2,
                            _ => 3,
                        };
                        rd.flow().def_side = Some(side);
                        rd.border_target = BorderTarget::Cell;
                        rd.pending_border = Some(Border {
                            style: BorderStyle::Single,
                            width: 0.5,
                            color: Rgb(0, 0, 0),
                            space: 0.0,
                        });
                        let pending = rd.pending_border;
                        let f = rd.flow();
                        if let (Some(s), Some(b)) = (f.def_side, pending) {
                            f.cur_def.borders[s] = Some(b);
                        }
                    }
                    "brdrw" | "brdrcf" | "brdrdb" | "brdrdot" | "brdrdash" | "brdrnone"
                    | "brdrs" | "brdrth" => {
                        if rd.border_target == BorderTarget::Cell {
                            let color = rd.color(v);
                            let f = rd.flow();
                            if let Some(s) = f.def_side
                                && let Some(b) = &mut f.cur_def.borders[s]
                            {
                                match w {
                                    "brdrw" => b.width = pt(v).max(0.25),
                                    "brdrcf" => b.color = color.unwrap_or(Rgb(0, 0, 0)),
                                    "brdrdb" => b.style = BorderStyle::Double,
                                    "brdrdot" => b.style = BorderStyle::Dotted,
                                    "brdrdash" => b.style = BorderStyle::Dashed,
                                    "brdrnone" => f.cur_def.borders[s] = None,
                                    _ => {}
                                }
                            }
                        }
                    }
                    "clcbpat" => rd.flow().cur_def.shd = v,
                    "clvmgf" => rd.flow().cur_def.vmerge = Some(VMerge::Restart),
                    "clvmrg" => rd.flow().cur_def.vmerge = Some(VMerge::Continue),
                    "clvertalc" => rd.flow().cur_def.valign = VAlign::Center,
                    "clvertalb" => rd.flow().cur_def.valign = VAlign::Bottom,
                    "cellx" => {
                        let f = rd.flow();
                        let mut d = std::mem::take(&mut f.cur_def);
                        d.right = pt(v);
                        f.defs.push(d);
                        f.def_side = None;
                        rd.border_target = BorderTarget::None;
                    }
                    "cell" => rd.end_cell(),
                    "row" => rd.end_row(),
                    // Page setup.
                    "paperw" => rd.sect.w = pt(v),
                    "paperh" => rd.sect.h = pt(v),
                    "margl" => rd.sect.left = pt(v),
                    "margr" => rd.sect.right = pt(v),
                    "margt" => rd.sect.top = pt(v),
                    "margb" => rd.sect.bottom = pt(v),
                    "headery" => rd.sect.header = pt(v),
                    "footery" => rd.sect.footer = pt(v),
                    "deftab" => rd.doc.default_tab = pt(v).max(1.0),
                    "titlepg" => rd.sect.title_pg = true,
                    _ => {}
                }
            }
        }
    }
    // Whatever is left in the body.
    let st = stack.first().cloned().unwrap_or(State {
        chr: Chr::default(),
        par: Par::default(),
        dest: Dest::Body,
        uc: 1,
    });
    while rd.flows.len() > 1 {
        rd.flows.pop();
    }
    if !rd.flow().items.is_empty() {
        rd.end_para(&st);
    }
    rd.close_table();
    let w = rd.sect.w - rd.sect.left - rd.sect.right;
    rd.sect.cols = Cols {
        cols: vec![(w.max(36.0), 0.0)],
        sep: false,
    };
    let blocks = std::mem::take(&mut rd.flow().blocks);
    rd.doc.sections.push(Section {
        sect: rd.sect.clone(),
        blocks,
    });
    Ok(rd.doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_formatting_tables_and_unicode() {
        let rtf = br#"{\rtf1\ansi\ansicpg1252\deff0{\fonttbl{\f0\froman Times New Roman;}{\f1\fswiss Arial;}}{\colortbl;\red255\green0\blue0;}
{\info{\title Test doc}{\author A. Writer}}
\paperw11906\paperh16838\margl1440\margr1440
\pard\qc\b\f1\fs32 Title\par
\pard\plain\f0\fs24 Plain, {\i italic}, {\cf1 red} and caf\'e9 \u8364?.\par
\trowd\cellx3000\cellx6000\pard\intbl A1\cell B1\cell\row
\trowd\cellx3000\cellx6000\pard\intbl A2\cell B2\cell\row
\pard After the table.\par}"#;
        let d = read(rtf).unwrap();
        assert_eq!(d.meta.title.as_deref(), Some("Test doc"));
        let s = &d.sections[0];
        assert!((s.sect.w - 595.3).abs() < 0.1);
        let texts: Vec<String> = s
            .blocks
            .iter()
            .map(|b| match b {
                Block::Para(p) => p
                    .items
                    .iter()
                    .filter_map(|i| match i {
                        Item::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect(),
                Block::Table(t) => format!("table {}x{}", t.rows.len(), t.grid.len()),
            })
            .collect();
        assert_eq!(texts[0], "Title");
        assert_eq!(texts[1], "Plain, italic, red and café €.");
        assert_eq!(texts[2], "table 2x2");
        assert_eq!(texts[3], "After the table.");
        let Block::Para(p) = &s.blocks[1] else {
            panic!()
        };
        let red = p.items.iter().find_map(|i| match i {
            Item::Text { text, rs, .. } if text == "red" => Some(rs.color),
            _ => None,
        });
        assert_eq!(red, Some(Rgb(255, 0, 0)));
    }
}
