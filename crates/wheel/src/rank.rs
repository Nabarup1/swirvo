//! Decayed frecency and stable slot assignment (docs/04 §3).

use std::collections::HashMap;

/// Milliseconds since an arbitrary epoch.
pub type Millis = u64;

/// Exponentially decayed use counts. Two numbers per key, so there is no history to scan:
/// `score ← score · 2^(−Δt/τ) + 1` on every use.
#[derive(Debug, Clone)]
pub struct Frecency {
    half_life_ms: f64,
    entries: HashMap<(String, String), (f64, Millis)>,
}

impl Default for Frecency {
    fn default() -> Self {
        Frecency::new(14 * 24 * 3600 * 1000)
    }
}

impl Frecency {
    pub fn new(half_life_ms: Millis) -> Self {
        Frecency {
            half_life_ms: half_life_ms as f64,
            entries: HashMap::new(),
        }
    }

    fn decay(&self, score: f64, since: Millis, now: Millis) -> f64 {
        let dt = now.saturating_sub(since) as f64;
        score * (-dt / self.half_life_ms).exp2()
    }

    pub fn bump(&mut self, ctx: &str, target: &str, now: Millis) {
        let half_life = self.half_life_ms;
        let e = self
            .entries
            .entry((ctx.to_string(), target.to_string()))
            .or_insert((0.0, now));
        let dt = now.saturating_sub(e.1) as f64;
        e.0 = e.0 * (-dt / half_life).exp2() + 1.0;
        e.1 = now;
    }

    pub fn score(&self, ctx: &str, target: &str, now: Millis) -> f64 {
        self.entries
            .get(&(ctx.to_string(), target.to_string()))
            .map_or(0.0, |&(s, t)| self.decay(s, t, now))
    }

    pub fn entries(&self) -> impl Iterator<Item = (&str, &str, f64, Millis)> {
        self.entries
            .iter()
            .map(|((c, t), &(s, at))| (c.as_str(), t.as_str(), s, at))
    }

    pub fn restore(&mut self, ctx: &str, target: &str, score: f64, at: Millis) {
        self.entries
            .insert((ctx.to_string(), target.to_string()), (score, at));
    }
}

/// Challenger must beat the incumbent by this factor before it may take its slot.
pub const SWAP_FACTOR: f64 = 1.25;

/// Assign ranked targets to wheel slots without wrecking muscle memory.
///
/// 1. Pinned slots never move.
/// 2. Targets already showing keep their slot.
/// 3. If `allow_swap` (at most once per context per day), the single best challenger
///    replaces the weakest incumbent, but only when it scores more than 1.25× higher.
/// 4. Empty slots fill with the best remaining targets, nearest 12 o'clock first.
pub fn assign_slots(
    capacity: usize,
    previous: &[Option<String>],
    pinned: &[(usize, String)],
    ranked: &[(String, f64)],
    allow_swap: bool,
) -> Vec<Option<String>> {
    let mut slots: Vec<Option<String>> = vec![None; capacity];
    let mut is_pinned = vec![false; capacity];
    for (i, t) in pinned {
        if *i < capacity && ranked.iter().any(|(r, _)| r == t) {
            slots[*i] = Some(t.clone());
            is_pinned[*i] = true;
        }
    }
    let score = |t: &str| ranked.iter().find(|(r, _)| r == t).map(|(_, s)| *s);
    let placed = |slots: &[Option<String>], t: &str| slots.iter().any(|s| s.as_deref() == Some(t));

    for (i, prev) in previous.iter().enumerate().take(capacity) {
        if let Some(t) = prev
            && !is_pinned[i]
            && score(t).is_some()
            && !placed(&slots, t)
        {
            slots[i] = Some(t.clone());
        }
    }

    // Best candidate not yet placed (ranked is sorted by the caller, but don't rely on it).
    let mut pool: Vec<&(String, f64)> = ranked.iter().filter(|(t, _)| !placed(&slots, t)).collect();
    pool.sort_by(|a, b| b.1.total_cmp(&a.1));

    if allow_swap
        && slots.iter().all(Option::is_some)
        && let Some(&(challenger, cs)) = pool.first().map(|c| (&c.0, c.1)).as_ref()
    {
        let weakest = (0..capacity)
            .filter(|&i| !is_pinned[i])
            .filter_map(|i| slots[i].as_deref().and_then(score).map(|s| (i, s)))
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((i, s)) = weakest
            && cs > s * SWAP_FACTOR
        {
            slots[i] = Some(challenger.clone());
            pool.remove(0);
        }
    }

    let mut rest = pool.into_iter();
    for slot in slots.iter_mut() {
        if slot.is_none() {
            match rest.next() {
                Some((t, _)) => *slot = Some(t.clone()),
                None => break,
            }
        }
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn r(items: &[(&str, f64)]) -> Vec<(String, f64)> {
        items.iter().map(|(t, s)| (t.to_string(), *s)).collect()
    }

    #[test]
    fn half_life() {
        let mut f = Frecency::new(1000);
        f.bump("heic", "jpg", 0);
        assert!((f.score("heic", "jpg", 1000) - 0.5).abs() < 1e-9);
        f.bump("heic", "jpg", 1000);
        assert!((f.score("heic", "jpg", 1000) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn incumbents_stay_without_swap() {
        let prev = vec![Some("png".into()), Some("jpg".into())];
        let out = assign_slots(
            2,
            &prev,
            &[],
            &r(&[("jpg", 9.0), ("webp", 8.0), ("png", 1.0)]),
            false,
        );
        assert_eq!(out, vec![Some("png".into()), Some("jpg".into())]);
    }

    #[test]
    fn one_swap_needs_clear_lead() {
        let prev = vec![Some("png".into()), Some("jpg".into())];
        let small_lead = assign_slots(
            2,
            &prev,
            &[],
            &r(&[("jpg", 9.0), ("webp", 1.2), ("png", 1.0)]),
            true,
        );
        assert_eq!(small_lead[0].as_deref(), Some("png"));
        let big_lead = assign_slots(
            2,
            &prev,
            &[],
            &r(&[("jpg", 9.0), ("webp", 8.0), ("png", 1.0)]),
            true,
        );
        assert_eq!(big_lead, vec![Some("webp".into()), Some("jpg".into())]);
    }

    #[test]
    fn pins_win() {
        let out = assign_slots(
            3,
            &[],
            &[(0, "jxl".into())],
            &r(&[("jpg", 9.0), ("jxl", 0.1), ("png", 5.0)]),
            true,
        );
        assert_eq!(
            out,
            vec![Some("jxl".into()), Some("jpg".into()), Some("png".into())]
        );
    }

    proptest! {
        #[test]
        fn invariants(
            cap in 1usize..8,
            scores in proptest::collection::vec(0.0f64..10.0, 1..12),
            prev_idx in proptest::collection::vec(proptest::option::of(0usize..12), 0..8),
            allow in proptest::bool::ANY,
        ) {
            let ranked: Vec<(String, f64)> = scores.iter().enumerate().map(|(i, s)| (format!("t{i}"), *s)).collect();
            let mut prev: Vec<Option<String>> = Vec::new();
            for p in prev_idx.iter().take(cap) {
                let t = p.map(|i| format!("t{i}"));
                prev.push(if prev.contains(&t) { None } else { t });
            }
            let out = assign_slots(cap, &prev, &[], &ranked, allow);
            // No duplicates; every placed target exists.
            let placed: Vec<&String> = out.iter().flatten().collect();
            let mut dedup = placed.clone(); dedup.sort(); dedup.dedup();
            prop_assert_eq!(dedup.len(), placed.len());
            prop_assert!(placed.iter().all(|t| ranked.iter().any(|(r, _)| r == *t)));
            // Slots fill up to min(capacity, candidates).
            prop_assert_eq!(placed.len(), cap.min(ranked.len()));
            // Stability: at most one previously occupied valid slot changes, and none without allow.
            let changed = prev.iter().enumerate()
                .filter(|(i, p)| p.as_ref().is_some_and(|t| ranked.iter().any(|(r, _)| r == t)) && out[*i] != **p)
                .count();
            prop_assert!(changed <= usize::from(allow));
        }
    }
}
