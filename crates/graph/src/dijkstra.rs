use std::cmp::Reverse;
use std::collections::BinaryHeap;

use swirvo_core::{Cost, Preset};

type Key = (u32, u32, u32, u32);

/// Dijkstra over an implicit graph with lexicographic costs.
///
/// Correct because every component of [`Cost`] is non-negative and the preset's key is a
/// lexicographic order over sums, i.e. a totally ordered monoid where adding an edge never
/// decreases the key. The hop count is the last key component, so ties prefer short routes.
///
/// `out(u)` yields edge ids leaving `u`; `edge(e)` gives `(target, cost)`.
/// Returns the edge ids along the best path and its total cost.
pub fn shortest_path<I, O, E>(
    n: usize,
    src: usize,
    dst: usize,
    out: O,
    edge: E,
    preset: Preset,
) -> Option<(Vec<u32>, Cost)>
where
    I: Iterator<Item = u32>,
    O: Fn(usize) -> I,
    E: Fn(u32) -> (usize, Cost),
{
    let mut best: Vec<Option<(Key, Cost, u32)>> = vec![None; n]; // (key, cost, hops)
    let mut prev: Vec<Option<(usize, u32)>> = vec![None; n]; // (node, edge)
    let mut heap = BinaryHeap::new();
    best[src] = Some((preset.rank(Cost::default(), 0), Cost::default(), 0));
    heap.push(Reverse((preset.rank(Cost::default(), 0), src)));

    while let Some(Reverse((key, u))) = heap.pop() {
        let (ukey, ucost, uhops) = best[u].expect("pushed nodes have a best entry");
        if key > ukey {
            continue; // stale heap entry
        }
        if u == dst {
            break;
        }
        for e in out(u) {
            let (v, c) = edge(e);
            let cost = ucost + c;
            let hops = uhops + 1;
            let k = preset.rank(cost, hops);
            if best[v].is_none_or(|(bk, _, _)| k < bk) {
                best[v] = Some((k, cost, hops));
                prev[v] = Some((u, e));
                heap.push(Reverse((k, v)));
            }
        }
    }

    let (_, cost, _) = best[dst]?;
    let mut path = Vec::new();
    let mut cur = dst;
    while cur != src {
        let (p, e) = prev[cur]?;
        path.push(e);
        cur = p;
    }
    path.reverse();
    Some((path, cost))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Exhaustive search over simple paths: the oracle for small graphs.
    fn brute(
        n: usize,
        edges: &[(usize, usize, Cost)],
        src: usize,
        dst: usize,
        p: Preset,
    ) -> Option<Key> {
        #[allow(clippy::too_many_arguments)]
        fn go(
            u: usize,
            dst: usize,
            edges: &[(usize, usize, Cost)],
            seen: &mut Vec<bool>,
            cost: Cost,
            hops: u32,
            p: Preset,
            best: &mut Option<Key>,
        ) {
            if u == dst {
                let k = p.rank(cost, hops);
                if best.is_none_or(|b| k < b) {
                    *best = Some(k);
                }
                return;
            }
            for &(a, b, c) in edges {
                if a == u && !seen[b] {
                    seen[b] = true;
                    go(b, dst, edges, seen, cost + c, hops + 1, p, best);
                    seen[b] = false;
                }
            }
        }
        let mut seen = vec![false; n];
        seen[src] = true;
        let mut best = None;
        go(src, dst, edges, &mut seen, Cost::default(), 0, p, &mut best);
        best
    }

    fn cost() -> impl Strategy<Value = Cost> {
        (0u32..4, 0u32..2, 0u32..50).prop_map(|(loss, download, time_ms)| Cost {
            loss,
            download,
            time_ms,
        })
    }

    proptest! {
        #[test]
        fn optimal_against_brute_force(
            n in 2usize..7,
            raw in proptest::collection::vec((0usize..7, 0usize..7, cost()), 0..18),
            preset in prop_oneof![Just(Preset::Balanced), Just(Preset::Best), Just(Preset::Fastest)],
        ) {
            let edges: Vec<(usize, usize, Cost)> = raw.into_iter().map(|(a, b, c)| (a % n, b % n, c)).filter(|(a, b, _)| a != b).collect();
            let got = shortest_path(n, 0, n - 1,
                |u| edges.iter().enumerate().filter(move |(_, e)| e.0 == u).map(|(i, _)| i as u32).collect::<Vec<_>>().into_iter(),
                |e| { let (_, b, c) = edges[e as usize]; (b, c) },
                preset);
            let want = brute(n, &edges, 0, n - 1, preset);
            match (got, want) {
                (None, None) => {}
                (Some((path, c)), Some(k)) => {
                    prop_assert_eq!(preset.rank(c, path.len() as u32), k);
                    // The path is connected and ends at dst.
                    let mut cur = 0;
                    for e in &path { prop_assert_eq!(edges[*e as usize].0, cur); cur = edges[*e as usize].1; }
                    prop_assert_eq!(cur, n - 1);
                }
                (g, w) => prop_assert!(false, "mismatch: got {:?}, want {:?}", g, w),
            }
        }
    }
}
