//! Paragraphs (docs/13 §4.3): lines of one region → paragraphs with alignment, indents, pitch,
//! tab stops and list markers.

use super::extract::RawGlyph;
use super::model::{Align, Bullet, FontInfo, Para, Piece, TabStop};
use super::text::{self, Marker, TLine, TSeg};

pub struct Ctx<'a> {
    pub g: &'a [RawGlyph],
    pub fonts: &'a [FontInfo],
    pub segs: &'a [TSeg],
}

struct Info {
    line: TLine,
    marker: Option<Marker>,
    pieces: Vec<Piece>,
    tabs: Vec<TabStop>,
    bold: f32,
    /// Width of the line's first word, for "would it have fitted on the line above?".
    first_word: f32,
}

fn first_word(cx: &Ctx<'_>, line: &TLine) -> f32 {
    let Some(&s) = line.segs.first() else {
        return 0.0;
    };
    let ids = &cx.segs[s].ids;
    let mut start: Option<f32> = None;
    let mut end = 0.0f32;
    for &i in ids {
        let g = &cx.g[i];
        if g.ch == ' ' {
            if start.is_some() {
                break;
            }
            continue;
        }
        if start.is_some() && g.x0 - end > 0.15 * g.size {
            break;
        }
        start.get_or_insert(g.x0);
        end = end.max(g.x1);
    }
    start.map_or(0.0, |s0| end - s0)
}

fn bold_frac(p: &[Piece]) -> f32 {
    let (mut b, mut n) = (0usize, 0usize);
    for p in p {
        if let Piece::Run(r) = p {
            let k = r.text.chars().filter(|c| !c.is_whitespace()).count();
            n += k;
            if r.bold {
                b += k;
            }
        }
    }
    if n == 0 { 0.0 } else { b as f32 / n as f32 }
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// Paragraphs for one region. `col` is the column's horizontal extent (for alignment);
/// `avail` is how far right a line could have run before wrapping.
pub fn paragraphs(cx: &Ctx<'_>, lines: Vec<TLine>, col: (f32, f32), avail: f32) -> Vec<Para> {
    let aligned = text::aligned(cx.segs, &lines);
    paragraphs_with(cx, lines, col, avail, &aligned)
}

/// As [`paragraphs`], with column alignments measured over a larger region (a whole page
/// column), so table rows split into separate blocks still line up.
pub fn paragraphs_with(
    cx: &Ctx<'_>,
    lines: Vec<TLine>,
    col: (f32, f32),
    avail: f32,
    aligned: &text::Aligned,
) -> Vec<Para> {
    if lines.is_empty() {
        return Vec::new();
    }
    let lx0 = lines.iter().map(|l| l.x0).fold(f32::INFINITY, f32::min);
    let lx1 = lines.iter().map(|l| l.x1).fold(f32::NEG_INFINITY, f32::max);
    let infos: Vec<Info> = lines
        .into_iter()
        .map(|line| {
            let marker = text::marker(cx.g, cx.segs, &line);
            let (pieces, tabs) = text::pieces(
                cx.g,
                cx.fonts,
                cx.segs,
                &line,
                marker.as_ref(),
                lx1.max(col.1),
                aligned,
            );
            let bold = bold_frac(&pieces);
            let first_word = first_word(cx, &line);
            Info {
                line,
                marker,
                pieces,
                tabs,
                bold,
                first_word,
            }
        })
        .collect();
    // Where lines wrap: a shared right edge (justified or boxed text), else the column's.
    let edge = infos
        .iter()
        .filter(|i| (i.line.x1 - lx1).abs() <= 1.5)
        .count();
    let wrap = if edge >= 2 {
        lx1
    } else {
        avail.max(col.1).max(lx1)
    };
    // Typical pitch of this region, for single-line paragraphs.
    let pitch_hint = median(
        infos
            .windows(2)
            .filter(|w| (w[0].line.size - w[1].line.size).abs() < 0.5)
            .map(|w| w[1].line.base - w[0].line.base)
            .filter(|&p| p > 0.0 && p < 1.75 * infos[0].line.size.max(1.0))
            .collect(),
    );
    let mut groups: Vec<Vec<Info>> = Vec::new();
    for li in infos {
        let joins = groups.last().is_some_and(|p| continues(p, &li, lx0, wrap));
        if joins {
            if let Some(p) = groups.last_mut() {
                p.push(li);
            }
        } else {
            groups.push(vec![li]);
        }
    }
    groups
        .into_iter()
        .map(|grp| build(cx, grp, col, pitch_hint))
        .collect()
}

fn centered_pair(a: &TLine, b: &TLine) -> bool {
    let (ca, cb) = ((a.x0 + a.x1) / 2.0, (b.x0 + b.x1) / 2.0);
    (ca - cb).abs() < 3.0 && (a.x0 - b.x0).abs() > 4.0
}

fn continues(p: &[Info], l: &Info, lx0: f32, wrap: f32) -> bool {
    let last = &p[p.len() - 1];
    let (a, b) = (&last.line, &l.line);
    let size = a.size.max(b.size);
    let pitch = b.base - a.base;
    if pitch <= 0.5 * size {
        return false;
    }
    if p.len() >= 2 {
        let pp = a.base - p[p.len() - 2].line.base;
        if (pitch - pp).abs() > 0.2 * pp.max(1.0) {
            return false;
        }
    } else if pitch > 1.75 * size {
        return false;
    }
    if (a.size - b.size).abs() > 0.15 * a.size
        || (last.bold - l.bold).abs() > 0.5
        || !l.tabs.is_empty()
        || !last.tabs.is_empty()
        || l.marker.is_some()
    {
        return false;
    }
    let em = size;
    if centered_pair(a, b) && a.x0 > lx0 + 2.0 * em {
        return true;
    }
    // The next line's first word would have fitted on the previous line: that was a hard
    // break (an address, a short last line), not a wrap.
    if a.x1 + 0.28 * a.size + l.first_word <= wrap - 0.5 {
        return false;
    }
    if p.len() == 1 {
        if let Some(m) = &last.marker {
            return (b.x0 - m.text_x).abs() < 0.6 * em;
        }
        // Same left edge, a first-line indent (first line further right), or a modest
        // hanging indent (continuation further right).
        return b.x0 - a.x0 < 4.0 * em;
    }
    let body = p[1].line.x0;
    (b.x0 - body).abs() < 0.6 * em
}

/// Mean gap between words on a line, in ems.
fn word_gap(cx: &Ctx<'_>, line: &TLine) -> Option<f32> {
    let (mut sum, mut n) = (0.0f32, 0usize);
    let mut prev: Option<&RawGlyph> = None;
    for &s in &line.segs {
        for &i in &cx.segs[s].ids {
            let g = &cx.g[i];
            if g.ch == ' ' {
                continue;
            }
            if let Some(p) = prev {
                let gap = g.x0 - p.x1;
                if gap > 0.12 * g.size && gap < 3.0 * g.size {
                    sum += gap / g.size;
                    n += 1;
                }
            }
            prev = Some(g);
        }
        prev = None;
    }
    (n > 0).then(|| sum / n as f32)
}

/// Justified lines have their word gaps stretched compared with the paragraph's last line
/// (or a normal space). Paragraphs of three or more aligned lines are taken as justified.
fn stretched(cx: &Ctx<'_>, grp: &[Info]) -> bool {
    if grp.len() >= 4 {
        return true;
    }
    let body: Vec<f32> = grp[..grp.len() - 1]
        .iter()
        .filter_map(|i| word_gap(cx, &i.line))
        .collect();
    if body.is_empty() {
        return false;
    }
    let mean = body.iter().sum::<f32>() / body.len() as f32;
    let normal = word_gap(cx, &grp[grp.len() - 1].line)
        .unwrap_or(0.28)
        .min(0.33);
    mean > normal * 1.12 + 0.01
}

fn build(cx: &Ctx<'_>, grp: Vec<Info>, col: (f32, f32), pitch_hint: Option<f32>) -> Para {
    let n = grp.len();
    let first = &grp[0];
    let size_max = grp.iter().map(|i| i.line.size).fold(0.0, f32::max);
    let pitch = if n >= 2 {
        median(
            grp.windows(2)
                .map(|w| w[1].line.base - w[0].line.base)
                .collect(),
        )
        .unwrap_or(1.2 * size_max)
    } else {
        pitch_hint
            .filter(|p| *p >= size_max && *p <= 1.6 * size_max)
            .unwrap_or(1.2 * size_max)
    };
    let first_x = first.line.x0;
    let mut left = if n >= 2 {
        grp[1..]
            .iter()
            .map(|i| i.line.x0)
            .fold(f32::INFINITY, f32::min)
    } else {
        first_x
    };
    if let Some(m) = &first.marker {
        left = m.text_x;
    }
    let right = grp
        .iter()
        .map(|i| i.line.x1)
        .fold(f32::NEG_INFINITY, f32::max);
    let (cx0, cx1) = col;
    let cw = (cx1 - cx0).max(1.0);
    let align = if first.marker.is_some() {
        Align::Left
    } else if n == 1 {
        let c = (first.line.x0 + first.line.x1) / 2.0;
        if (c - (cx0 + cx1) / 2.0).abs() < 0.03 * cw && first.line.x0 > cx0 + 0.08 * cw {
            Align::Center
        } else if (cx1 - first.line.x1).abs() < 3.0 && first.line.x0 > cx0 + 0.35 * cw {
            Align::Right
        } else {
            Align::Left
        }
    } else {
        let lines: Vec<&TLine> = grp.iter().map(|i| &i.line).collect();
        let near = |a: f32, b: f32, t: f32| (a - b).abs() <= t;
        let lefts_aligned = lines[1..].iter().all(|l| near(l.x0, lines[1].x0, 2.5));
        let rights_all = lines.iter().all(|l| near(l.x1, right, 2.5));
        let rights_body = lines[..n - 1].iter().all(|l| near(l.x1, right, 2.5));
        let c0 = (lines[0].x0 + lines[0].x1) / 2.0;
        let centers = lines.iter().all(|l| near((l.x0 + l.x1) / 2.0, c0, 3.0));
        let lefts_differ = lines.iter().any(|l| !near(l.x0, lines[0].x0, 2.5));
        if centers && lefts_differ {
            Align::Center
        } else if rights_all && lefts_differ && !lefts_aligned {
            Align::Right
        } else if rights_body && (n >= 3 || lines[n - 1].x1 < right - 3.0) && stretched(cx, &grp) {
            Align::Justify
        } else {
            Align::Left
        }
    };
    let mut tabs: Vec<TabStop> = Vec::new();
    for i in &grp {
        for t in &i.tabs {
            if !tabs.iter().any(|u| (u.x - t.x).abs() < 1.0) {
                tabs.push(*t);
            }
        }
    }
    tabs.sort_by(|a, b| a.x.total_cmp(&b.x));
    let bullet = first.marker.as_ref().and_then(|m| {
        let ch = m.bullet?;
        let gid = first
            .line
            .segs
            .first()
            .and_then(|&s| cx.segs[s].ids.iter().find(|&&i| cx.g[i].ch != ' '))?;
        let gl = &cx.g[*gid];
        Some(Bullet {
            glyph: ch,
            font: gl.font,
            size: (gl.size * 2.0).round() / 2.0,
            color: gl.color,
        })
    });
    let numbered = first.marker.as_ref().is_some_and(|m| m.bullet.is_none());
    // Dominant size and weight by character count.
    let mut sizes: Vec<(f32, usize)> = Vec::new();
    let (mut bold_chars, mut all_chars) = (0usize, 0usize);
    for i in &grp {
        for p in &i.pieces {
            if let Piece::Run(r) = p {
                let k = r.text.chars().filter(|c| !c.is_whitespace()).count();
                all_chars += k;
                if r.bold {
                    bold_chars += k;
                }
                match sizes.iter_mut().find(|(s, _)| (*s - r.size).abs() < 0.26) {
                    Some(e) => e.1 += k,
                    None => sizes.push((r.size, k)),
                }
            }
        }
    }
    let size = sizes
        .iter()
        .max_by_key(|e| e.1)
        .map_or(first.line.size, |e| e.0);
    Para {
        left,
        first: first_x,
        right,
        first_base: first.line.base,
        last_base: grp[n - 1].line.base,
        pitch,
        align,
        tabs,
        lines: grp.into_iter().map(|i| i.pieces).collect(),
        bullet,
        size,
        bold: all_chars > 0 && bold_chars * 2 > all_chars,
        heading: None,
        numbered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdflayout::text::{lines, rows, segments, word};

    fn paras(g: &[RawGlyph]) -> Vec<Para> {
        let ids: Vec<usize> = (0..g.len()).collect();
        let r = rows(g, &ids);
        let s = segments(g, &r);
        let l = lines(&s, &(0..s.len()).collect::<Vec<_>>());
        let cx = Ctx {
            g,
            fonts: &[],
            segs: &s,
        };
        paragraphs(&cx, l, (50.0, 350.0), 350.0)
    }

    #[test]
    fn justified_paragraph_then_indented_paragraph() {
        let mut g = Vec::new();
        // Paragraph 1: three full lines (to x = 350) and a short last line.
        for k in 0..3 {
            let base = 100.0 + 12.0 * k as f32;
            let mut x = 50.0 + 5.0 * k as f32;
            g.extend(word(&"w".repeat(k + 1), 50.0, base, 10.0));
            while x + 30.0 < 344.0 {
                g.extend(word("abcd", x + 10.0, base, 10.0));
                x += 23.0;
            }
            g.extend(word("e", 345.0, base, 10.0));
        }
        g.extend(word("end.", 50.0, 136.0, 10.0));
        // Paragraph 2 starts with a first-line indent.
        let mut x = 70.0;
        while x < 345.0 {
            g.extend(word("abcd", x, 148.0, 10.0));
            x += 23.0;
        }
        g.extend(word("e", 345.0, 148.0, 10.0));
        g.extend(word("tail", 50.0, 160.0, 10.0));
        let p = paras(&g);
        assert_eq!(
            p.len(),
            2,
            "{:?}",
            p.iter().map(Para::text).collect::<Vec<_>>()
        );
        assert_eq!(p[0].align, Align::Justify);
        assert_eq!(p[0].lines.len(), 4);
        assert!((p[0].pitch - 12.0).abs() < 0.01);
        assert_eq!(p[1].first, 70.0);
        assert_eq!(p[1].left, 50.0);
    }

    #[test]
    fn centered_title_and_bullets() {
        let mut g = word("Title", 175.0, 60.0, 20.0); // centered in 50..350
        g.extend(word("•", 50.0, 100.0, 10.0));
        g.extend(word("First", 62.0, 100.0, 10.0));
        g.extend(word("•", 50.0, 112.0, 10.0));
        g.extend(word("Second", 62.0, 112.0, 10.0));
        let p = paras(&g);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].align, Align::Center);
        assert_eq!(p[1].bullet.as_ref().map(|b| b.glyph), Some('•'));
        assert_eq!(p[1].left, 62.0);
        assert_eq!(p[1].first, 50.0);
        assert_eq!(p[2].text(), "Second");
    }
}
