//! Text (docs/13 §4.1): glyphs → rows (one baseline each, scripts folded in) → segments (word
//! runs without big gaps; the atoms of reading order) → lines → styled pieces.

use super::extract::RawGlyph;
use super::model::{BBox, FontInfo, Piece, Run, Script, TabStop};
use std::collections::HashMap;

/// Glyphs on one baseline, sorted by x.
#[derive(Clone, Debug)]
pub struct TRow {
    pub base: f32,
    pub size: f32,
    pub ids: Vec<usize>,
}

/// A run of glyphs in one row with no gap wider than ~1 em.
#[derive(Clone, Debug)]
pub struct TSeg {
    pub row: usize,
    pub ids: Vec<usize>,
    pub bbox: BBox,
    pub base: f32,
    pub size: f32,
}

/// Segments of one row inside one region.
#[derive(Clone, Debug)]
pub struct TLine {
    pub segs: Vec<usize>,
    pub base: f32,
    pub size: f32,
    pub x0: f32,
    pub x1: f32,
}

/// A list marker at the start of a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Marker {
    pub bullet: Option<char>,
    /// Non-space glyphs that make up the marker.
    pub n: usize,
    /// Where the item's text starts.
    pub text_x: f32,
}

/// Drop glyphs drawn twice in the same place. An offset or stroked repeat is how producers
/// fake bold, so the survivor becomes bold; visible text wins over invisible.
pub fn dedupe(g: &mut Vec<RawGlyph>) {
    let mut grid: HashMap<(char, i32, i32), Vec<usize>> = HashMap::new();
    let mut keep = vec![true; g.len()];
    for i in 0..g.len() {
        if g[i].ch == ' ' {
            continue;
        }
        let (cx, cy) = (g[i].x0.floor() as i32, g[i].base.floor() as i32);
        let mut dup = None;
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                if let Some(v) = grid.get(&(g[i].ch, cx + dx, cy + dy)) {
                    for &j in v {
                        let tol = 0.12 * g[j].size;
                        if (g[j].x0 - g[i].x0).abs() <= tol
                            && (g[j].base - g[i].base).abs() <= tol
                            && (g[j].size - g[i].size).abs() <= 0.05 * g[j].size
                        {
                            dup = Some(j);
                            break 'search;
                        }
                    }
                }
            }
        }
        match dup {
            Some(j) => {
                keep[i] = false;
                if g[j].invisible && !g[i].invisible {
                    let bold = g[j].bold;
                    g[j] = g[i].clone();
                    g[j].bold |= bold;
                } else if !g[i].invisible
                    && (g[i].stroked != g[j].stroked
                        || (g[j].x0 - g[i].x0).abs() > 0.01 * g[j].size
                        || (g[j].base - g[i].base).abs() > 0.01 * g[j].size)
                {
                    g[j].bold = true;
                }
            }
            None => grid.entry((g[i].ch, cx, cy)).or_default().push(i),
        }
    }
    let mut k = keep.into_iter();
    g.retain(|_| k.next().unwrap_or(true));
}

fn row_stats(g: &[RawGlyph], ids: &[usize]) -> (f32, f32) {
    let size = ids
        .iter()
        .filter(|&&i| g[i].ch != ' ')
        .map(|&i| g[i].size)
        .fold(0.0, f32::max);
    let mut bases: Vec<f32> = ids
        .iter()
        .filter(|&&i| g[i].ch != ' ' && g[i].size >= 0.85 * size)
        .map(|&i| g[i].base)
        .collect();
    bases.sort_by(f32::total_cmp);
    let base = bases
        .get(bases.len() / 2)
        .copied()
        .unwrap_or(g[ids[0]].base);
    (base, size.max(1.0))
}

/// Group glyphs into rows: one baseline each, with super- and subscripts folded into the line
/// they belong to.
pub fn rows(g: &[RawGlyph], ids: &[usize]) -> Vec<TRow> {
    let mut v: Vec<usize> = ids.to_vec();
    v.sort_by(|&a, &b| g[a].base.total_cmp(&g[b].base));
    let mut groups: Vec<(f32, f32, Vec<usize>)> = Vec::new(); // (anchor base, size, ids)
    for i in v {
        let gi = &g[i];
        match groups.last_mut() {
            Some((anchor, size, ids)) if (gi.base - *anchor).abs() <= 0.22 * gi.size.max(*size) => {
                *size = size.max(gi.size);
                ids.push(i);
            }
            _ => groups.push((gi.base, gi.size, vec![i])),
        }
    }
    let mut rows: Vec<TRow> = groups
        .into_iter()
        .map(|(_, _, mut ids)| {
            ids.sort_by(|&a, &b| g[a].x0.total_cmp(&g[b].x0));
            let (base, size) = row_stats(g, &ids);
            TRow { base, size, ids }
        })
        .collect();
    fold_scripts(g, &mut rows);
    rows.retain(|r| !r.ids.is_empty());
    rows
}

/// Merge rows of small glyphs that sit just above or below a bigger row, next to its text.
fn fold_scripts(g: &[RawGlyph], rows: &mut [TRow]) {
    let n = rows.len();
    for i in 0..n {
        let (rb, rs) = (rows[i].base, rows[i].size);
        let (x0, x1) = span(g, &rows[i].ids);
        let mut target = None;
        // Scripts are short: exponents, footnote marks, formula indices.
        let n = rows[i].ids.iter().filter(|&&k| g[k].ch != ' ').count();
        if n == 0 || n > 6 {
            continue;
        }
        for j in [i.wrapping_sub(1), i + 1, i.wrapping_sub(2), i + 2] {
            let Some(d) = rows.get(j) else { continue };
            if j == i || d.ids.is_empty() || d.size < 1.15 * rs {
                continue;
            }
            // Superscripts sit up to half an em higher, subscripts about a third lower.
            let up = d.base - rb;
            if !(up > 0.0 && up <= 0.5 * d.size || up < 0.0 && -up <= 0.35 * d.size) {
                continue;
            }
            // Touching the bigger row's text: right after a glyph, or right before one.
            let near = d.ids.iter().any(|&k| {
                let gk = &g[k];
                gk.ch != ' '
                    && ((x0 - gk.x1) >= -0.3 * d.size && (x0 - gk.x1) <= 0.6 * d.size
                        || (gk.x0 - x1) >= -0.3 * d.size && (gk.x0 - x1) <= 0.6 * d.size)
            });
            if near {
                target = Some(j);
                break;
            }
        }
        if let Some(j) = target {
            let mut moved = std::mem::take(&mut rows[i].ids);
            rows[j].ids.append(&mut moved);
            let ids = &mut rows[j].ids;
            ids.sort_by(|&a, &b| g[a].x0.total_cmp(&g[b].x0));
        }
    }
}

fn span(g: &[RawGlyph], ids: &[usize]) -> (f32, f32) {
    ids.iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), &i| {
            (a.min(g[i].x0), b.max(g[i].x1))
        })
}

/// Split rows into segments at gaps wider than 0.9 em.
pub fn segments(g: &[RawGlyph], rows: &[TRow]) -> Vec<TSeg> {
    let mut out = Vec::new();
    for (ri, r) in rows.iter().enumerate() {
        let mut cur: Vec<usize> = Vec::new();
        let mut last_x1 = f32::NEG_INFINITY;
        let mut last_size = 0.0f32;
        let flush = |cur: &mut Vec<usize>, out: &mut Vec<TSeg>| {
            while cur.last().is_some_and(|&i| g[i].ch == ' ') {
                cur.pop();
            }
            if cur.is_empty() {
                return;
            }
            let ids = std::mem::take(cur);
            let (x0, x1) = span(g, &ids);
            let size = ids
                .iter()
                .filter(|&&i| g[i].ch != ' ' && (g[i].base - r.base).abs() <= 0.12 * r.size)
                .map(|&i| g[i].size)
                .fold(0.0, f32::max);
            let size = if size > 0.0 { size } else { r.size };
            out.push(TSeg {
                row: ri,
                bbox: BBox::new(x0, r.base - 0.78 * size, x1, r.base + 0.22 * size),
                base: r.base,
                size,
                ids,
            });
        };
        for &i in &r.ids {
            let gi = &g[i];
            if gi.ch == ' ' {
                if !cur.is_empty() {
                    cur.push(i);
                }
                continue;
            }
            let em = gi.size.max(last_size);
            if !cur.is_empty() && gi.x0 - last_x1 > 0.9 * em {
                flush(&mut cur, &mut out);
            }
            cur.push(i);
            last_x1 = last_x1.max(gi.x1);
            last_size = gi.size;
        }
        flush(&mut cur, &mut out);
    }
    out
}

/// Group a region's segments into lines (one per row), top to bottom.
pub fn lines(segs: &[TSeg], ids: &[usize]) -> Vec<TLine> {
    let mut by_row: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for &s in ids {
        by_row.entry(segs[s].row).or_default().push(s);
    }
    let mut out: Vec<TLine> = by_row
        .into_values()
        .map(|mut v| {
            v.sort_by(|&a, &b| segs[a].bbox.x0.total_cmp(&segs[b].bbox.x0));
            let x0 = v
                .iter()
                .map(|&s| segs[s].bbox.x0)
                .fold(f32::INFINITY, f32::min);
            let x1 = v
                .iter()
                .map(|&s| segs[s].bbox.x1)
                .fold(f32::NEG_INFINITY, f32::max);
            let size = v.iter().map(|&s| segs[s].size).fold(0.0, f32::max);
            TLine {
                base: segs[v[0]].base,
                size,
                x0,
                x1,
                segs: v,
            }
        })
        .collect();
    out.sort_by(|a, b| a.base.total_cmp(&b.base));
    out
}

fn bullet_char(c: char) -> Option<char> {
    Some(match c {
        '•' | '\u{F0B7}' | '\u{2219}' => '•',
        '◦' | '\u{F0A8}' => '◦',
        '▪' | '\u{F0A7}' => '▪',
        '❖' | '\u{F076}' => '❖',
        '➢' | '\u{F0D8}' => '➢',
        '✓' | '✔' | '\u{F0FC}' => '✓',
        '·' | '‣' | '●' | '○' | '■' | '□' | '◆' | '◇' | '►' | '▶' | '➤' | '▫' | '–' | '-' | '*' => {
            c
        }
        _ => return None,
    })
}

fn line_glyphs(g: &[RawGlyph], segs: &[TSeg], line: &TLine) -> Vec<usize> {
    line.segs
        .iter()
        .flat_map(|&s| segs[s].ids.iter().copied())
        .filter(|&i| g[i].ch != ' ')
        .collect()
}

/// A bullet or number at the start of the line, followed by a visible gap.
pub fn marker(g: &[RawGlyph], segs: &[TSeg], line: &TLine) -> Option<Marker> {
    let ids = line_glyphs(g, segs, line);
    let first = *ids.first()?;
    let size = line.size;
    if let Some(b) = bullet_char(g[first].ch) {
        let next = *ids.get(1)?;
        let gap = g[next].x0 - g[first].x1;
        let need = if matches!(b, '-' | '–' | '*') {
            0.3
        } else {
            0.12
        };
        return (gap >= need * size).then_some(Marker {
            bullet: Some(b),
            n: 1,
            text_x: g[next].x0,
        });
    }
    // The first token: glyphs up to the first visible gap.
    let mut k = 1;
    while k < ids.len() && k < 8 && g[ids[k]].x0 - g[ids[k - 1]].x1 < 0.2 * size {
        k += 1;
    }
    let next = *ids.get(k)?;
    let s: String = ids[..k].iter().map(|&i| g[i].ch).collect();
    is_number_marker(&s).then_some(Marker {
        bullet: None,
        n: k,
        text_x: g[next].x0,
    })
}

/// `1.` `12)` `a.` `iv)` `(b)` `2.1` `2.1.` — but not words or years.
fn is_number_marker(s: &str) -> bool {
    fn simple(i: &str) -> bool {
        let numeric = !i.is_empty() && i.len() <= 3 && i.chars().all(|c| c.is_ascii_digit());
        let letter = i.len() == 1 && i.chars().all(|c| c.is_ascii_alphabetic());
        let roman = !i.is_empty()
            && i.len() <= 6
            && (i.chars().all(|c| "ivxlcdm".contains(c))
                || i.chars().all(|c| "IVXLCDM".contains(c)));
        numeric || letter || roman
    }
    fn dotted(i: &str) -> bool {
        i.contains('.')
            && i.split('.')
                .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
    }
    if let Some(r) = s.strip_prefix('(') {
        return r.strip_suffix(')').is_some_and(simple);
    }
    if let Some(i) = s.strip_suffix(['.', ')']) {
        return simple(i) || dotted(i);
    }
    dotted(s)
}

fn style(g: &RawGlyph, fonts: &[FontInfo], base: f32, size: f32) -> Run {
    let f = fonts.get(g.font as usize);
    let small = g.size < 0.85 * size;
    let script = if small && g.base < base - 0.12 * size {
        Script::Super
    } else if small && g.base > base + 0.08 * size {
        Script::Sub
    } else {
        Script::Normal
    };
    Run {
        text: String::new(),
        font: g.font,
        size: (g.size * 2.0).round() / 2.0,
        color: g.color,
        bold: f.is_some_and(|f| f.bold) || g.bold,
        italic: f.is_some_and(|f| f.italic) || g.italic,
        underline: g.underline,
        strike: g.strike,
        highlight: g.highlight,
        script,
        link: g.link,
    }
}

fn same_style(a: &Run, b: &Run) -> bool {
    a.font == b.font
        && (a.size - b.size).abs() < 0.26
        && a.color == b.color
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strike == b.strike
        && a.highlight == b.highlight
        && a.script == b.script
        && a.link == b.link
}

fn push_char(out: &mut Vec<Piece>, ch: char, st: Run) {
    if let Some(Piece::Run(r)) = out.last_mut()
        && same_style(r, &st)
    {
        r.text.push(ch);
        return;
    }
    let mut r = st;
    r.text.push(ch);
    out.push(Piece::Run(r));
}

fn push_space(out: &mut [Piece]) {
    if let Some(Piece::Run(r)) = out.last_mut()
        && !r.text.ends_with(' ')
    {
        r.text.push(' ');
    }
}

fn is_dot(c: char) -> bool {
    matches!(c, '.' | '·' | '…' | '․')
}

/// Styled pieces for one line, plus the tab stops its gaps need. `right_edge` is the region's
/// right edge, for right-aligned last segments (page numbers, amounts).
/// Columns of a form or a borderless table: x positions where segments (other than line
/// starts) begin, or short ones end, on two or more lines of a region.
#[derive(Clone, Debug, Default)]
pub struct Aligned {
    pub starts: Vec<f32>,
    pub ends: Vec<f32>,
}

fn repeated(mut xs: Vec<f32>) -> Vec<f32> {
    xs.sort_by(f32::total_cmp);
    let mut out = Vec::new();
    let mut i = 0;
    while i < xs.len() {
        let mut j = i + 1;
        while j < xs.len() && xs[j] - xs[i] <= 1.5 {
            j += 1;
        }
        if j - i >= 2 {
            out.push(xs[i]);
        }
        i = j;
    }
    out
}

pub fn aligned(segs: &[TSeg], lines: &[TLine]) -> Aligned {
    let starts = lines
        .iter()
        .flat_map(|l| l.segs.iter().skip(1).map(|&s| segs[s].bbox.x0))
        .collect();
    let ends = lines
        .iter()
        .flat_map(|l| {
            let w = l.x1 - l.x0;
            l.segs
                .iter()
                .skip(1)
                .filter(move |&&s| segs[s].bbox.w() <= 0.4 * w)
                .map(|&s| segs[s].bbox.x1)
        })
        .collect();
    Aligned {
        starts: repeated(starts),
        ends: repeated(ends),
    }
}

pub fn pieces(
    g: &[RawGlyph],
    fonts: &[FontInfo],
    segs: &[TSeg],
    line: &TLine,
    marker: Option<&Marker>,
    right_edge: f32,
    aligned: &Aligned,
) -> (Vec<Piece>, Vec<TabStop>) {
    let mut out: Vec<Piece> = Vec::new();
    let mut tabs = Vec::new();
    let (base, size) = (line.base, line.size);
    let mut skip = marker.map_or(0, |m| m.n);
    let numbered = marker.is_some_and(|m| m.bullet.is_none());
    let mut dots: Vec<usize> = Vec::new();
    let flush_dots = |out: &mut Vec<Piece>, dots: &mut Vec<usize>| {
        for &d in dots.iter() {
            push_char(out, g[d].ch, style(&g[d], fonts, base, size));
        }
        dots.clear();
    };
    for (k, &si) in line.segs.iter().enumerate() {
        let s = &segs[si];
        let mut leader = false;
        if k > 0 && dots.len() >= 5 {
            // "Chapter ........ 7" with the number in its own segment.
            dots.clear();
            if let Some(Piece::Run(r)) = out.last_mut() {
                let t = r.text.trim_end().len();
                r.text.truncate(t);
            }
            tabs.push(TabStop {
                x: line.x1,
                right: true,
                dots: true,
            });
            out.push(Piece::Tab);
            leader = true;
        } else if k > 0 {
            flush_dots(&mut out, &mut dots);
        }
        // Nothing to separate from yet (only a bullet so far), or a tab already does it.
        let fresh = out.is_empty() || matches!(out.last(), Some(Piece::Tab));
        if k > 0 && skip == 0 && !leader && !fresh {
            let prev = &segs[line.segs[k - 1]];
            let gap = s.bbox.x0 - prev.bbox.x1;
            let starts = aligned.starts.iter().any(|&x| (x - s.bbox.x0).abs() <= 1.5);
            let ends = aligned.ends.iter().any(|&x| (x - s.bbox.x1).abs() <= 1.5);
            if gap >= 1.6 * size || starts || ends {
                let last = k + 1 == line.segs.len();
                let right = (last && (right_edge - s.bbox.x1).abs() <= 3.0) || (ends && !starts);
                tabs.push(TabStop {
                    x: if right { s.bbox.x1 } else { s.bbox.x0 },
                    right,
                    dots: false,
                });
                out.push(Piece::Tab);
            } else {
                push_space(&mut out);
            }
        }
        let has_spaces = s.ids.iter().any(|&i| g[i].ch == ' ');
        let thr = if has_spaces { 0.5 } else { 0.15 };
        let mut prev: Option<usize> = None;
        let mut pending = false;
        for &i in &s.ids {
            let gi = &g[i];
            if gi.ch == ' ' {
                pending = true;
                continue;
            }
            if skip > 0 {
                skip -= 1;
                if skip == 0 && numbered {
                    // The number stays as text; the hanging indent is its tab stop.
                    push_char(&mut out, gi.ch, style(gi, fonts, base, size));
                    out.push(Piece::Tab);
                } else if numbered {
                    push_char(&mut out, gi.ch, style(gi, fonts, base, size));
                }
                prev = None;
                pending = false;
                continue;
            }
            if let Some(p) = prev
                && gi.x0 - g[p].x1 > thr * gi.size.max(g[p].size)
            {
                pending = true;
            }
            // Dot leaders (tables of contents) become a tab with a dotted leader.
            if is_dot(gi.ch) {
                if pending && !dots.is_empty() {
                    pending = false;
                }
                if dots.is_empty() && pending && prev.is_some() {
                    push_space(&mut out);
                }
                pending = false;
                dots.push(i);
                prev = Some(i);
                continue;
            }
            if dots.len() >= 5 {
                dots.clear();
                if let Some(Piece::Run(r)) = out.last_mut() {
                    let t = r.text.trim_end().len();
                    r.text.truncate(t);
                }
                tabs.push(TabStop {
                    x: line.x1,
                    right: true,
                    dots: true,
                });
                out.push(Piece::Tab);
                pending = false;
            } else if !dots.is_empty() {
                flush_dots(&mut out, &mut dots);
            }
            if pending && prev.is_some() {
                push_space(&mut out);
            }
            pending = false;
            push_char(&mut out, gi.ch, style(gi, fonts, base, size));
            prev = Some(i);
        }
    }
    flush_dots(&mut out, &mut dots);
    if let Some(Piece::Run(r)) = out.last_mut() {
        let t = r.text.trim_end().len();
        r.text.truncate(t);
    }
    (out, tabs)
}

#[cfg(test)]
pub(crate) fn word(s: &str, x: f32, base: f32, size: f32) -> Vec<RawGlyph> {
    let mut v = Vec::new();
    let mut x = x;
    for ch in s.chars() {
        let mut gl = RawGlyph::test(ch, x, base, size);
        gl.x1 = x + 0.5 * size;
        v.push(gl);
        x += 0.5 * size;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(g: &[RawGlyph]) -> (Vec<TSeg>, TLine) {
        let ids: Vec<usize> = (0..g.len()).collect();
        let r = rows(g, &ids);
        let s = segments(g, &r);
        let l = lines(&s, &(0..s.len()).collect::<Vec<_>>());
        (s, l.into_iter().next().unwrap())
    }

    fn text(p: &[Piece]) -> String {
        p.iter()
            .map(|p| match p {
                Piece::Run(r) => r.text.clone(),
                Piece::Tab => "\t".into(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn spaces_tabs_and_superscripts() {
        let mut g = word("Hello", 0.0, 100.0, 10.0);
        g.extend(word("world", 28.0, 100.0, 10.0)); // 3 pt gap: a space
        g.extend(word("2", 53.0, 96.5, 6.0)); // superscript
        g.extend(word("Total", 200.0, 100.0, 10.0)); // far: a tab
        let (s, l) = line_of(&g);
        assert_eq!(s.len(), 2, "one gap is a tab-sized gap");
        let (p, tabs) = pieces(&g, &[], &s, &l, None, 300.0, &Aligned::default());
        assert_eq!(text(&p), "Hello world2\tTotal");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].x, 200.0);
        let sup = p.iter().find_map(|p| match p {
            Piece::Run(r) if r.text == "2" => Some(r.script),
            _ => None,
        });
        assert_eq!(sup, Some(Script::Super));
    }

    #[test]
    fn duplicates_make_bold() {
        let mut g = word("Hi", 0.0, 100.0, 10.0);
        let mut again = word("Hi", 0.3, 100.0, 10.0);
        g.append(&mut again);
        dedupe(&mut g);
        assert_eq!(g.len(), 2);
        assert!(g.iter().all(|x| x.bold));
    }

    #[test]
    fn markers() {
        let mut g = word("•", 0.0, 100.0, 10.0);
        g.extend(word("Item", 12.0, 100.0, 10.0));
        let (s, l) = line_of(&g);
        let m = marker(&g, &s, &l).unwrap();
        assert_eq!((m.bullet, m.n, m.text_x), (Some('•'), 1, 12.0));
        let (p, _) = pieces(&g, &[], &s, &l, Some(&m), 300.0, &Aligned::default());
        assert_eq!(text(&p), "Item", "Word draws the bullet");

        let mut g = word("12.", 0.0, 100.0, 10.0);
        g.extend(word("Twelve", 20.0, 100.0, 10.0));
        let (s, l) = line_of(&g);
        let m = marker(&g, &s, &l).unwrap();
        assert_eq!((m.bullet, m.n), (None, 3));
        let (p, _) = pieces(&g, &[], &s, &l, Some(&m), 300.0, &Aligned::default());
        assert_eq!(text(&p), "12.\tTwelve");

        assert!(is_number_marker("(iv)"));
        assert!(is_number_marker("2.1"));
        assert!(!is_number_marker("Hello."));
    }

    #[test]
    fn dot_leaders_become_a_tab() {
        let mut g = word("Intro", 0.0, 100.0, 10.0);
        for k in 0..12 {
            g.extend(word(".", 30.0 + k as f32 * 6.0, 100.0, 10.0));
        }
        g.extend(word("7", 110.0, 100.0, 10.0));
        let (s, l) = line_of(&g);
        let (p, tabs) = pieces(&g, &[], &s, &l, None, 115.0, &Aligned::default());
        assert_eq!(text(&p), "Intro\t7");
        assert!(tabs.iter().any(|t| t.dots && t.right));
    }
}
