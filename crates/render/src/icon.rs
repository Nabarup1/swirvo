//! The Swirvo mark (docs/02 §14.1): a rounded square in the accent gradient with three
//! white blades curling clockwise out of a centre dot: a swirl, and the radial wheel
//! turning a file into something else.
//!
//! One geometry, in a 256 × 256 design space, feeds both the in-app icon (tray, title
//! bars; drawn with tiny-skia) and the exported SVG/PNG/ICO/MSIX assets
//! (`examples/icon_assets.rs`), so they can never drift apart. Sizes up to 24 px use a
//! simpler variant: a fuller square, wider blades, no centre dot.

use std::f32::consts::PI;

/// Path commands in design units (256 × 256, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    M(f32, f32),
    L(f32, f32),
    C(f32, f32, f32, f32, f32, f32),
    Z,
}

/// The gradient's ends: warm orange at the top left, coral red at the bottom right.
pub const TOP: (u8, u8, u8) = (0xF7, 0x9A, 0x45);
pub const BOTTOM: (u8, u8, u8) = (0xD9, 0x48, 0x2B);

/// Proportions for one variant (design units, degrees).
struct Spec {
    /// Background square: inset from the edge and corner radius.
    inset: f32,
    radius: f32,
    /// Blades start at `r_in` from the centre and end on the circle `r_out`.
    r_in: f32,
    r_out: f32,
    /// How far each blade turns from its root to its tip, and how wide it is at the tip.
    sweep: f32,
    width: f32,
    /// How fast the blade widens (1: evenly; below 1: full width sooner, bolder).
    taper: f32,
    /// Centre dot radius (0: none).
    dot: f32,
}

const LARGE: Spec = Spec {
    inset: 14.0,
    radius: 56.0,
    r_in: 26.0,
    r_out: 92.0,
    sweep: 128.0,
    width: 66.0,
    taper: 0.85,
    dot: 15.0,
};

const SMALL: Spec = Spec {
    inset: 6.0,
    radius: 44.0,
    r_in: 14.0,
    r_out: 100.0,
    sweep: 112.0,
    width: 84.0,
    taper: 0.6,
    dot: 0.0,
};

/// Is `px` small enough for the simplified mark?
pub fn small(px: u32) -> bool {
    px <= 24
}

/// The background: a rounded square.
pub fn background(small: bool) -> Vec<Cmd> {
    let s = if small { &SMALL } else { &LARGE };
    rounded_rect(s.inset, s.inset, 256.0 - s.inset, 256.0 - s.inset, s.radius)
}

/// The white mark: three blades curling clockwise out of the centre (a swirl, and the
/// wheel turning), plus a centre dot on the large variant.
pub fn mark(small: bool) -> Vec<Vec<Cmd>> {
    let s = if small { &SMALL } else { &LARGE };
    let (cx, cy) = (128.0, 128.0);
    let at = |a: f32, r: f32| (cx + r * a.to_radians().cos(), cy + r * a.to_radians().sin());
    // The blade's radius and width along its length (t from root 0 to tip 1).
    let radius = |t: f32| s.r_in + (s.r_out - s.r_in) * t.powf(0.85);
    let width = |t: f32| s.width * t.powf(s.taper);
    const N: usize = 36;
    let mut shapes = Vec::new();
    for k in 0..3 {
        let base = -90.0 + 120.0 * k as f32;
        let mut p = Vec::new();
        // Leading (clockwise) edge from root to tip…
        for i in 0..=N {
            let t = i as f32 / N as f32;
            let (x, y) = at(base + s.sweep * t, radius(t));
            p.push(if i == 0 { Cmd::M(x, y) } else { Cmd::L(x, y) });
        }
        // …along the outer circle across the tip…
        let a1 = base + s.sweep;
        let a0 = a1 - width(1.0);
        arc(&mut p, cx, cy, s.r_out, a1.to_radians(), a0.to_radians());
        // …and the trailing edge back to the root.
        for i in (0..N).rev() {
            let t = i as f32 / N as f32;
            let (x, y) = at(base + s.sweep * t - width(t), radius(t));
            p.push(Cmd::L(x, y));
        }
        p.push(Cmd::Z);
        shapes.push(p);
    }
    if s.dot > 0.0 {
        shapes.push(circle(cx, cy, s.dot));
    }
    shapes
}

/// The icon as a standalone SVG document (256 × 256 view box).
pub fn svg(small: bool) -> String {
    let d = |cmds: &[Cmd]| {
        let mut out = String::new();
        for c in cmds {
            let part = match *c {
                Cmd::M(x, y) => format!("M{x:.2} {y:.2}"),
                Cmd::L(x, y) => format!("L{x:.2} {y:.2}"),
                Cmd::C(a, b, c, d, e, f) => {
                    format!("C{a:.2} {b:.2} {c:.2} {d:.2} {e:.2} {f:.2}")
                }
                Cmd::Z => "Z".into(),
            };
            out.push_str(&part);
        }
        out
    };
    let hex = |(r, g, b): (u8, u8, u8)| format!("#{r:02X}{g:02X}{b:02X}");
    let mut s = String::new();
    s.push_str(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"256\" height=\"256\" viewBox=\"0 0 256 256\">\n",
    );
    s.push_str(&format!(
        "  <defs><linearGradient id=\"g\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"{}\"/><stop offset=\"1\" stop-color=\"{}\"/></linearGradient></defs>\n",
        hex(TOP),
        hex(BOTTOM)
    ));
    s.push_str(&format!(
        "  <path d=\"{}\" fill=\"url(#g)\"/>\n",
        d(&background(small))
    ));
    for shape in mark(small) {
        s.push_str(&format!("  <path d=\"{}\" fill=\"#FFFFFF\"/>\n", d(&shape)));
    }
    s.push_str("</svg>\n");
    s
}

/// Cubic Bézier arc from angle `a0` to `a1` (radians, either direction), the current
/// point being on the circle at `a0`.
fn arc(p: &mut Vec<Cmd>, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let n = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = (a1 - a0) / n as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..n {
        let (s, e) = (a0 + step * i as f32, a0 + step * (i + 1) as f32);
        let (sx, sy) = (cx + r * s.cos(), cy + r * s.sin());
        let (ex, ey) = (cx + r * e.cos(), cy + r * e.sin());
        p.push(Cmd::C(
            sx - k * r * s.sin(),
            sy + k * r * s.cos(),
            ex + k * r * e.sin(),
            ey - k * r * e.cos(),
            ex,
            ey,
        ));
    }
}

fn circle(cx: f32, cy: f32, r: f32) -> Vec<Cmd> {
    let mut p = vec![Cmd::M(cx + r, cy)];
    arc(&mut p, cx, cy, r, 0.0, 2.0 * PI);
    p.push(Cmd::Z);
    p
}

fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Vec<Cmd> {
    let mut p = vec![Cmd::M(x0 + r, y0), Cmd::L(x1 - r, y0)];
    arc(&mut p, x1 - r, y0 + r, r, -PI / 2.0, 0.0);
    p.push(Cmd::L(x1, y1 - r));
    arc(&mut p, x1 - r, y1 - r, r, 0.0, PI / 2.0);
    p.push(Cmd::L(x0 + r, y1));
    arc(&mut p, x0 + r, y1 - r, r, PI / 2.0, PI);
    p.push(Cmd::L(x0, y0 + r));
    arc(&mut p, x0 + r, y0 + r, r, PI, 1.5 * PI);
    p.push(Cmd::Z);
    p
}
