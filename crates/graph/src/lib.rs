//! The conversion graph (docs/04 §2).
//!
//! Nodes are formats, edges are engine capabilities. Two structures answer the two
//! questions the product asks:
//!
//! * **"What can these files become?"** (opening the wheel): `REACH[s]` is a 256-bit set
//!   precomputed per source format, and several files AND their sets together. O(k).
//! * **"How do I get from A to B?"** (on drop): Dijkstra with a preset-dependent
//!   lexicographic cost, so lossless routes beat lossy ones and installed engines beat
//!   downloads. V ≈ 100, E ≈ 300, so a query takes microseconds.

mod bitset;
mod dijkstra;
mod sensible;

use std::collections::HashSet;
use std::fmt::Write as _;

use swirvo_core::{Capability, Cost, FormatId, Preset};
use swirvo_formats::Registry;

pub use bitset::FormatSet;
pub use dijkstra::shortest_path;
pub use sensible::sensible;

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("engine '{engine}' declares unknown format '{key}'")]
    UnknownFormat {
        engine: &'static str,
        key: &'static str,
    },
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: FormatId,
    pub to: FormatId,
    pub cap: Capability,
}

/// A planned route: the edges to run, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub edges: Vec<u32>,
    pub cost: Cost,
}

pub struct Graph {
    registry: &'static Registry,
    edges: Vec<Edge>,
    out: Vec<Vec<u32>>,
    reach: Vec<FormatSet>,
    installed_packs: HashSet<String>,
}

impl Graph {
    /// Build the graph and precompute reachability. Run again whenever packs, hardware
    /// or settings change; it takes well under a millisecond.
    pub fn build(
        registry: &'static Registry,
        caps: impl IntoIterator<Item = Capability>,
        installed_packs: &[&str],
    ) -> Result<Graph, GraphError> {
        let n = registry.len();
        let mut edges = Vec::new();
        let mut out = vec![Vec::new(); n];
        for cap in caps {
            let from = registry.id(cap.from).ok_or(GraphError::UnknownFormat {
                engine: cap.engine,
                key: cap.from,
            })?;
            let to = registry.id(cap.to).ok_or(GraphError::UnknownFormat {
                engine: cap.engine,
                key: cap.to,
            })?;
            if from == to {
                continue;
            }
            out[from.index()].push(edges.len() as u32);
            edges.push(Edge { from, to, cap });
        }
        let installed_packs = installed_packs.iter().map(|s| s.to_string()).collect();
        let mut g = Graph {
            registry,
            edges,
            out,
            reach: Vec::new(),
            installed_packs,
        };
        g.reach = (0..n)
            .map(|s| {
                let from = registry.get(FormatId(s as u16));
                g.bfs_reach(s)
                    .iter()
                    .filter(|&t| sensible(from, registry.get(t)))
                    .collect()
            })
            .collect();
        Ok(g)
    }

    fn bfs_reach(&self, src: usize) -> FormatSet {
        let mut seen = FormatSet::default();
        let mut stack = vec![src];
        while let Some(u) = stack.pop() {
            for &e in &self.out[u] {
                let v = self.edges[e as usize].to.index();
                if v != src && !seen.contains(FormatId(v as u16)) {
                    seen.insert(FormatId(v as u16));
                    stack.push(v);
                }
            }
        }
        seen
    }

    pub fn registry(&self) -> &'static Registry {
        self.registry
    }
    pub fn edge(&self, i: u32) -> &Edge {
        &self.edges[i as usize]
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Formats reachable from `from`, excluding itself.
    pub fn reach(&self, from: FormatId) -> FormatSet {
        self.reach[from.index()]
    }

    /// Targets every input can reach: the wheel's candidate set for a multi-file drop.
    pub fn common_targets(&self, inputs: &[FormatId]) -> FormatSet {
        let mut it = inputs.iter();
        let Some(&first) = it.next() else {
            return FormatSet::default();
        };
        let mut set = self.reach(first);
        for &f in it {
            // A file already in format f can "become" f trivially; keep f if others reach it.
            let mut r = self.reach(f);
            r.insert(f);
            set = set.and(r);
        }
        set
    }

    /// Cheapest route under `preset` for an input of `size_mb` megabytes.
    pub fn route(
        &self,
        from: FormatId,
        to: FormatId,
        preset: Preset,
        size_mb: f64,
    ) -> Option<Route> {
        self.route_avoiding(from, to, preset, size_mb, &[])
    }

    /// The cheapest route that uses none of the engines in `avoid`: the fallback after an
    /// engine has failed on this file.
    pub fn route_avoiding(
        &self,
        from: FormatId,
        to: FormatId,
        preset: Preset,
        size_mb: f64,
        avoid: &[&str],
    ) -> Option<Route> {
        if from == to || !sensible(self.registry.get(from), self.registry.get(to)) {
            return None;
        }
        let n = self.registry.len();
        let (edges, cost) = shortest_path(
            n,
            from.index(),
            to.index(),
            |u| {
                self.out[u]
                    .iter()
                    .copied()
                    .filter(|&e| !avoid.contains(&self.edges[e as usize].cap.engine))
            },
            |e| {
                let edge = &self.edges[e as usize];
                let installed = edge
                    .cap
                    .pack
                    .is_none_or(|p| self.installed_packs.contains(p));
                (edge.to.index(), edge.cap.cost(size_mb, installed))
            },
            preset,
        )?;
        Some(Route { edges, cost })
    }

    /// One-line explanation: `heic → png (image, lossless) → jpg (image, lossy 30)`.
    pub fn explain(&self, route: &Route) -> String {
        let mut s = String::new();
        if let Some(&first) = route.edges.first() {
            s.push_str(self.registry.key(self.edge(first).from));
        }
        for &e in &route.edges {
            let edge = self.edge(e);
            let quality = if edge.cap.loss == 0 {
                "lossless".to_string()
            } else {
                format!("lossy {}", edge.cap.loss)
            };
            let _ = write!(
                s,
                " → {} ({}, {quality}",
                self.registry.key(edge.to),
                edge.cap.engine
            );
            if !edge.cap.note.is_empty() {
                let _ = write!(s, "; {}", edge.cap.note);
            }
            s.push(')');
        }
        s
    }
}

impl Route {
    pub fn is_lossless(&self) -> bool {
        self.cost.loss == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_formats::registry;

    fn caps() -> Vec<Capability> {
        vec![
            Capability::new("img", "heic", "png"),
            Capability::new("img", "png", "jpg").loss(30),
            Capability::new("img", "heic", "jpg")
                .loss(30)
                .pack("Media")
                .time(5, 1),
            Capability::new("img", "png", "webp"),
            Capability::new("doc", "md", "html"),
        ]
    }

    #[test]
    fn route_avoiding_skips_a_failed_engine() {
        let r = registry();
        let caps = vec![
            Capability::new("fast", "docx", "pdf").loss(1).time(60, 40),
            Capability::new("slow", "docx", "pdf")
                .loss(1)
                .time(2500, 300),
            Capability::new("reflow", "docx", "pdf").loss(15),
        ];
        let g = Graph::build(r, caps, &[]).unwrap();
        let (docx, pdf) = (r.id("docx").unwrap(), r.id("pdf").unwrap());
        let engine = |route: Option<Route>| route.map(|x| g.edge(x.edges[0]).cap.engine);
        assert_eq!(
            engine(g.route(docx, pdf, Preset::Balanced, 1.0)),
            Some("fast")
        );
        let one = g.route_avoiding(docx, pdf, Preset::Balanced, 1.0, &["fast"]);
        assert_eq!(engine(one), Some("slow"));
        let two = g.route_avoiding(docx, pdf, Preset::Balanced, 1.0, &["fast", "slow"]);
        assert_eq!(engine(two), Some("reflow"));
        let none = g.route_avoiding(
            docx,
            pdf,
            Preset::Balanced,
            1.0,
            &["fast", "slow", "reflow"],
        );
        assert!(none.is_none());
    }

    #[test]
    fn prefers_installed_route_when_loss_ties() {
        let r = registry();
        let g = Graph::build(r, caps(), &[]).unwrap();
        let (heic, jpg) = (r.id("heic").unwrap(), r.id("jpg").unwrap());
        let route = g.route(heic, jpg, Preset::Balanced, 3.0).unwrap();
        // Same loss (30) either way, so no-download wins over the faster pack route.
        assert_eq!(route.edges.len(), 2);
        assert_eq!(
            g.explain(&route),
            "heic → png (img, lossless) → jpg (img, lossy 30)"
        );

        let g2 = Graph::build(r, caps(), &["Media"]).unwrap();
        assert_eq!(
            g2.route(heic, jpg, Preset::Balanced, 3.0)
                .unwrap()
                .edges
                .len(),
            1
        );
    }

    #[test]
    fn reachability_and_intersection() {
        let r = registry();
        let g = Graph::build(r, caps(), &[]).unwrap();
        let id = |k| r.id(k).unwrap();
        let reach = g.reach(id("heic"));
        assert!(
            reach.contains(id("webp")) && reach.contains(id("jpg")) && !reach.contains(id("heic"))
        );
        let common = g.common_targets(&[id("heic"), id("png")]);
        assert!(
            common.contains(id("jpg"))
                && common.contains(id("png"))
                && !common.contains(id("html"))
        );
        assert!(g.common_targets(&[id("heic"), id("md")]).is_empty());
    }

    #[test]
    fn unknown_format_is_an_error() {
        let bad = vec![Capability::new("x", "nope", "png")];
        assert!(Graph::build(registry(), bad, &[]).is_err());
    }
}
