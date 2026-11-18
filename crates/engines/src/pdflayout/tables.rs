//! Lattice tables (docs/13 §4.4): ruling lines (and edges of shaded boxes) → a cell grid with
//! merged cells, per-edge borders and fills. Text is assigned later, by the page builder.

use super::extract::Mark;
use super::model::{BBox, Border, Rgb};

/// Snap distance for "the same line" and "touching".
const TOL: f32 = 2.0;
/// Boxes taller than this are panels, not table cells: their edges aren't rulings.
const MAX_CELL_BOX_H: f32 = 40.0;

#[derive(Clone, Debug)]
struct Seg {
    horizontal: bool,
    at: f32,
    lo: f32,
    hi: f32,
    width: f32,
    color: Rgb,
    /// Only an edge of a filled box: it shapes the grid but draws no border.
    virt: bool,
    srcs: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridCell {
    pub r0: usize,
    pub r1: usize,
    pub c0: usize,
    pub c1: usize,
    /// Top, left, bottom, right.
    pub borders: [Option<Border>; 4],
    pub fill: Option<Rgb>,
}

impl GridCell {
    pub fn bbox(&self, g: &Grid) -> BBox {
        BBox::new(g.xs[self.c0], g.ys[self.r0], g.xs[self.c1], g.ys[self.r1])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub xs: Vec<f32>,
    pub ys: Vec<f32>,
    /// Merged cells in row-major order of their top-left corner.
    pub cells: Vec<GridCell>,
    /// Marks the table draws itself (borders, fills); the page mustn't draw them again.
    pub consumed: Vec<usize>,
}

impl Grid {
    pub fn bbox(&self) -> BBox {
        BBox::new(
            self.xs[0],
            self.ys[0],
            self.xs[self.xs.len() - 1],
            self.ys[self.ys.len() - 1],
        )
    }

    /// The merged cell containing a point.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<usize> {
        let c = self.xs.windows(2).position(|w| x >= w[0] && x < w[1])?;
        let r = self.ys.windows(2).position(|w| y >= w[0] && y < w[1])?;
        self.cells
            .iter()
            .position(|k| (k.r0..k.r1).contains(&r) && (k.c0..k.c1).contains(&c))
    }
}

struct Uf(Vec<usize>);

impl Uf {
    fn new(n: usize) -> Uf {
        Uf((0..n).collect())
    }
    fn find(&mut self, mut i: usize) -> usize {
        while self.0[i] != i {
            self.0[i] = self.0[self.0[i]];
            i = self.0[i];
        }
        i
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b] = a;
        }
    }
}

fn segments(marks: &[Mark], consumed: &[bool]) -> Vec<Seg> {
    let mut v = Vec::new();
    let mut edge = |horizontal, at, lo, hi, width, color, virt, i| {
        v.push(Seg {
            horizontal,
            at,
            lo,
            hi,
            width,
            color,
            virt,
            srcs: vec![i],
        })
    };
    for (i, m) in marks.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        match *m {
            Mark::Rule {
                horizontal,
                at,
                lo,
                hi,
                width,
                color,
            } => edge(horizontal, at, lo, hi, width, color, false, i),
            Mark::Frame {
                bbox: b,
                color,
                width,
            } => {
                edge(true, b.y0, b.x0, b.x1, width, color, false, i);
                edge(true, b.y1, b.x0, b.x1, width, color, false, i);
                edge(false, b.x0, b.y0, b.y1, width, color, false, i);
                edge(false, b.x1, b.y0, b.y1, width, color, false, i);
            }
            Mark::Box { bbox: b, fill } if b.h() <= MAX_CELL_BOX_H => {
                edge(true, b.y0, b.x0, b.x1, 0.0, fill, true, i);
                edge(true, b.y1, b.x0, b.x1, 0.0, fill, true, i);
                edge(false, b.x0, b.y0, b.y1, 0.0, fill, true, i);
                edge(false, b.x1, b.y0, b.y1, 0.0, fill, true, i);
            }
            _ => {}
        }
    }
    v
}

/// Merge collinear, touching segments.
fn merge(mut v: Vec<Seg>) -> Vec<Seg> {
    v.sort_by(|a, b| {
        (a.horizontal, a.at, a.lo)
            .partial_cmp(&(b.horizontal, b.at, b.lo))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<Seg> = Vec::with_capacity(v.len());
    for s in v {
        if let Some(p) = out.iter_mut().rev().take(16).find(|p| {
            p.horizontal == s.horizontal
                && (p.at - s.at).abs() <= 1.0
                && s.lo <= p.hi + 1.0
                && s.hi >= p.lo - 1.0
                // Different colors or weights stay separate, so each cell keeps its border.
                && (s.virt || p.virt || (p.color == s.color && (p.width - s.width).abs() < 0.3))
        }) {
            p.lo = p.lo.min(s.lo);
            p.hi = p.hi.max(s.hi);
            if p.virt && !s.virt {
                p.at = s.at; // a drawn line beats a box edge
            }
            if p.virt && !s.virt {
                p.width = s.width;
                p.color = s.color;
            } else if !s.virt {
                p.width = p.width.max(s.width);
            }
            p.virt &= s.virt;
            p.srcs.extend(s.srcs);
        } else {
            out.push(s);
        }
    }
    out
}

/// Cluster sorted positions closer than `TOL` into their mean.
fn cluster(mut v: Vec<f32>) -> Vec<f32> {
    v.sort_by(f32::total_cmp);
    let mut out: Vec<(f32, f32)> = Vec::new(); // (sum, count)
    for x in v {
        match out.last_mut() {
            Some((s, n)) if x - *s / *n <= TOL => {
                *s += x;
                *n += 1.0;
            }
            _ => out.push((x, 1.0)),
        }
    }
    out.into_iter().map(|(s, n)| s / n).collect()
}

/// The segment that covers at least 70% of [lo, hi] on line `at`.
fn covering<'a>(segs: &[&'a Seg], horizontal: bool, at: f32, lo: f32, hi: f32) -> Option<&'a Seg> {
    let need = (hi - lo) * 0.7;
    segs.iter()
        .copied()
        .filter(|s| s.horizontal == horizontal && (s.at - at).abs() <= TOL)
        .find(|s| s.hi.min(hi) - s.lo.max(lo) >= need)
}

fn border(s: Option<&Seg>) -> Option<Border> {
    s.filter(|s| !s.virt).map(|s| Border {
        width: s.width,
        color: s.color,
    })
}

/// Find lattice tables among a page's marks. `consumed` marks are ignored.
pub fn find(marks: &[Mark], consumed: &[bool]) -> Vec<Grid> {
    let segs = merge(segments(marks, consumed));
    if segs.len() > 6000 {
        return Vec::new();
    }
    let n = segs.len();
    let mut uf = Uf::new(n);
    for i in 0..n {
        if !segs[i].horizontal {
            continue;
        }
        for j in 0..n {
            let (h, v) = (&segs[i], &segs[j]);
            if v.horizontal {
                continue;
            }
            if v.at >= h.lo - TOL && v.at <= h.hi + TOL && h.at >= v.lo - TOL && h.at <= v.hi + TOL
            {
                uf.union(i, j);
            }
        }
    }
    let mut comps: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        comps.entry(uf.find(i)).or_default().push(i);
    }
    let mut grids = Vec::new();
    for members in comps.values() {
        let comp: Vec<&Seg> = members.iter().map(|&i| &segs[i]).collect();
        if let Some(g) = grid(&comp, marks) {
            grids.push(g);
        }
    }
    grids
}

fn grid(comp: &[&Seg], marks: &[Mark]) -> Option<Grid> {
    let (hs, vs): (Vec<&Seg>, Vec<&Seg>) = comp.iter().partition(|s| s.horizontal);
    if hs.is_empty() || vs.is_empty() {
        return None;
    }
    let virt_only = comp.iter().all(|s| s.virt);
    let mut xs: Vec<f32> = vs.iter().map(|s| s.at).collect();
    xs.push(hs.iter().map(|s| s.lo).fold(f32::INFINITY, f32::min));
    xs.push(hs.iter().map(|s| s.hi).fold(f32::NEG_INFINITY, f32::max));
    let mut ys: Vec<f32> = hs.iter().map(|s| s.at).collect();
    ys.push(vs.iter().map(|s| s.lo).fold(f32::INFINITY, f32::min));
    ys.push(vs.iter().map(|s| s.hi).fold(f32::NEG_INFINITY, f32::max));
    let xs = cluster(xs);
    let ys = cluster(ys);
    let (nc, nr) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
    if nc == 0 || nr == 0 || nc * nr < 2 || nc > 64 || nr > 400 {
        return None;
    }
    if virt_only && (nr < 2 || nc * nr < 4) {
        return None;
    }
    let (w, h) = (xs[nc] - xs[0], ys[nr] - ys[0]);
    if w < 20.0 || h < 8.0 {
        return None;
    }
    let all: Vec<&Seg> = comp.to_vec();
    let vline = |r: usize, c: usize| covering(&all, false, xs[c], ys[r], ys[r + 1]);
    let hline = |r: usize, c: usize| covering(&all, true, ys[r], xs[c], xs[c + 1]);

    // Merge cells across missing interior boundaries; fall back to row spans only if the
    // result isn't made of rectangles.
    let cells =
        merged(nr, nc, true, &vline, &hline).or_else(|| merged(nr, nc, false, &vline, &hline))?;

    let mut consumed: Vec<usize> = comp
        .iter()
        .filter(|s| !s.virt)
        .flat_map(|s| s.srcs.iter().copied())
        .collect();
    let mut out = Vec::with_capacity(cells.len());
    for (r0, r1, c0, c1) in cells {
        let bb = BBox::new(xs[c0], ys[r0], xs[c1], ys[r1]);
        let top = border(covering(&all, true, ys[r0], xs[c0], xs[c1]));
        let bottom = border(covering(&all, true, ys[r1], xs[c0], xs[c1]));
        let left = border(covering(&all, false, xs[c0], ys[r0], ys[r1]));
        let right = border(covering(&all, false, xs[c1], ys[r0], ys[r1]));
        // A filled box that covers the cell is its shading.
        let mut fill = None;
        for (i, m) in marks.iter().enumerate() {
            if let Mark::Box { bbox, fill: f } = m
                && bbox.overlap(&bb) >= 0.8 * bb.area()
                && bbox.area() <= 1.3 * bb.area()
            {
                fill = Some(*f);
                consumed.push(i);
            }
        }
        out.push(GridCell {
            r0,
            r1,
            c0,
            c1,
            borders: [top, left, bottom, right],
            fill,
        });
    }
    consumed.sort_unstable();
    consumed.dedup();
    Some(Grid {
        xs,
        ys,
        cells: out,
        consumed,
    })
}

type Edge<'a> = dyn Fn(usize, usize) -> Option<&'a Seg> + 'a;

/// Cell groups as (r0, r1, c0, c1), or None if some group isn't a rectangle.
fn merged(
    nr: usize,
    nc: usize,
    vertical: bool,
    vline: &Edge<'_>,
    hline: &Edge<'_>,
) -> Option<Vec<(usize, usize, usize, usize)>> {
    let mut uf = Uf::new(nr * nc);
    for r in 0..nr {
        for c in 0..nc {
            if c + 1 < nc && vline(r, c + 1).is_none() {
                uf.union(r * nc + c, r * nc + c + 1);
            }
            if vertical && r + 1 < nr && hline(r + 1, c).is_none() {
                uf.union(r * nc + c, (r + 1) * nc + c);
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, (usize, usize, usize, usize, usize)> =
        Default::default();
    for r in 0..nr {
        for c in 0..nc {
            let g = groups.entry(uf.find(r * nc + c)).or_insert((r, r, c, c, 0));
            g.0 = g.0.min(r);
            g.1 = g.1.max(r);
            g.2 = g.2.min(c);
            g.3 = g.3.max(c);
            g.4 += 1;
        }
    }
    let mut out = Vec::new();
    for &(r0, r1, c0, c1, n) in groups.values() {
        if n != (r1 - r0 + 1) * (c1 - c0 + 1) {
            return None;
        }
        out.push((r0, r1 + 1, c0, c1 + 1));
    }
    out.sort_unstable_by_key(|&(r0, _, c0, _)| (r0, c0));
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(horizontal: bool, at: f32, lo: f32, hi: f32) -> Mark {
        Mark::Rule {
            horizontal,
            at,
            lo,
            hi,
            width: 0.5,
            color: [0, 0, 0],
        }
    }

    #[test]
    fn a_three_by_two_grid_with_a_merged_header() {
        // Columns at x = 0, 100, 200, 300; rows at y = 0, 20, 40.
        // The header row (0..20) has no vertical line at x = 100 or 200: one merged cell.
        let mut m = vec![
            rule(true, 0.0, 0.0, 300.0),
            rule(true, 20.0, 0.0, 300.0),
            rule(true, 40.0, 0.0, 300.0),
            rule(false, 0.0, 0.0, 40.0),
            rule(false, 300.0, 0.0, 40.0),
            rule(false, 100.0, 20.0, 40.0),
            rule(false, 200.0, 20.0, 40.0),
        ];
        m.push(Mark::Box {
            bbox: BBox::new(0.0, 0.0, 300.0, 20.0),
            fill: [200, 220, 240],
        });
        let g = find(&m, &vec![false; m.len()]);
        assert_eq!(g.len(), 1);
        let g = &g[0];
        assert_eq!(g.xs.len(), 4);
        assert_eq!(g.ys.len(), 3);
        assert_eq!(g.cells.len(), 4, "one merged header + three body cells");
        assert_eq!((g.cells[0].c0, g.cells[0].c1), (0, 3));
        assert_eq!(g.cells[0].fill, Some([200, 220, 240]));
        assert!(g.cells[1].borders.iter().all(Option::is_some));
        assert_eq!(g.cell_at(150.0, 30.0), Some(2));
        assert!(
            g.consumed.contains(&7),
            "the header fill is drawn by the table"
        );
    }

    #[test]
    fn lone_lines_and_boxes_are_not_tables() {
        let m = vec![
            rule(true, 10.0, 0.0, 500.0),
            Mark::Box {
                bbox: BBox::new(0.0, 100.0, 200.0, 400.0),
                fill: [240, 240, 240],
            },
        ];
        assert!(find(&m, &[false, false]).is_empty());
    }

    #[test]
    fn missing_outer_edges_come_from_segment_extents() {
        // Only horizontal rules plus one interior vertical: a 2×2 grid.
        let m = vec![
            rule(true, 0.0, 0.0, 200.0),
            rule(true, 20.0, 0.0, 200.0),
            rule(true, 40.0, 0.0, 200.0),
            rule(false, 100.0, 0.0, 40.0),
        ];
        let g = find(&m, &[false; 4]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].xs, vec![0.0, 100.0, 200.0]);
        assert_eq!(g[0].cells.len(), 4);
        assert!(
            g[0].cells[0].borders[1].is_none(),
            "no left border was drawn"
        );
    }
}
