//! Paragraph text: shaping into atoms, then Word's greedy line breaking.
//!
//! An [`Atom`] is the unit a line can't break inside: a word with its trailing spaces
//! (possibly several runs and fonts), one CJK character, a tab, a break, an inline picture.
//! Lines are built left to right with Word's tab rules (custom stops, the implicit stop at
//! a hanging indent, default stops from the margin; right, center and decimal stops
//! align the text that follows), then measured with the paragraph's line-spacing rule.

use std::sync::Arc;

use super::fonts::Fonts;
use super::model::{BreakKind, FieldKind, Item, Label, Link, Para, ParaStyle, RunStyle};
use super::numbering::{NumFmt, Suffix};
use super::props::{Align, Leader, LineRule, TabKind, Vert};

#[derive(Debug, Clone, Copy)]
pub struct PGlyph {
    pub id: u16,
    /// Byte range of the glyph's cluster in [`Seg::text`].
    pub start: u32,
    pub end: u32,
    /// Advance and offsets in points.
    pub adv: f32,
    pub dx: f32,
    pub dy: f32,
    pub space: bool,
}

/// One shaped run of text in one face.
#[derive(Debug, Clone)]
pub struct Seg {
    pub face: usize,
    pub size: f32,
    /// Baseline shift upwards, in points.
    pub rise: f32,
    pub glyphs: Vec<PGlyph>,
    pub text: String,
    pub width: f32,
    /// Ascent (with external leading) and descent in points, before the rise.
    pub asc: f32,
    pub desc: f32,
    pub rs: Arc<RunStyle>,
    pub link: Option<Arc<Link>>,
}

#[derive(Debug, Clone)]
pub enum Mark {
    Bookmark(String),
    Note(String),
}

#[derive(Debug, Clone)]
pub enum Kind {
    Text {
        word: Vec<Seg>,
        space: Vec<Seg>,
        /// A soft hyphen ends this atom: shown only if the line breaks here.
        hyphen: Option<Seg>,
        label: Option<Align>,
    },
    Tab {
        rs: Arc<RunStyle>,
        label: bool,
    },
    Break(BreakKind),
    Object {
        item: usize,
        w: f32,
        h: f32,
    },
}

#[derive(Debug, Clone)]
pub struct Atom {
    pub kind: Kind,
    pub w: f32,
    pub sw: f32,
    pub marks: Vec<Mark>,
}

/// What lines need from the page they land on.
pub struct Env<'a> {
    pub fonts: &'a mut Fonts,
    pub field: &'a dyn Fn(FieldKind, Option<NumFmt>) -> String,
    pub note_number: &'a dyn Fn(&str) -> String,
    /// Inside a footnote: its number, shown by the footnote's own mark.
    pub note_mark: Option<String>,
}

/// Note marks are superscript unless the document formats them otherwise.
fn superscript(rs: &Arc<RunStyle>) -> Arc<RunStyle> {
    if rs.vert == Vert::Baseline {
        let mut s = (**rs).clone();
        s.vert = Vert::Super;
        Arc::new(s)
    } else {
        rs.clone()
    }
}

fn is_break_space(c: char) -> bool {
    matches!(c, ' ' | '\u{3000}' | '\u{2002}'..='\u{2006}' | '\u{2008}'..='\u{200b}')
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x2E80..=0x2FFF | 0x3040..=0x30FF | 0x3100..=0x31FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FFFF)
}

/// Characters a line may not start with (closing punctuation, small kana).
fn no_break_before(c: char) -> bool {
    matches!(
        c,
        '、' | '。'
            | '，'
            | '．'
            | '：'
            | '；'
            | '？'
            | '！'
            | '）'
            | '」'
            | '』'
            | '】'
            | '〉'
            | '》'
            | '・'
            | 'ー'
            | '々'
            | '…'
            | ','
            | '.'
            | ')'
            | ']'
            | '}'
            | '!'
            | '?'
            | ':'
            | ';'
            | '%'
    )
}

fn is_complex(c: char) -> bool {
    matches!(c as u32,
        0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0E00..=0x0EFF | 0x0F00..=0x0FFF
        | 0x1000..=0x109F | 0x1780..=0x17FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

fn is_rtl(c: char) -> bool {
    matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Word's font slot for a character: ASCII, other Latin, East Asian, complex script.
fn slot(c: char, rs: &RunStyle) -> usize {
    if rs.cs || is_complex(c) {
        3
    } else if is_cjk(c) || matches!(c as u32, 0xAC00..=0xD7AF | 0x1100..=0x11FF) {
        2
    } else if (c as u32) < 0x80 {
        0
    } else {
        1
    }
}

/// Map Symbol/Wingdings private-use code points to Unicode when the font isn't installed.
fn symbol_char(c: char) -> char {
    match c as u32 {
        0xF0B7 | 0xF0A7 | 0xF09F => '•',
        0xF06F => '○',
        0xF0A8 | 0xF071 => '▪',
        0xF0D8 | 0xF0E8 => '➢',
        0xF0FC => '✓',
        0xF0FB => '✗',
        0xF076 => '❖',
        0xF0B2 => '◆',
        0xF02D => '–',
        u @ 0xF020..=0xF07E => char::from_u32(u - 0xF000).unwrap_or(c),
        _ => c,
    }
}

fn segs_for(
    text: &str,
    rs: &Arc<RunStyle>,
    link: &Option<Arc<Link>>,
    fonts: &mut Fonts,
    out: &mut Vec<Seg>,
) {
    if rs.small_caps {
        // Lowercase letters become capitals at 80 %.
        let mut chunk = String::new();
        let mut lower = false;
        for c in text.chars() {
            let l = c.is_lowercase();
            if l != lower && !chunk.is_empty() {
                small_caps_chunk(&chunk, lower, rs, link, fonts, out);
                chunk.clear();
            }
            lower = l;
            chunk.push(c);
        }
        if !chunk.is_empty() {
            small_caps_chunk(&chunk, lower, rs, link, fonts, out);
        }
        return;
    }
    let text = if rs.caps {
        std::borrow::Cow::Owned(text.to_uppercase())
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    shape_run(&text, rs, link, 1.0, fonts, out);
}

fn small_caps_chunk(
    chunk: &str,
    lower: bool,
    rs: &Arc<RunStyle>,
    link: &Option<Arc<Link>>,
    fonts: &mut Fonts,
    out: &mut Vec<Seg>,
) {
    if lower {
        shape_run(&chunk.to_uppercase(), rs, link, 0.8, fonts, out);
    } else {
        shape_run(chunk, rs, link, 1.0, fonts, out);
    }
}

fn shape_run(
    text: &str,
    rs: &Arc<RunStyle>,
    link: &Option<Arc<Link>>,
    size_mul: f32,
    fonts: &mut Fonts,
    out: &mut Vec<Seg>,
) {
    // Group by font slot (whitespace stays with what came before), then by coverage.
    let mut groups: Vec<(usize, String)> = Vec::new();
    for c in text.chars() {
        let s = if c.is_whitespace() || c.is_ascii_punctuation() {
            groups.last().map_or_else(|| slot(c, rs), |g| g.0)
        } else {
            slot(c, rs)
        };
        match groups.last_mut() {
            Some((gs, t)) if *gs == s => t.push(c),
            _ => groups.push((s, c.to_string())),
        }
    }
    for (s, t) in groups {
        let (bold, italic, size) = if s == 3 {
            (rs.b_cs, rs.i_cs, rs.sz_cs)
        } else {
            (rs.b, rs.i, rs.sz)
        };
        let family = &rs.fonts[s];
        let Some(base) = fonts.face(family, bold, italic) else {
            continue;
        };
        let symbolic = matches!(
            family.to_lowercase().as_str(),
            "symbol" | "wingdings" | "wingdings 2" | "wingdings 3" | "webdings"
        );
        let mut sub: Vec<(usize, String)> = Vec::new();
        for c in t.chars() {
            let mut c = c;
            let mut f = fonts.cover(base, c, bold, italic);
            if symbolic && !fonts.get(f).has(c) {
                c = symbol_char(c);
                f = fonts.cover(base, c, bold, italic);
            }
            match sub.last_mut() {
                Some((sf, st)) if *sf == f => st.push(c),
                _ => sub.push((f, c.to_string())),
            }
        }
        for (f, st) in sub {
            out.push(make_seg(f, &st, size * size_mul, rs, link, fonts));
        }
    }
}

fn make_seg(
    face: usize,
    text: &str,
    size: f32,
    rs: &Arc<RunStyle>,
    link: &Option<Arc<Link>>,
    fonts: &mut Fonts,
) -> Seg {
    let (size, rise) = match rs.vert {
        Vert::Baseline => (size, 0.0),
        Vert::Super => (size * 0.65, size * 0.33),
        Vert::Sub => (size * 0.65, -size * 0.12),
    };
    let rise = rise + rs.position;
    let rtl = rs.rtl || text.chars().any(is_rtl);
    let kern = rs.kern.is_some_and(|k| size >= k);
    let shaped = fonts.shape(face, text, rtl, kern);
    let mut clusters: Vec<u32> = shaped.iter().map(|g| g.cluster).collect();
    clusters.sort_unstable();
    clusters.dedup();
    let end_of = |c: u32| -> u32 {
        match clusters.binary_search(&c) {
            Ok(i) => clusters.get(i + 1).copied().unwrap_or(text.len() as u32),
            Err(_) => text.len() as u32,
        }
    };
    let bytes = text.as_bytes();
    let glyphs: Vec<PGlyph> = shaped
        .iter()
        .map(|g| {
            let start = g.cluster.min(text.len() as u32);
            let end = end_of(g.cluster).max(start);
            let space = bytes.get(start as usize) == Some(&b' ');
            PGlyph {
                id: g.id,
                start,
                end,
                adv: g.adv * size * rs.scale + rs.spacing,
                dx: g.dx * size,
                dy: g.dy * size,
                space,
            }
        })
        .collect();
    let width = glyphs.iter().map(|g| g.adv).sum();
    let f = fonts.get(face);
    Seg {
        face,
        size,
        rise,
        glyphs,
        text: text.to_string(),
        width,
        asc: f.asc * size,
        desc: f.desc * size,
        rs: rs.clone(),
        link: link.clone(),
    }
}

/// Text of one run inside an atom: its style, link and characters.
type Part = (Arc<RunStyle>, Option<Arc<Link>>, String);

/// A right, center or decimal tab waiting for the text after it: index of the first
/// item after the tab, x of the tab, the stop, its kind, leader and run style.
type PendingTab = (usize, f32, f32, TabKind, Leader, Arc<RunStyle>);

#[derive(Default)]
struct Pending {
    word: Vec<Part>,
    space: Vec<Part>,
    marks: Vec<Mark>,
    hyphen: Option<Arc<RunStyle>>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.word.is_empty() && self.space.is_empty()
    }
}

fn push_part(parts: &mut Vec<Part>, rs: &Arc<RunStyle>, link: &Option<Arc<Link>>, c: char) {
    match parts.last_mut() {
        Some((r, l, s)) if Arc::ptr_eq(r, rs) && l == link => s.push(c),
        _ => parts.push((rs.clone(), link.clone(), c.to_string())),
    }
}

fn flush(p: &mut Pending, fonts: &mut Fonts, out: &mut Vec<Atom>, label: Option<Align>) {
    if p.is_empty() && p.marks.is_empty() {
        return;
    }
    let mut word = Vec::new();
    for (rs, link, t) in &p.word {
        segs_for(t, rs, link, fonts, &mut word);
    }
    let mut space = Vec::new();
    for (rs, link, t) in &p.space {
        segs_for(t, rs, link, fonts, &mut space);
    }
    let hyphen = p.hyphen.take().and_then(|rs| {
        let mut v = Vec::new();
        segs_for("-", &rs, &None, fonts, &mut v);
        v.pop()
    });
    let w = word.iter().map(|s| s.width).sum();
    let sw = space.iter().map(|s| s.width).sum();
    out.push(Atom {
        kind: Kind::Text {
            word,
            space,
            hyphen,
            label,
        },
        w,
        sw,
        marks: std::mem::take(&mut p.marks),
    });
    p.word.clear();
    p.space.clear();
}

/// Build the atoms of a paragraph (its list label first).
pub fn atoms(p: &Para, env: &mut Env<'_>) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut cur = Pending::default();
    if let Some(Label {
        text,
        rs,
        suffix,
        jc,
    }) = &p.label
    {
        for c in text.chars() {
            push_part(&mut cur.word, rs, &None, c);
        }
        flush(&mut cur, env.fonts, &mut out, Some(*jc));
        match suffix {
            Suffix::Tab => out.push(Atom {
                kind: Kind::Tab {
                    rs: rs.clone(),
                    label: true,
                },
                w: 0.0,
                sw: 0.0,
                marks: Vec::new(),
            }),
            Suffix::Space => {
                if let Some(Atom {
                    kind: Kind::Text { space, .. },
                    sw,
                    ..
                }) = out.last_mut()
                {
                    let mut v = Vec::new();
                    segs_for(" ", rs, &None, env.fonts, &mut v);
                    *sw += v.iter().map(|s| s.width).sum::<f32>();
                    space.extend(v);
                }
            }
            Suffix::Nothing => {}
        }
    }
    // Break after a CJK character or a hyphen, unless the next character forbids it.
    let mut break_after = false;
    let mut prev_alnum = false;
    let text_item = |cur: &mut Pending,
                     out: &mut Vec<Atom>,
                     fonts: &mut Fonts,
                     text: &str,
                     rs: &Arc<RunStyle>,
                     link: &Option<Arc<Link>>,
                     break_after: &mut bool,
                     prev_alnum: &mut bool| {
        for c in text.chars() {
            if c == '\u{ad}' {
                cur.hyphen = Some(rs.clone());
                flush(cur, fonts, out, None);
                *break_after = false;
                continue;
            }
            if is_break_space(c) {
                push_part(&mut cur.space, rs, link, c);
                *break_after = false;
                *prev_alnum = false;
                continue;
            }
            let cjk = is_cjk(c);
            if !cur.space.is_empty()
                || ((*break_after || cjk) && !cur.word.is_empty() && !no_break_before(c))
            {
                flush(cur, fonts, out, None);
            }
            push_part(&mut cur.word, rs, link, c);
            *break_after =
                cjk || (matches!(c, '-' | '\u{2010}' | '\u{2013}' | '\u{2014}') && *prev_alnum);
            *prev_alnum = c.is_alphanumeric();
        }
    };
    for (idx, it) in p.items.iter().enumerate() {
        match it {
            Item::Text { text, rs, link } => text_item(
                &mut cur,
                &mut out,
                env.fonts,
                text,
                rs,
                link,
                &mut break_after,
                &mut prev_alnum,
            ),
            Item::Field { kind, fmt, rs } => {
                let t = (env.field)(*kind, *fmt);
                text_item(
                    &mut cur,
                    &mut out,
                    env.fonts,
                    &t,
                    rs,
                    &None,
                    &mut break_after,
                    &mut prev_alnum,
                );
            }
            Item::NoteRef { id, rs, .. } => {
                let t = (env.note_number)(id);
                let rs = superscript(rs);
                for c in t.chars() {
                    push_part(&mut cur.word, &rs, &None, c);
                }
                cur.marks.push(Mark::Note(id.clone()));
            }
            Item::NoteMark { rs } => {
                if let Some(t) = env.note_mark.clone() {
                    let rs = superscript(rs);
                    for c in t.chars() {
                        push_part(&mut cur.word, &rs, &None, c);
                    }
                }
            }
            Item::Tab { rs } => {
                flush(&mut cur, env.fonts, &mut out, None);
                out.push(Atom {
                    kind: Kind::Tab {
                        rs: rs.clone(),
                        label: false,
                    },
                    w: 0.0,
                    sw: 0.0,
                    marks: Vec::new(),
                });
                break_after = false;
            }
            Item::Break(kind) => {
                flush(&mut cur, env.fonts, &mut out, None);
                out.push(Atom {
                    kind: Kind::Break(*kind),
                    w: 0.0,
                    sw: 0.0,
                    marks: Vec::new(),
                });
                break_after = false;
            }
            Item::Drawing(d) => {
                if d.anchor.is_none() {
                    flush(&mut cur, env.fonts, &mut out, None);
                    out.push(Atom {
                        kind: Kind::Object {
                            item: idx,
                            w: d.w,
                            h: d.h,
                        },
                        w: d.w,
                        sw: 0.0,
                        marks: Vec::new(),
                    });
                }
            }
            Item::Bookmark(b) => cur.marks.push(Mark::Bookmark(b.clone())),
        }
    }
    flush(&mut cur, env.fonts, &mut out, None);
    out
}

// ---------------------------------------------------------------- lines

#[derive(Debug, Clone)]
pub enum Placed {
    Seg {
        x: f32,
        seg: Seg,
    },
    Leader {
        x0: f32,
        x1: f32,
        kind: Leader,
        rs: Arc<RunStyle>,
    },
    Object {
        x: f32,
        item: usize,
        w: f32,
        h: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    Wrap,
    Forced(BreakKind),
    Para,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub items: Vec<Placed>,
    /// Right edge of the content, trailing spaces excluded.
    pub right: f32,
    pub height: f32,
    /// Baseline position from the line's top.
    pub baseline: f32,
    pub end: End,
    pub marks: Vec<Mark>,
    /// Placed items from this index on stretch when justifying (after the last tab).
    pub just_from: usize,
}

#[derive(Clone)]
pub struct Breaker<'a> {
    atoms: Vec<Atom>,
    pub pos: usize,
    p: &'a ParaStyle,
    default_tab: f32,
    /// Metrics of the paragraph mark: an empty line's height.
    mark_asc: f32,
    mark_desc: f32,
}

impl<'a> Breaker<'a> {
    pub fn new(
        atoms: Vec<Atom>,
        p: &'a ParaStyle,
        mark: &RunStyle,
        fonts: &mut Fonts,
        default_tab: f32,
    ) -> Breaker<'a> {
        let (mark_asc, mark_desc) = fonts
            .face(&mark.fonts[0], mark.b, mark.i)
            .map(|f| {
                let face = fonts.get(f);
                (face.asc * mark.sz, face.desc * mark.sz)
            })
            .unwrap_or((mark.sz * 0.9, mark.sz * 0.25));
        Breaker {
            atoms,
            pos: 0,
            p,
            default_tab,
            mark_asc,
            mark_desc,
        }
    }

    pub fn done(&self) -> bool {
        self.pos >= self.atoms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.atoms.is_empty()
    }

    /// The next tab stop after `x` (column coordinates), with its kind and leader.
    fn stop(&self, x: f32, first: bool, limit: f32) -> (f32, TabKind, Leader) {
        let eps = 0.05;
        let mut best: Option<(f32, TabKind, Leader)> = None;
        for t in &self.p.tabs {
            if t.kind == TabKind::Bar || t.pos <= x + eps {
                continue;
            }
            best = Some((t.pos, t.kind, t.leader));
            break;
        }
        // A hanging indent is an implicit stop on the first line.
        if first && self.p.first < 0.0 && x + eps < self.p.left {
            let hang = self.p.left;
            if best.is_none_or(|b| hang < b.0) {
                best = Some((hang, TabKind::Left, Leader::None));
            }
        }
        if let Some(b) = best {
            return b;
        }
        let d = self.default_tab.max(1.0);
        let next = ((x + eps) / d).floor() * d + d;
        let _ = limit;
        (next, TabKind::Left, Leader::None)
    }

    /// Break the next line between `x0` (where it starts) and `limit`, both in column
    /// coordinates.
    pub fn next_line(&mut self, x0: f32, limit: f32, first: bool) -> Line {
        let mut x = x0;
        let mut items: Vec<Placed> = Vec::new();
        let mut marks: Vec<Mark> = Vec::new();
        let mut has_content = false;
        let mut end = End::Para;
        let mut last_space = 0.0;
        let mut just_from = 0usize;
        // A right/center/decimal tab waiting for the text after it.
        let mut pending: Option<PendingTab> = None;
        let mut hyphen_at_end: Option<Seg> = None;
        let resolve = |items: &mut Vec<Placed>,
                       pending: &mut Option<PendingTab>,
                       x: &mut f32,
                       trailing: f32| {
            if let Some((idx, tab_x, stop, kind, leader, rs)) = pending.take() {
                let group = (*x - trailing) - tab_x;
                let dec = if kind == TabKind::Decimal {
                    decimal_offset(&items[idx..]).unwrap_or(group)
                } else {
                    group
                };
                let start = match kind {
                    TabKind::Right => stop - group,
                    TabKind::Center => stop - group / 2.0,
                    _ => stop - dec,
                };
                let delta = (start - tab_x).max(0.0);
                for it in &mut items[idx..] {
                    shift(it, delta);
                }
                if leader != Leader::None && delta > 0.0 {
                    items.insert(
                        idx,
                        Placed::Leader {
                            x0: tab_x,
                            x1: tab_x + delta,
                            kind: leader,
                            rs,
                        },
                    );
                }
                *x += delta;
            }
        };
        while self.pos < self.atoms.len() {
            let a = &self.atoms[self.pos];
            match &a.kind {
                Kind::Text {
                    word, space, label, ..
                } => {
                    let w = a.w;
                    let room = limit - x;
                    if has_content && w > room + 0.01 {
                        end = End::Wrap;
                        break;
                    }
                    if !has_content
                        && w > (limit - x0) + 0.01
                        && word.len() + space.len() > 0
                        && label.is_none()
                    {
                        // Wider than the whole line: break inside the word.
                        if let Some((head, tail)) = split_atom(a, (limit - x).max(1.0)) {
                            self.atoms[self.pos] = tail;
                            self.atoms.insert(self.pos, head);
                            continue;
                        }
                    }
                    let a = &self.atoms[self.pos];
                    let Kind::Text {
                        word,
                        space,
                        hyphen,
                        label,
                    } = &a.kind
                    else {
                        unreachable!("checked above")
                    };
                    let mut sx = x;
                    if let Some(j) = label {
                        // List labels align on the first-line start.
                        sx = match j {
                            Align::Right => x - a.w,
                            Align::Center => x - a.w / 2.0,
                            _ => x,
                        };
                    }
                    for s in word.iter().chain(space.iter()) {
                        items.push(Placed::Seg {
                            x: sx,
                            seg: s.clone(),
                        });
                        sx += s.width;
                    }
                    x = sx;
                    last_space = a.sw;
                    marks.extend(a.marks.iter().cloned());
                    hyphen_at_end = hyphen.clone();
                    has_content = true;
                    self.pos += 1;
                    let _ = (word, space);
                }
                Kind::Tab { rs, label } => {
                    resolve(&mut items, &mut pending, &mut x, 0.0);
                    let (stop, kind, leader) = self.stop(x, first, limit);
                    let rs = rs.clone();
                    let _ = label;
                    match kind {
                        TabKind::Left | TabKind::Bar | TabKind::Clear => {
                            if leader != Leader::None && stop > x {
                                items.push(Placed::Leader {
                                    x0: x,
                                    x1: stop,
                                    kind: leader,
                                    rs,
                                });
                            }
                            x = stop;
                        }
                        _ => pending = Some((items.len(), x, stop, kind, leader, rs)),
                    }
                    last_space = 0.0;
                    has_content = true;
                    just_from = items.len();
                    hyphen_at_end = None;
                    self.pos += 1;
                }
                Kind::Break(k) => {
                    marks.extend(a.marks.iter().cloned());
                    end = End::Forced(*k);
                    self.pos += 1;
                    break;
                }
                Kind::Object { item, w, h } => {
                    if has_content && x + w > limit + 0.01 {
                        end = End::Wrap;
                        break;
                    }
                    items.push(Placed::Object {
                        x,
                        item: *item,
                        w: *w,
                        h: *h,
                    });
                    x += w;
                    last_space = 0.0;
                    has_content = true;
                    hyphen_at_end = None;
                    self.pos += 1;
                }
            }
        }
        if self.pos >= self.atoms.len() && end == End::Wrap {
            end = End::Para;
        }
        resolve(&mut items, &mut pending, &mut x, last_space);
        let mut right = x - last_space;
        if end == End::Wrap
            && let Some(h) = hyphen_at_end
        {
            let w = h.width;
            items.push(Placed::Seg { x: right, seg: h });
            right += w;
        }
        let (height, baseline) = self.measure(&items);
        Line {
            items,
            right,
            height,
            baseline,
            end,
            marks,
            just_from,
        }
    }

    /// Line height and baseline under the paragraph's spacing rule. Extra space from
    /// multiple or exact spacing goes above the text, as in Word.
    fn measure(&self, items: &[Placed]) -> (f32, f32) {
        let mut asc: f32 = 0.0;
        let mut desc: f32 = 0.0;
        let mut obj: f32 = 0.0;
        let mut any = false;
        for it in items {
            match it {
                Placed::Seg { seg, .. } => {
                    any = true;
                    asc = asc.max(seg.asc + seg.rise.max(0.0));
                    desc = desc.max(seg.desc - seg.rise.min(0.0));
                }
                Placed::Object { h, .. } => obj = obj.max(*h),
                Placed::Leader { .. } => {}
            }
        }
        if !any {
            asc = self.mark_asc;
            desc = self.mark_desc;
        }
        let natural = asc + desc;
        let full = asc.max(obj) + desc;
        let h = match self.p.line {
            LineRule::Auto(m) => (natural * m).max(full),
            LineRule::Exact(v) => v,
            LineRule::AtLeast(v) => v.max(full),
        };
        (h, h - desc)
    }
}

fn shift(it: &mut Placed, dx: f32) {
    match it {
        Placed::Seg { x, .. } | Placed::Object { x, .. } => *x += dx,
        Placed::Leader { x0, x1, .. } => {
            *x0 += dx;
            *x1 += dx;
        }
    }
}

/// Width from the start of `items` to the first decimal separator.
fn decimal_offset(items: &[Placed]) -> Option<f32> {
    let x0 = match items.first()? {
        Placed::Seg { x, .. } | Placed::Object { x, .. } => *x,
        Placed::Leader { x0, .. } => *x0,
    };
    for it in items {
        if let Placed::Seg { x, seg } = it {
            let mut gx = *x;
            for g in &seg.glyphs {
                let ch = seg.text.get(g.start as usize..g.end as usize).unwrap_or("");
                if ch.starts_with('.') || ch.starts_with(',') {
                    return Some(gx - x0);
                }
                gx += g.adv;
            }
        }
    }
    None
}

/// Split a text atom so its head fits `room` (at least one cluster).
fn split_atom(a: &Atom, room: f32) -> Option<(Atom, Atom)> {
    let Kind::Text { word, space, .. } = &a.kind else {
        return None;
    };
    let mut head: Vec<Seg> = Vec::new();
    let mut tail: Vec<Seg> = Vec::new();
    let mut used = 0.0;
    let mut cut = false;
    for s in word {
        if cut {
            tail.push(s.clone());
            continue;
        }
        if used + s.width <= room {
            used += s.width;
            head.push(s.clone());
            continue;
        }
        // Split this segment at a cluster boundary.
        let mut k = 0;
        let mut w = 0.0;
        while k < s.glyphs.len() {
            let end_cluster = s.glyphs[k].start;
            let mut j = k;
            let mut cw = 0.0;
            while j < s.glyphs.len() && s.glyphs[j].start == end_cluster {
                cw += s.glyphs[j].adv;
                j += 1;
            }
            if used + w + cw > room && (k > 0 || !head.is_empty()) {
                break;
            }
            w += cw;
            k = j;
        }
        if k == 0 || k >= s.glyphs.len() {
            if k >= s.glyphs.len() {
                used += s.width;
                head.push(s.clone());
            } else {
                tail.push(s.clone());
            }
            cut = true;
            continue;
        }
        let byte = s.glyphs[k].start as usize;
        let mk = |glyphs: &[PGlyph], text: &str, off: u32| -> Seg {
            let gl: Vec<PGlyph> = glyphs
                .iter()
                .map(|g| PGlyph {
                    start: g.start - off,
                    end: g.end.saturating_sub(off).max(g.start - off),
                    ..*g
                })
                .collect();
            Seg {
                width: gl.iter().map(|g| g.adv).sum(),
                glyphs: gl,
                text: text.to_string(),
                ..s.clone()
            }
        };
        let (t1, t2) = s.text.split_at(byte.min(s.text.len()));
        head.push(mk(&s.glyphs[..k], t1, 0));
        tail.push(mk(&s.glyphs[k..], t2, byte as u32));
        cut = true;
    }
    if head.is_empty() || (tail.is_empty() && space.is_empty()) {
        return None;
    }
    let hw = head.iter().map(|s| s.width).sum();
    let tw = tail.iter().map(|s| s.width).sum();
    Some((
        Atom {
            kind: Kind::Text {
                word: head,
                space: Vec::new(),
                hyphen: None,
                label: None,
            },
            w: hw,
            sw: 0.0,
            marks: a.marks.clone(),
        },
        Atom {
            kind: Kind::Text {
                word: tail,
                space: space.clone(),
                hyphen: match &a.kind {
                    Kind::Text { hyphen, .. } => hyphen.clone(),
                    _ => None,
                },
                label: None,
            },
            w: tw,
            sw: a.sw,
            marks: Vec::new(),
        },
    ))
}
