//! Compare two PDFs. The report opens with a summary page, then puts each old page beside
//! the new one: what was removed marked red on the left, what was added green on the
//! right.
//!
//! Pages with text on both sides are compared word by word: the words are diffed as one
//! sequence per document (so text that moved to the next page isn't a change) with
//! Myers' algorithm in linear space. Pages without text on either side (scans,
//! drawings), or every page in [`Mode::Visual`], are compared by their pixels
//! ([`super::visual`]), with orange boxes around the changed areas.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use krilla::page::PageSettings;
use krilla::pdf::PdfDocument;
use swirvo_core::{CancelToken, EngineError};

use super::geometry::Box4;
use super::stamp::{self, Item};
use super::visual;
use crate::docxlayout::fonts::Fonts;
use crate::pdf_tools::PageRenderer;
use crate::pdflayout::extract::{self, RawGlyph};

/// Past this many edits the documents are too different for a useful word diff.
const MAX_EDITS: usize = 50_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Compared {
    pub added: usize,
    pub removed: usize,
    /// Report sheets (pairs of pages) with at least one change.
    pub changed_pages: usize,
    /// Changed areas found by comparing pages' appearance.
    pub regions: usize,
    /// Pages compared by appearance rather than by their words.
    pub visual_pages: usize,
}

#[derive(Debug, Clone)]
struct Word {
    text: String,
    page: usize,
    /// Display points, y up.
    rect: Box4,
}

/// Words with their boxes, page by page in drawing order.
fn words(pages: &[extract::RawPage]) -> Vec<Word> {
    let mut out = Vec::new();
    for (pi, p) in pages.iter().enumerate() {
        let mut cur: Option<Word> = None;
        let mut prev: Option<&RawGlyph> = None;
        for g in &p.glyphs {
            let size = g.size.max(1.0);
            let r = [
                g.x0,
                p.h - (g.base + 0.25 * size),
                g.x1,
                p.h - (g.base - 0.85 * size),
            ];
            let breaks = g.ch.is_whitespace()
                || prev.is_some_and(|q| {
                    (g.base - q.base).abs() > 0.4 * size
                        || g.x0 - q.x1 > 0.2 * size
                        || g.x0 + 0.5 * size < q.x0
                });
            if breaks && let Some(w) = cur.take() {
                out.push(w);
            }
            prev = Some(g);
            if g.ch.is_whitespace() {
                continue;
            }
            match &mut cur {
                Some(w) => {
                    w.text.push(g.ch);
                    w.rect = [
                        w.rect[0].min(r[0]),
                        w.rect[1].min(r[1]),
                        w.rect[2].max(r[2]),
                        w.rect[3].max(r[3]),
                    ];
                }
                None => {
                    cur = Some(Word {
                        text: g.ch.to_string(),
                        page: pi,
                        rect: r,
                    })
                }
            }
        }
        out.extend(cur);
    }
    out
}

fn hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Mark `del[i]` for every element of `a` not in the common subsequence and `ins[j]` for
/// every element of `b` not in it. Err when there are more than `MAX_EDITS` edits.
fn diff(a: &[u64], b: &[u64], del: &mut [bool], ins: &mut [bool]) -> Result<(), ()> {
    let mut budget = MAX_EDITS;
    rec(a, 0, a.len(), b, 0, b.len(), del, ins, &mut budget, 0)
}

#[allow(clippy::too_many_arguments)]
fn rec(
    a: &[u64],
    mut a0: usize,
    mut a1: usize,
    b: &[u64],
    mut b0: usize,
    mut b1: usize,
    del: &mut [bool],
    ins: &mut [bool],
    budget: &mut usize,
    depth: usize,
) -> Result<(), ()> {
    while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
        a0 += 1;
        b0 += 1;
    }
    while a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
    }
    if a0 == a1 || b0 == b1 {
        del[a0..a1].fill(true);
        ins[b0..b1].fill(true);
        return Ok(());
    }
    if depth > 200 {
        return Err(());
    }
    let (x0, y0, x1, y1, d) = middle_snake(&a[a0..a1], &b[b0..b1]);
    if d > *budget {
        return Err(());
    }
    *budget -= d.min(*budget);
    rec(a, a0, a0 + x0, b, b0, b0 + y0, del, ins, budget, depth + 1)?;
    rec(a, a0 + x1, a1, b, b0 + y1, b1, del, ins, budget, depth + 1)
}

/// Myers' middle snake: (x, y) → (u, v) on an optimal path, and the edit distance.
fn middle_snake(a: &[u64], b: &[u64]) -> (usize, usize, usize, usize, usize) {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let delta = n - m;
    let max = ((n + m + 1) / 2) as usize + 1;
    let off = max as isize;
    let mut vf = vec![0isize; 2 * max + 2];
    let mut vb = vec![0isize; 2 * max + 2];
    let odd = delta % 2 != 0;
    for d in 0..max as isize {
        // Forward.
        let mut k = -d;
        while k <= d {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && vf[i - 1] < vf[i + 1]) {
                vf[i + 1]
            } else {
                vf[i - 1] + 1
            };
            let mut y = x - k;
            let (sx, sy) = (x, y);
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            vf[i] = x;
            let kb = delta - k;
            if odd && kb > -d && kb < d {
                let j = (kb + off) as usize;
                if vf[i] + vb[j] >= n {
                    return (
                        sx as usize,
                        sy as usize,
                        x as usize,
                        y as usize,
                        (2 * d - 1) as usize,
                    );
                }
            }
            k += 2;
        }
        // Backward (distances from the ends).
        let mut k = -d;
        while k <= d {
            let i = (k + off) as usize;
            let mut x = if k == -d || (k != d && vb[i - 1] < vb[i + 1]) {
                vb[i + 1]
            } else {
                vb[i - 1] + 1
            };
            let mut y = x - k;
            let (sx, sy) = (x, y);
            while x < n && y < m && a[(n - 1 - x) as usize] == b[(m - 1 - y) as usize] {
                x += 1;
                y += 1;
            }
            vb[i] = x;
            let kf = delta - k;
            if !odd && kf >= -d && kf <= d {
                let j = (kf + off) as usize;
                if vb[i] + vf[j] >= n {
                    return (
                        (n - x) as usize,
                        (m - y) as usize,
                        (n - sx) as usize,
                        (m - sy) as usize,
                        (2 * d) as usize,
                    );
                }
            }
            k += 2;
        }
    }
    // Not reached for non-empty inputs; treat as all different.
    (0, 0, a.len(), b.len(), a.len() + b.len())
}

/// How to compare.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Words where both pages have text; pixels where either side has none (scans,
    /// drawings).
    #[default]
    Auto,
    /// Words only.
    Text,
    /// Pixels only, every page.
    Visual,
}

/// Pages are rendered to fit this many pixels for the visual comparison (about 100 dpi
/// for A4 or Letter: fine lines and small text still show).
const VISUAL_PX: u32 = 1100;

const REMOVED: [u8; 3] = [220, 45, 45];
const ADDED: [u8; 3] = [25, 160, 70];
const BOX: [u8; 3] = [240, 130, 0];

/// Compare `old` with `new` into a side-by-side report at `out`, choosing words or
/// pixels per page.
pub fn compare(
    old: &Path,
    new: &Path,
    out: &Path,
    cancel: &CancelToken,
) -> Result<Compared, EngineError> {
    compare_with(old, new, out, Mode::Auto, cancel)
}

/// Overlays and boxes for the pages compared by their pixels.
#[derive(Default)]
struct Visual {
    old: Vec<Vec<Item>>,
    new: Vec<Vec<Item>>,
    regions: usize,
    changed: Vec<usize>,
}

/// Display-point box (y up) for a pixel box of `d`, on a page of `size`, moved by `shift`.
fn to_points(d: &visual::PageDiff, b: &[u32; 4], size: (f32, f32), shift: (i32, i32)) -> Box4 {
    let (kx, ky) = (size.0 / d.w as f32, size.1 / d.h as f32);
    let pad = 4.0;
    let x0 = (b[0] as i32 + shift.0) as f32 * kx - pad;
    let x1 = (b[2] as i32 + shift.0) as f32 * kx + pad;
    let y0 = (b[1] as i32 + shift.1) as f32 * ky - pad;
    let y1 = (b[3] as i32 + shift.1) as f32 * ky + pad;
    [
        x0.max(0.0),
        (size.1 - y1).max(0.0),
        x1.min(size.0),
        (size.1 - y0).min(size.1),
    ]
}

fn compare_pixels(
    old: &Path,
    new: &Path,
    which: &[bool],
    counts: (usize, usize),
    cancel: &CancelToken,
) -> Result<Visual, EngineError> {
    let (ra, rb) = (PageRenderer::open(old)?, PageRenderer::open(new)?);
    let mut v = Visual {
        old: vec![Vec::new(); counts.0],
        new: vec![Vec::new(); counts.1],
        ..Visual::default()
    };
    ra.session(|sa| {
        rb.session(|sb| {
            for i in (0..which.len()).filter(|&i| which[i]) {
                cancel.check()?;
                let (Some(pa), Some(pb)) = (ra.size(i), rb.size(i)) else {
                    // A page only one side has: all of it is the change.
                    let (list, size) = match (ra.size(i), rb.size(i)) {
                        (Some(s), None) => (&mut v.old[i], s),
                        (None, Some(s)) => (&mut v.new[i], s),
                        _ => continue,
                    };
                    list.extend(outline([0.0, 0.0, size.0, size.1], 6.0));
                    v.regions += 1;
                    v.changed.push(i);
                    continue;
                };
                let ia = sa.render_fit(i, VISUAL_PX, VISUAL_PX)?;
                let ib = sb.render_fit(i, VISUAL_PX, VISUAL_PX)?;
                let d = visual::diff(&ia, &ib);
                if !d.changed() {
                    continue;
                }
                v.regions += d.boxes.len();
                v.changed.push(i);
                v.old[i].push(Item::Image {
                    bytes: Arc::new(visual::overlay(&d, &d.removed, REMOVED, d.shift)),
                    rect: [0.0, 0.0, pa.0, pa.1],
                    opacity: 1.0,
                });
                v.new[i].push(Item::Image {
                    bytes: Arc::new(visual::overlay(&d, &d.added, ADDED, (0, 0))),
                    rect: [0.0, 0.0, pb.0, pb.1],
                    opacity: 1.0,
                });
                for b in &d.boxes {
                    v.old[i].extend(outline(to_points(&d, b, pa, d.shift), 1.5));
                    v.new[i].extend(outline(to_points(&d, b, pb, (0, 0)), 1.5));
                }
            }
            Ok::<(), EngineError>(())
        })
    })?;
    Ok(v)
}

/// [`compare`] with the mode chosen.
pub fn compare_with(
    old: &Path,
    new: &Path,
    out: &Path,
    mode: Mode,
    cancel: &CancelToken,
) -> Result<Compared, EngineError> {
    let ea = extract::extract(&std::fs::read(old)?, cancel)?;
    let eb = extract::extract(&std::fs::read(new)?, cancel)?;
    let (wa, wb) = (words(&ea.pages), words(&eb.pages));
    let pages = ea.pages.len().max(eb.pages.len());
    if mode == Mode::Text && wa.is_empty() && wb.is_empty() {
        return Err(EngineError::Invalid(
            "neither PDF has text to compare; compare them visually instead".into(),
        ));
    }
    let has_text = |ws: &[Word], i: usize| ws.iter().any(|w| w.page == i);
    // Pages compared by their pixels.
    let visual: Vec<bool> = (0..pages)
        .map(|i| match mode {
            Mode::Text => false,
            Mode::Visual => true,
            Mode::Auto => !has_text(&wa, i) || !has_text(&wb, i),
        })
        .collect();
    // Words on visually compared pages stay out of the word diff, or a page that is a
    // scan on one side would count all its words as added.
    let keep = |w: &Word| !visual.get(w.page).copied().unwrap_or(false);
    let wa: Vec<Word> = wa.into_iter().filter(keep).collect();
    let wb: Vec<Word> = wb.into_iter().filter(keep).collect();
    let ha: Vec<u64> = wa.iter().map(|w| hash(&w.text)).collect();
    let hb: Vec<u64> = wb.iter().map(|w| hash(&w.text)).collect();
    let mut del = vec![false; wa.len()];
    let mut ins = vec![false; wb.len()];
    cancel.check()?;
    diff(&ha, &hb, &mut del, &mut ins).map_err(|_| {
        EngineError::Invalid("the two PDFs are too different to compare word by word".into())
    })?;
    let counts = (
        ins.iter().filter(|&&x| x).count(),
        del.iter().filter(|&&x| x).count(),
    );
    let marks = |ws: &[Word], on: &[bool], n: usize| -> Vec<Vec<Box4>> {
        let mut v = vec![Vec::new(); n];
        for (w, _) in ws.iter().zip(on).filter(|(_, m)| **m) {
            if let Some(p) = v.get_mut(w.page) {
                p.push(w.rect);
            }
        }
        v
    };
    let ma = marks(&wa, &del, ea.pages.len());
    let mb = marks(&wb, &ins, eb.pages.len());
    let any_visual = visual.iter().any(|v| *v);
    let vis = if any_visual {
        compare_pixels(old, new, &visual, (ea.pages.len(), eb.pages.len()), cancel)?
    } else {
        Visual {
            old: vec![Vec::new(); ea.pages.len()],
            new: vec![Vec::new(); eb.pages.len()],
            ..Visual::default()
        }
    };
    if counts == (0, 0) && vis.regions == 0 {
        return Err(EngineError::Invalid(if any_visual {
            "the two PDFs look the same".into()
        } else {
            "the two PDFs have the same text".into()
        }));
    }

    // Marked copies, then the summary and the side-by-side sheets.
    let dir = tempfile::tempdir()?;
    let (ca, cb) = (dir.path().join("a.pdf"), dir.path().join("b.pdf"));
    let mark = |src: &Path, dst: &Path, m: &[Vec<Box4>], v: &[Vec<Item>], color: [u8; 3]| {
        stamp::stamp(src, dst, None, cancel, |p| {
            let mut items: Vec<Item> = m
                .get(p.index)
                .map(|rs| {
                    rs.iter()
                        .map(|r| Item::Rect {
                            rect: *r,
                            color,
                            opacity: 0.35,
                        })
                        .collect()
                })
                .unwrap_or_default();
            items.extend(v.get(p.index).cloned().unwrap_or_default());
            items
        })
        .map(|_| ())
    };
    mark(old, &ca, &ma, &vis.old, REMOVED)?;
    mark(new, &cb, &mb, &vis.new, ADDED)?;
    let pa = super::assemble::load(&ca)?;
    let pb = super::assemble::load(&cb)?;
    let sa: Vec<(f32, f32)> = pa.pages().iter().map(|p| p.render_dimensions()).collect();
    let sb: Vec<(f32, f32)> = pb.pages().iter().map(|p| p.render_dimensions()).collect();
    let (da, db) = (PdfDocument::new(pa), PdfDocument::new(pb));
    let mut doc = krilla::Document::new();
    let margin = 18.0;
    let summary = Fonts::available();
    let mut changed = Vec::new();
    for i in 0..pages {
        cancel.check()?;
        let a = sa.get(i).copied();
        let b = sb.get(i).copied();
        let (w0, h0) = a.or(b).unwrap_or((595.0, 842.0));
        let (w1, h1) = b.or(a).unwrap_or((595.0, 842.0));
        let h = h0.max(h1);
        let settings = PageSettings::from_wh(w0 + w1 + 3.0 * margin, h + 2.0 * margin)
            .ok_or_else(|| EngineError::Invalid("bad page size".into()))?;
        if i == 0 && summary {
            // The summary page goes first, at the size of the sheets after it.
            doc.start_page_with(settings.clone()).finish();
        }
        let mut page = doc.start_page_with(settings);
        let mut s = page.surface();
        if let Some(size) = a {
            super::assemble::draw_in(&mut s, &da, size, i, (margin, margin, w0, h));
        }
        if let Some(size) = b {
            super::assemble::draw_in(&mut s, &db, size, i, (2.0 * margin + w0, margin, w1, h));
        }
        s.finish();
        page.finish();
        if ma.get(i).is_some_and(|m| !m.is_empty())
            || mb.get(i).is_some_and(|m| !m.is_empty())
            || vis.changed.contains(&i)
        {
            changed.push(i);
        }
    }
    let rep = Compared {
        added: counts.0,
        removed: counts.1,
        changed_pages: changed.len(),
        regions: vis.regions,
        visual_pages: visual.iter().filter(|v| **v).count(),
    };
    if !summary {
        super::assemble::write(doc, out)?;
        return Ok(rep);
    }
    let tmp = dir.path().join("report.pdf");
    super::assemble::write(doc, &tmp)?;
    let lines = summary_lines(old, new, &rep, &changed, &vis.changed);
    stamp::stamp(&tmp, out, Some(&[0]), cancel, |p| {
        let mut y = p.h - 72.0;
        lines
            .iter()
            .enumerate()
            .map(|(k, (text, size, bold))| {
                let item = Item::Text {
                    text: text.clone(),
                    size: *size,
                    color: if k == 0 { [20, 20, 20] } else { [60, 60, 60] },
                    opacity: 1.0,
                    x: 54.0,
                    y,
                    align: stamp::HAlign::Left,
                    angle: 0.0,
                    bold: *bold,
                };
                y -= size * 1.9;
                item
            })
            .collect()
    })?;
    Ok(rep)
}

/// A rectangle outline as four thin filled strips (stamps draw filled shapes).
fn outline(r: Box4, w: f32) -> Vec<Item> {
    let [x0, y0, x1, y1] = r;
    [
        [x0, y0, x1, y0 + w],
        [x0, y1 - w, x1, y1],
        [x0, y0, x0 + w, y1],
        [x1 - w, y0, x1, y1],
    ]
    .into_iter()
    .map(|rect| Item::Rect {
        rect,
        color: BOX,
        opacity: 0.9,
    })
    .collect()
}

/// "2, 5, 7–9" from sorted 0-based page indexes.
fn page_list(pages: &[usize]) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let mut j = i;
        while j + 1 < pages.len() && pages[j + 1] == pages[j] + 1 {
            j += 1;
        }
        parts.push(match j - i {
            0 => (pages[i] + 1).to_string(),
            1 => format!("{}, {}", pages[i] + 1, pages[j] + 1),
            _ => format!("{}–{}", pages[i] + 1, pages[j] + 1),
        });
        i = j + 1;
    }
    parts.join(", ")
}

/// The summary page's lines: (text, size, bold).
fn summary_lines(
    old: &Path,
    new: &Path,
    rep: &Compared,
    changed: &[usize],
    visual_changed: &[usize],
) -> Vec<(String, f32, bool)> {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let plural = |n: usize, one: &str, many: &str| {
        if n == 1 {
            format!("1 {one}")
        } else {
            format!("{n} {many}")
        }
    };
    let mut v = vec![
        ("What changed".to_string(), 22.0, true),
        (format!("Original: {}", name(old)), 12.0, false),
        (format!("New: {}", name(new)), 12.0, false),
        (
            format!(
                "Changed: {} ({})",
                plural(changed.len(), "page", "pages"),
                page_list(changed)
            ),
            12.0,
            true,
        ),
    ];
    if rep.added + rep.removed > 0 {
        v.push((
            format!("Words: {} added, {} removed", rep.added, rep.removed),
            12.0,
            false,
        ));
    }
    if rep.visual_pages > 0 {
        v.push((
            if visual_changed.is_empty() {
                format!(
                    "Compared by appearance: {}, no changes",
                    plural(rep.visual_pages, "page", "pages")
                )
            } else {
                format!(
                    "Compared by appearance: {} changed on {} ({})",
                    plural(rep.regions, "area", "areas"),
                    plural(visual_changed.len(), "page", "pages"),
                    page_list(visual_changed)
                )
            },
            12.0,
            false,
        ));
    }
    v.push((
        "Each sheet: original on the left, new on the right. Red: removed. Green: added. \
         Orange boxes: changed areas."
            .to_string(),
        10.0,
        false,
    ));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(a: &str, b: &str) -> (String, String) {
        let ta: Vec<&str> = a.split(' ').collect();
        let tb: Vec<&str> = b.split(' ').collect();
        let ha: Vec<u64> = ta.iter().map(|w| hash(w)).collect();
        let hb: Vec<u64> = tb.iter().map(|w| hash(w)).collect();
        let (mut d, mut i) = (vec![false; ha.len()], vec![false; hb.len()]);
        diff(&ha, &hb, &mut d, &mut i).unwrap();
        let pick = |t: &[&str], m: &[bool]| {
            t.iter()
                .zip(m)
                .filter(|(_, m)| **m)
                .map(|(w, _)| *w)
                .collect::<Vec<_>>()
                .join(" ")
        };
        (pick(&ta, &d), pick(&tb, &i))
    }

    #[test]
    fn word_diff_is_minimal() {
        assert_eq!(
            run("the quick brown fox jumps", "the slow brown fox leaps high"),
            ("quick jumps".into(), "slow leaps high".into())
        );
        assert_eq!(run("a b c", "a b c"), (String::new(), String::new()));
        assert_eq!(run("x", "y"), ("x".into(), "y".into()));
        assert_eq!(
            run("a b c d e f g h", "a c d x e f h"),
            ("b g".into(), "x".into())
        );
        // A long shared run with edits at both ends.
        let base: Vec<String> = (0..500).map(|i| format!("w{i}")).collect();
        let a = format!("start {} end", base.join(" "));
        let b = format!("begin {} finish", base.join(" "));
        assert_eq!(run(&a, &b), ("start end".into(), "begin finish".into()));
    }

    #[test]
    fn diff_matches_a_brute_force_lcs() {
        // Small alphabets make many ties and repeats: the hard cases.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..400 {
            let (la, lb, k) = (rnd(14) as usize, rnd(14) as usize, 1 + rnd(4));
            let a: Vec<u64> = (0..la).map(|_| rnd(k)).collect();
            let b: Vec<u64> = (0..lb).map(|_| rnd(k)).collect();
            let (mut d, mut i) = (vec![false; la], vec![false; lb]);
            diff(&a, &b, &mut d, &mut i).unwrap();
            let kept_a: Vec<u64> = a
                .iter()
                .zip(&d)
                .filter(|(_, x)| !**x)
                .map(|(v, _)| *v)
                .collect();
            let kept_b: Vec<u64> = b
                .iter()
                .zip(&i)
                .filter(|(_, x)| !**x)
                .map(|(v, _)| *v)
                .collect();
            assert_eq!(kept_a, kept_b, "{a:?} {b:?}");
            let mut t = vec![vec![0usize; lb + 1]; la + 1];
            for x in (0..la).rev() {
                for y in (0..lb).rev() {
                    t[x][y] = if a[x] == b[y] {
                        t[x + 1][y + 1] + 1
                    } else {
                        t[x + 1][y].max(t[x][y + 1])
                    };
                }
            }
            assert_eq!(kept_a.len(), t[0][0], "not minimal: {a:?} {b:?}");
        }
    }

    #[test]
    fn report_marks_the_changes() {
        if !crate::docxlayout::fonts::Fonts::available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("in.pdf");
        super::super::testing::sample(&src, 1, false);
        let write = |name: &str, text: &'static str| {
            let p = d.path().join(name);
            stamp::stamp(&src, &p, None, &CancelToken::new(), |_| {
                vec![Item::Text {
                    text: text.into(),
                    size: 14.0,
                    color: [0, 0, 0],
                    opacity: 1.0,
                    x: 72.0,
                    y: 500.0,
                    align: stamp::HAlign::Left,
                    angle: 0.0,
                    bold: false,
                }]
            })
            .unwrap();
            p
        };
        let a = write("a.pdf", "Payment is due within thirty days of delivery");
        let b = write("b.pdf", "Payment is due within sixty days of delivery");
        let out = d.path().join("report.pdf");
        let r = compare(&a, &b, &out, &CancelToken::new()).unwrap();
        assert_eq!((r.added, r.removed, r.changed_pages), (1, 1, 1));
        assert_eq!(
            (r.visual_pages, r.regions),
            (0, 0),
            "pages with text are compared by their words"
        );
        assert_eq!(
            super::super::assemble::page_count(&out).unwrap(),
            2,
            "a summary page, then the sheet"
        );
        let ex = extract::extract(&std::fs::read(&out).unwrap(), &CancelToken::new()).unwrap();
        let summary: String = ex.pages[0].glyphs.iter().map(|g| g.ch).collect();
        assert!(summary.contains("What changed"), "{summary}");
        assert!(
            compare(&a, &a, &out, &CancelToken::new()).is_err(),
            "same text"
        );
    }

    /// Drawings without text are compared by appearance: a shape added on one page is
    /// found and boxed, nothing else is, and the same drawing is "the same".
    #[test]
    fn drawings_are_compared_by_appearance() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a.pdf");
        super::super::testing::sample(&a, 3, false);
        let b = d.path().join("b.pdf");
        stamp::stamp(&a, &b, Some(&[1]), &CancelToken::new(), |_| {
            vec![Item::Rect {
                rect: [300.0, 300.0, 420.0, 360.0],
                color: [30, 30, 200],
                opacity: 1.0,
            }]
        })
        .unwrap();
        let out = d.path().join("report.pdf");
        let r = compare(&a, &b, &out, &CancelToken::new()).unwrap();
        assert_eq!(r.visual_pages, 3, "no text: every page by appearance");
        assert_eq!((r.changed_pages, r.regions), (1, 1), "{r:?}");
        assert_eq!((r.added, r.removed), (0, 0));
        let n = super::super::assemble::page_count(&out).unwrap();
        assert_eq!(n, if Fonts::available() { 4 } else { 3 });

        // The new sheet shows green where the shape was added, with an orange box.
        let sheet = if Fonts::available() { 2 } else { 1 };
        let img = PageRenderer::open(&out)
            .unwrap()
            .session(|s| s.render_dpi(sheet, 36.0))
            .unwrap();
        let (w, h) = img.dimensions();
        let k = w as f32 / (595.0 * 2.0 + 3.0 * 18.0);
        let at = |x: f32, y_up: f32| {
            let p = img.get_pixel(
                ((x * k) as u32).min(w - 1),
                (((842.0 + 36.0 - y_up) * k) as u32).min(h - 1),
            );
            [p[0], p[1], p[2]]
        };
        // Right half: x offset 2 × margin + old width.
        let inside = at(36.0 + 595.0 + 360.0, 18.0 + 330.0);
        assert!(inside[1] > inside[0], "added area tinted green: {inside:?}");
        assert!(
            compare(&a, &a, &out, &CancelToken::new()).is_err(),
            "same drawing"
        );
        assert!(
            compare_with(&a, &b, &out, Mode::Text, &CancelToken::new()).is_err(),
            "no words to compare"
        );
    }

    #[test]
    fn page_lists_read_naturally() {
        assert_eq!(page_list(&[0, 1, 2, 4, 6, 7]), "1–3, 5, 7, 8");
        assert_eq!(page_list(&[]), "");
    }
}
