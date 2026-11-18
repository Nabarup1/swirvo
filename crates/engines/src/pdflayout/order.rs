//! Reading order (docs/13 §4.2): recursive XY-cut over atoms. Columns need a whitespace river
//! through the whole region, and a split that looks like a label/value table is vetoed so its
//! rows stay together.

use super::model::BBox;

#[derive(Clone, Debug)]
pub struct Atom {
    pub bbox: BBox,
    /// Text atoms carry their row id: atoms sharing a row share a baseline.
    pub row: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Leaf(Vec<usize>),
    Rows(Vec<Node>),
    Cols(Vec<Node>),
}

/// A band of the page: columns side by side, each a sequence of leaf groups.
#[derive(Clone, Debug, PartialEq)]
pub struct BandSpec {
    pub cols: Vec<Vec<Vec<usize>>>,
}

const MIN_GUTTER: f32 = 9.0;

fn extent(atoms: &[Atom], ids: &[usize]) -> BBox {
    ids.iter().fold(BBox::EMPTY, |b, &i| b.union(atoms[i].bbox))
}

/// Gaps between the merged projections of `ids` on one axis, widest first.
fn gaps(atoms: &[Atom], ids: &[usize], vertical: bool, min: f32) -> Vec<(f32, f32)> {
    let mut iv: Vec<(f32, f32)> = ids
        .iter()
        .map(|&i| {
            let b = atoms[i].bbox;
            if vertical { (b.x0, b.x1) } else { (b.y0, b.y1) }
        })
        .collect();
    iv.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = Vec::new();
    let mut end = f32::NEG_INFINITY;
    for (a, b) in iv {
        if end > f32::NEG_INFINITY && a - end >= min {
            out.push((end, a));
        }
        end = end.max(b);
    }
    out.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
    out
}

fn rows_of(atoms: &[Atom], ids: &[usize]) -> Vec<usize> {
    let mut r: Vec<usize> = ids.iter().filter_map(|&i| atoms[i].row).collect();
    r.sort_unstable();
    r.dedup();
    r
}

fn side_ok(atoms: &[Atom], ids: &[usize]) -> bool {
    rows_of(atoms, ids).len() >= 2
        || ids
            .iter()
            .any(|&i| atoms[i].row.is_none() && atoms[i].bbox.h() >= 30.0)
}

fn valid_split(atoms: &[Atom], left: &[usize], right: &[usize], region: &BBox) -> bool {
    let (lb, rb) = (extent(atoms, left), extent(atoms, right));
    let minw = (0.15 * region.w()).max(50.0);
    if lb.w() < minw || rb.w() < minw || !side_ok(atoms, left) || !side_ok(atoms, right) {
        return false;
    }
    // Veto: short lines on one side that mostly share baselines with the other side are the
    // labels of a table or form, not a column.
    let (rl, rr) = (rows_of(atoms, left), rows_of(atoms, right));
    if rl.is_empty() || rr.is_empty() {
        return true;
    }
    let (small, small_box, other) = if rl.len() <= rr.len() {
        (left, lb, &rr)
    } else {
        (right, rb, &rl)
    };
    let small_rows = rows_of(atoms, small);
    let shared = small_rows
        .iter()
        .filter(|r| other.binary_search(r).is_ok())
        .count();
    let frac = shared as f32 / small_rows.len() as f32;
    let mut widths: Vec<f32> = small
        .iter()
        .filter(|&&i| atoms[i].row.is_some())
        .map(|&i| atoms[i].bbox.w())
        .collect();
    widths.sort_by(f32::total_cmp);
    let median = widths.get(widths.len() / 2).copied().unwrap_or(0.0);
    let short = median < 0.55 * small_box.w() || small_box.w() < 0.35 * region.w();
    !(frac >= 0.7 && short)
}

/// Recursive XY-cut. `line_gap` is the smallest full-width gap that separates bands.
pub fn cut(atoms: &[Atom], ids: Vec<usize>, line_gap: f32) -> Node {
    cut_at(atoms, ids, line_gap, 0)
}

fn cut_at(atoms: &[Atom], ids: Vec<usize>, line_gap: f32, depth: u8) -> Node {
    if ids.len() <= 1 || depth > 24 {
        return Node::Leaf(ids);
    }
    let region = extent(atoms, &ids);
    for (a, b) in gaps(atoms, &ids, true, MIN_GUTTER) {
        let mid = (a + b) / 2.0;
        let (left, right): (Vec<usize>, Vec<usize>) =
            ids.iter().partition(|&&i| atoms[i].bbox.cx() < mid);
        if valid_split(atoms, &left, &right, &region) {
            let mut cols = Vec::new();
            for part in [left, right] {
                match cut_at(atoms, part, line_gap, depth + 1) {
                    Node::Cols(c) => cols.extend(c),
                    n => cols.push(n),
                }
            }
            return Node::Cols(cols);
        }
    }
    let mut hg: Vec<f32> = gaps(atoms, &ids, false, line_gap)
        .into_iter()
        .map(|(a, b)| (a + b) / 2.0)
        .collect();
    if hg.is_empty() {
        return Node::Leaf(ids);
    }
    hg.sort_by(f32::total_cmp);
    let mut parts: Vec<Vec<usize>> = vec![Vec::new(); hg.len() + 1];
    for &i in &ids {
        let k = hg.partition_point(|&y| y < atoms[i].bbox.cy());
        parts[k].push(i);
    }
    let mut rows = Vec::new();
    for p in parts.into_iter().filter(|p| !p.is_empty()) {
        match cut_at(atoms, p, line_gap, depth + 1) {
            Node::Rows(r) => rows.extend(r),
            n => rows.push(n),
        }
    }
    Node::Rows(rows)
}

fn leaves(atoms: &[Atom], n: Node, out: &mut Vec<Vec<usize>>) {
    match n {
        Node::Leaf(ids) => out.push(ids),
        Node::Rows(ch) => {
            for c in ch {
                leaves(atoms, c, out);
            }
        }
        Node::Cols(ch) => {
            // Word can't nest columns: inner columns become one group (tab-stop lines).
            let mut all = Vec::new();
            for c in ch {
                let mut sub = Vec::new();
                leaves(atoms, c, &mut sub);
                all.extend(sub.into_iter().flatten());
            }
            all.sort_by(|&a, &b| atoms[a].bbox.y0.total_cmp(&atoms[b].bbox.y0));
            out.push(all);
        }
    }
}

/// Flatten the tree into bands; neighbouring single-column bands are merged.
pub fn bands(atoms: &[Atom], n: Node) -> Vec<BandSpec> {
    let mut raw: Vec<BandSpec> = Vec::new();
    flatten(atoms, n, &mut raw);
    let mut out: Vec<BandSpec> = Vec::new();
    for b in raw {
        match out.last_mut() {
            Some(prev) if prev.cols.len() == 1 && b.cols.len() == 1 => {
                let mut b = b;
                prev.cols[0].append(&mut b.cols[0]);
            }
            _ => out.push(b),
        }
    }
    out
}

fn flatten(atoms: &[Atom], n: Node, out: &mut Vec<BandSpec>) {
    match n {
        Node::Leaf(ids) => out.push(BandSpec {
            cols: vec![vec![ids]],
        }),
        Node::Rows(ch) => {
            for c in ch {
                flatten(atoms, c, out);
            }
        }
        Node::Cols(ch) => {
            let cols = ch
                .into_iter()
                .map(|c| {
                    let mut g = Vec::new();
                    leaves(atoms, c, &mut g);
                    g
                })
                .collect();
            out.push(BandSpec { cols });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(x0: f32, x1: f32, base: f32, row: usize) -> Atom {
        Atom {
            bbox: BBox::new(x0, base - 8.0, x1, base + 2.0),
            row: Some(row),
        }
    }

    #[test]
    fn title_then_two_columns() {
        let mut a = vec![text(100.0, 500.0, 50.0, 0)]; // title across the gutter
        for k in 0..10 {
            let y = 100.0 + 12.0 * k as f32;
            a.push(text(50.0, 290.0, y, 10 + k)); // left column
            a.push(text(310.0, 550.0, y, 10 + k)); // right column, same baselines
        }
        let ids: Vec<usize> = (0..a.len()).collect();
        let b = bands(&a, cut(&a, ids, 6.0));
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].cols.len(), 1);
        assert_eq!(b[1].cols.len(), 2, "long lines on both sides: real columns");
        let left: Vec<usize> = b[1].cols[0].iter().flatten().copied().collect();
        assert!(left.iter().all(|&i| a[i].bbox.x0 < 100.0));
        assert_eq!(left.len(), 10);
    }

    #[test]
    fn labels_and_values_stay_on_their_rows() {
        let mut a = Vec::new();
        for k in 0..8 {
            let y = 100.0 + 14.0 * k as f32;
            a.push(text(50.0, 130.0, y, k)); // short label
            a.push(text(250.0, 520.0, y, k)); // value
        }
        let ids: Vec<usize> = (0..a.len()).collect();
        let b = bands(&a, cut(&a, ids, 6.0));
        assert!(b.iter().all(|b| b.cols.len() == 1), "{b:?}");
    }

    #[test]
    fn picture_beside_text() {
        let mut a = vec![Atom {
            bbox: BBox::new(50.0, 100.0, 200.0, 250.0),
            row: None,
        }];
        for k in 0..10 {
            a.push(text(220.0, 550.0, 110.0 + 13.0 * k as f32, k));
        }
        let ids: Vec<usize> = (0..a.len()).collect();
        let b = bands(&a, cut(&a, ids, 6.0));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].cols.len(), 2);
    }
}
