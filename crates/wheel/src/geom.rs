//! Geometry at 100 % scale. Callers divide pointer offsets by the wheel's scale first.

use std::f32::consts::{PI, TAU};

/// Outer radius of the wedge band.
pub const R: f32 = 156.0;
/// Hub radius: dropping here cancels.
pub const R0: f32 = 50.0;
/// Inner radius of the wedge band (the 6 px between hub and band is a dead gap).
pub const RB: f32 = 56.0;
/// Preset sub-ring radii.
pub const R1: f32 = 164.0;
pub const R2: f32 = 220.0;
/// Wedges per ring; slot 7 becomes "More…" when there are more targets.
pub const MAX_WEDGES: usize = 8;
/// Distance outside the rim after which the wheel starts fading.
pub const EXIT_GRACE: f32 = 48.0;
/// Extra band past the rim that still counts as "pushing through" into the sub-ring.
pub const PUSH_BAND: f32 = 30.0;
/// Angular hysteresis so the highlight doesn't flicker on boundaries (3°).
pub const HYSTERESIS: f32 = 3.0 * PI / 180.0;
/// Visual gap between wedges, in px at the band's mid radius.
pub const GAP_PX: f32 = 3.0;

/// Angle of an offset: 0 at 12 o'clock, increasing clockwise, in [0, τ).
/// Screen coordinates: +x right, +y down.
pub fn angle(dx: f32, dy: f32) -> f32 {
    let a = dx.atan2(-dy);
    if a < 0.0 { a + TAU } else { a }
}

fn norm(a: f32) -> f32 {
    a.rem_euclid(TAU)
}

/// Signed smallest difference a − b in (−π, π].
pub fn ang_diff(a: f32, b: f32) -> f32 {
    let d = norm(a - b);
    if d > PI { d - TAU } else { d }
}

/// Center angle of wedge `i` of `n`; slot 0 sits at 12 o'clock.
pub fn wedge_center(i: usize, n: usize) -> f32 {
    i as f32 * TAU / n.max(1) as f32
}

/// Start and end angle of wedge `i`, shrunk by the visual gap.
pub fn wedge_span(i: usize, n: usize) -> (f32, f32) {
    if n <= 1 {
        return (0.0, TAU);
    }
    let w = TAU / n as f32;
    let pad = GAP_PX / ((RB + R) / 2.0) / 2.0;
    let c = wedge_center(i, n);
    (c - w / 2.0 + pad, c + w / 2.0 - pad)
}

/// Wedge index under angle `theta`, keeping `current` while within its half-width plus
/// [`HYSTERESIS`]. O(1).
pub fn wedge_index(theta: f32, n: usize, current: Option<usize>) -> usize {
    if n <= 1 {
        return 0;
    }
    let w = TAU / n as f32;
    let i = ((norm(theta) + w / 2.0) / w).floor() as usize % n;
    match current {
        Some(c)
            if c < n
                && c != i
                && ang_diff(theta, wedge_center(c, n)).abs() < w / 2.0 + HYSTERESIS =>
        {
            c
        }
        _ => i,
    }
}

/// A preset sub-ring that opened outward from wedge `parent`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SubRing {
    pub parent: usize,
    pub parent_count: usize,
    pub items: usize,
}

impl SubRing {
    /// Arc width per item: at most 0.42 rad, shrinking so the ring never wraps around.
    pub fn item_span(&self) -> f32 {
        0.42_f32.min(TAU * 0.92 / self.items.max(1) as f32)
    }
    pub fn span(&self, j: usize) -> (f32, f32) {
        let sp = self.item_span();
        let start = wedge_center(self.parent, self.parent_count) - sp * self.items as f32 / 2.0;
        (
            start + j as f32 * sp + 0.012,
            start + (j + 1) as f32 * sp - 0.012,
        )
    }
    pub fn item_at(&self, theta: f32) -> Option<usize> {
        (0..self.items).find(|&j| {
            let (a0, a1) = self.span(j);
            norm(theta - a0) <= a1 - a0
        })
    }
}

/// Where the pointer is relative to the wheel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Zone {
    Hub,
    /// Between hub and band.
    Gap,
    Ring(usize),
    Sub(usize),
    /// Just past the rim (or between sub-ring items): "push through" territory.
    Near {
        wedge: usize,
    },
    Outside {
        dist: f32,
    },
}

/// Classify an offset (already divided by scale) from the hub center.
pub fn hit(dx: f32, dy: f32, n: usize, current: Option<usize>, sub: Option<&SubRing>) -> Zone {
    let d = (dx * dx + dy * dy).sqrt();
    let th = angle(dx, dy);
    if d < R0 {
        return Zone::Hub;
    }
    if d < RB {
        return Zone::Gap;
    }
    if n == 0 {
        return Zone::Outside { dist: d };
    }
    if d <= R + 6.0 {
        return Zone::Ring(wedge_index(th, n, current));
    }
    if let Some(s) = sub
        && (R1 - 8.0..=R2 + 16.0).contains(&d)
        && let Some(j) = s.item_at(th)
    {
        return Zone::Sub(j);
    }
    let near_limit = if sub.is_some() {
        R2 + 24.0
    } else {
        R + PUSH_BAND
    };
    if d <= near_limit {
        return Zone::Near {
            wedge: wedge_index(th, n, current),
        };
    }
    Zone::Outside { dist: d - R }
}

/// Keep the wheel fully on screen: clamp the hub center into `work` (x, y, w, h) with a
/// 12 px margin and room for the tab pill above. Hit-testing then measures direction from
/// the hub, not from the pointer (docs/02 §3.7).
pub fn clamp_center(px: f32, py: f32, work: (f32, f32, f32, f32), scale: f32) -> (f32, f32) {
    let (x, y, w, h) = work;
    let d = R * scale;
    let m = 12.0;
    let top = d + 46.0 * scale + m;
    let cx = if w < 2.0 * (d + m) {
        x + w / 2.0
    } else {
        px.clamp(x + d + m, x + w - d - m)
    };
    let cy = if h < top + d + m {
        y + (top + h - d) / 2.0
    } else {
        py.clamp(y + top, y + h - d - m)
    };
    (cx, cy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn compass() {
        assert_eq!(hit(0.0, -100.0, 8, None, None), Zone::Ring(0)); // 12 o'clock
        assert_eq!(hit(100.0, 0.0, 8, None, None), Zone::Ring(2)); // 3 o'clock
        assert_eq!(hit(0.0, 100.0, 8, None, None), Zone::Ring(4));
        assert_eq!(hit(-100.0, 0.0, 8, None, None), Zone::Ring(6));
        assert_eq!(hit(5.0, 5.0, 8, None, None), Zone::Hub);
        assert_eq!(hit(0.0, -53.0, 8, None, None), Zone::Gap);
        assert!(matches!(
            hit(0.0, -175.0, 8, None, None),
            Zone::Near { wedge: 0 }
        ));
        assert!(matches!(
            hit(0.0, -400.0, 8, None, None),
            Zone::Outside { .. }
        ));
    }

    #[test]
    fn hysteresis_keeps_current_near_boundary() {
        let boundary = TAU / 16.0; // between slot 0 and 1 for n = 8
        let just_past = boundary + 0.02;
        assert_eq!(wedge_index(just_past, 8, None), 1);
        assert_eq!(wedge_index(just_past, 8, Some(0)), 0);
        assert_eq!(wedge_index(boundary + 0.2, 8, Some(0)), 1);
    }

    #[test]
    fn sub_ring_items() {
        let s = SubRing {
            parent: 0,
            parent_count: 8,
            items: 4,
        };
        let (a0, a1) = s.span(0);
        assert!(a0 < 0.0 && a1 < 0.0); // left of 12 o'clock
        let mid = (a0 + a1) / 2.0;
        let (dx, dy) = (mid.sin() * 190.0, -mid.cos() * 190.0);
        assert_eq!(hit(dx, dy, 8, Some(0), Some(&s)), Zone::Sub(0));
    }

    #[test]
    fn clamps_inside_work_area() {
        let (cx, cy) = clamp_center(0.0, 0.0, (0.0, 0.0, 1920.0, 1080.0), 1.0);
        assert!(cx - R >= 12.0 && cy - R - 46.0 >= 12.0);
        let (cx, _) = clamp_center(5000.0, 500.0, (0.0, 0.0, 1920.0, 1080.0), 1.0);
        assert!(cx + R <= 1920.0 - 12.0 + 0.01);
    }

    proptest! {
        #[test]
        fn index_matches_nearest_center(theta in 0.0f32..TAU, n in 2usize..=8) {
            let i = wedge_index(theta, n, None);
            let best = (0..n).min_by(|&a, &b| ang_diff(theta, wedge_center(a, n)).abs()
                .total_cmp(&ang_diff(theta, wedge_center(b, n)).abs())).unwrap();
            let da = ang_diff(theta, wedge_center(i, n)).abs();
            let db = ang_diff(theta, wedge_center(best, n)).abs();
            prop_assert!((da - db).abs() < 1e-4);
        }

        #[test]
        fn ring_zone_is_always_a_valid_wedge(dx in -400f32..400.0, dy in -400f32..400.0, n in 1usize..=8, cur in proptest::option::of(0usize..8)) {
            match hit(dx, dy, n, cur, None) {
                Zone::Ring(i) | Zone::Near { wedge: i } => prop_assert!(i < n),
                _ => {}
            }
        }
    }
}
