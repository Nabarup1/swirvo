//! Shape outlines: DrawingML preset geometries (with their adjust handles) and custom
//! geometry paths, as path commands in a `w` × `h` box (y down).

use std::collections::HashMap;
use std::f32::consts::PI;

use crate::docxlayout::layout::PathCmd as C;
use crate::docxlayout::xml::El;

/// Adjust values from `a:avLst` (`adj`, `adj1`…), as written (1/100000 units mostly).
pub fn adjusts(geom: &El) -> HashMap<String, f32> {
    geom.child("avLst")
        .map(|l| {
            l.all("gd")
                .filter_map(|g| {
                    let f = g.attr("fmla")?;
                    let v = f.strip_prefix("val ")?.trim().parse::<f32>().ok()?;
                    Some((g.attr("name")?.to_string(), v))
                })
                .collect()
        })
        .unwrap_or_default()
}

const K: f32 = 0.552_284_8;

fn ellipse(p: &mut Vec<C>, cx: f32, cy: f32, rx: f32, ry: f32) {
    p.push(C::Move(cx + rx, cy));
    p.push(C::Cubic(
        cx + rx,
        cy + ry * K,
        cx + rx * K,
        cy + ry,
        cx,
        cy + ry,
    ));
    p.push(C::Cubic(
        cx - rx * K,
        cy + ry,
        cx - rx,
        cy + ry * K,
        cx - rx,
        cy,
    ));
    p.push(C::Cubic(
        cx - rx,
        cy - ry * K,
        cx - rx * K,
        cy - ry,
        cx,
        cy - ry,
    ));
    p.push(C::Cubic(
        cx + rx * K,
        cy - ry,
        cx + rx,
        cy - ry * K,
        cx + rx,
        cy,
    ));
    p.push(C::Close);
}

fn poly(p: &mut Vec<C>, pts: &[(f32, f32)]) {
    for (i, &(x, y)) in pts.iter().enumerate() {
        p.push(if i == 0 { C::Move(x, y) } else { C::Line(x, y) });
    }
    p.push(C::Close);
}

fn round_rect(p: &mut Vec<C>, w: f32, h: f32, r: [f32; 4]) {
    // Radii: top-left, top-right, bottom-right, bottom-left.
    let [a, b, c, d] = r.map(|v| v.clamp(0.0, w.min(h) / 2.0));
    p.push(C::Move(a, 0.0));
    p.push(C::Line(w - b, 0.0));
    if b > 0.0 {
        p.push(C::Cubic(w - b + b * K, 0.0, w, b - b * K, w, b));
    }
    p.push(C::Line(w, h - c));
    if c > 0.0 {
        p.push(C::Cubic(w, h - c + c * K, w - c + c * K, h, w - c, h));
    }
    p.push(C::Line(d, h));
    if d > 0.0 {
        p.push(C::Cubic(d - d * K, h, 0.0, h - d + d * K, 0.0, h - d));
    }
    p.push(C::Line(0.0, a));
    if a > 0.0 {
        p.push(C::Cubic(0.0, a - a * K, a - a * K, 0.0, a, 0.0));
    }
    p.push(C::Close);
}

/// An elliptical arc of `rx`, `ry` from angle `a0` sweeping `sweep` (radians, clockwise
/// in y-down space) around (`cx`, `cy`), as cubic pieces. Returns the end point.
fn arc(p: &mut Vec<C>, cx: f32, cy: f32, rx: f32, ry: f32, a0: f32, sweep: f32) -> (f32, f32) {
    let n = (sweep.abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = sweep / n as f32;
    let mut a = a0;
    for _ in 0..n {
        let b = a + step;
        let k = 4.0 / 3.0 * ((b - a) / 4.0).tan();
        let (c0, s0, c1, s1) = (a.cos(), a.sin(), b.cos(), b.sin());
        p.push(C::Cubic(
            cx + rx * (c0 - k * s0),
            cy + ry * (s0 + k * c0),
            cx + rx * (c1 + k * s1),
            cy + ry * (s1 - k * c1),
            cx + rx * c1,
            cy + ry * s1,
        ));
        a = b;
    }
    (cx + rx * a.cos(), cy + ry * a.sin())
}

fn star(p: &mut Vec<C>, w: f32, h: f32, points: usize, inner: f32) {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let mut pts = Vec::with_capacity(points * 2);
    for i in 0..points * 2 {
        let a = -PI / 2.0 + i as f32 * PI / points as f32;
        let r = if i % 2 == 0 { 1.0 } else { inner };
        pts.push((cx + cx * r * a.cos(), cy + cy * r * a.sin()));
    }
    poly(p, &pts);
}

fn regular(p: &mut Vec<C>, w: f32, h: f32, sides: usize) {
    let (cx, cy) = (w / 2.0, h / 2.0);
    let pts: Vec<(f32, f32)> = (0..sides)
        .map(|i| {
            let a = -PI / 2.0 + i as f32 * 2.0 * PI / sides as f32;
            (cx + cx * a.cos(), cy + cy * a.sin())
        })
        .collect();
    poly(p, &pts);
}

/// A preset geometry's outline. `open` is true for lines (stroke only, never filled).
pub fn preset(name: &str, w: f32, h: f32, av: &HashMap<String, f32>) -> (Vec<C>, bool) {
    let a = |k: &str, d: f32| av.get(k).copied().unwrap_or(d) / 100_000.0;
    let ss = w.min(h);
    let mut p = Vec::new();
    let mut open = false;
    match name {
        "line" | "straightConnector1" => {
            p.push(C::Move(0.0, 0.0));
            p.push(C::Line(w, h));
            open = true;
        }
        "bentConnector2" => {
            poly_open(&mut p, &[(0.0, 0.0), (w, 0.0), (w, h)]);
            open = true;
        }
        "bentConnector3" => {
            let x = w * a("adj1", 50_000.0);
            poly_open(&mut p, &[(0.0, 0.0), (x, 0.0), (x, h), (w, h)]);
            open = true;
        }
        "bentConnector4" | "bentConnector5" => {
            let x = w * a("adj1", 50_000.0);
            poly_open(&mut p, &[(0.0, 0.0), (x, 0.0), (x, h), (w, h)]);
            open = true;
        }
        "curvedConnector2" | "curvedConnector3" | "curvedConnector4" | "curvedConnector5" => {
            let x = w * a("adj1", 50_000.0);
            p.push(C::Move(0.0, 0.0));
            p.push(C::Cubic(x, 0.0, x, h, w, h));
            open = true;
        }
        "arc" => {
            let a1 = av.get("adj1").copied().unwrap_or(16_200_000.0) / 60_000.0;
            let a2 = av.get("adj2").copied().unwrap_or(0.0) / 60_000.0;
            let mut sweep = a2 - a1;
            if sweep <= 0.0 {
                sweep += 360.0;
            }
            let (rx, ry) = (w / 2.0, h / 2.0);
            let r0 = a1.to_radians();
            p.push(C::Move(rx + rx * r0.cos(), ry + ry * r0.sin()));
            arc(&mut p, rx, ry, rx, ry, r0, sweep.to_radians());
            open = true;
        }
        "ellipse" | "flowChartConnector" | "cloud" | "flowChartOnlineStorage" => {
            ellipse(&mut p, w / 2.0, h / 2.0, w / 2.0, h / 2.0)
        }
        "roundRect" | "flowChartAlternateProcess" => {
            let r = ss * a("adj", 16_667.0);
            round_rect(&mut p, w, h, [r; 4]);
        }
        "round1Rect" => {
            let r = ss * a("adj", 16_667.0);
            round_rect(&mut p, w, h, [0.0, r, 0.0, 0.0]);
        }
        "round2SameRect" => {
            let r1 = ss * a("adj1", 16_667.0);
            let r2 = ss * a("adj2", 0.0);
            round_rect(&mut p, w, h, [r1, r1, r2, r2]);
        }
        "round2DiagRect" => {
            let r1 = ss * a("adj1", 16_667.0);
            let r2 = ss * a("adj2", 0.0);
            round_rect(&mut p, w, h, [r1, r2, r1, r2]);
        }
        "flowChartTerminator" => round_rect(&mut p, w, h, [h.min(w) / 2.0; 4]),
        "snip1Rect" => {
            let s = ss * a("adj", 16_667.0);
            poly(
                &mut p,
                &[(0.0, 0.0), (w - s, 0.0), (w, s), (w, h), (0.0, h)],
            );
        }
        "snip2SameRect" => {
            let s = ss * a("adj1", 16_667.0);
            poly(
                &mut p,
                &[(s, 0.0), (w - s, 0.0), (w, s), (w, h), (0.0, h), (0.0, s)],
            );
        }
        "flowChartMerge" => poly(&mut p, &[(0.0, 0.0), (w, 0.0), (w / 2.0, h)]),
        "triangle" | "flowChartExtract" => {
            let x = w * a("adj", 50_000.0);
            poly(&mut p, &[(x, 0.0), (w, h), (0.0, h)]);
        }
        "rtTriangle" => poly(&mut p, &[(0.0, 0.0), (w, h), (0.0, h)]),
        "diamond" | "flowChartDecision" => poly(
            &mut p,
            &[(w / 2.0, 0.0), (w, h / 2.0), (w / 2.0, h), (0.0, h / 2.0)],
        ),
        "parallelogram" | "flowChartInputOutput" => {
            let x = ss * a("adj", 25_000.0);
            poly(&mut p, &[(x, 0.0), (w, 0.0), (w - x, h), (0.0, h)]);
        }
        "trapezoid" | "flowChartManualOperation" => {
            let x = ss * a("adj", 25_000.0);
            poly(&mut p, &[(0.0, 0.0), (w, 0.0), (w - x, h), (x, h)]);
        }
        "pentagon" => regular(&mut p, w, h, 5),
        "hexagon" | "flowChartPreparation" => {
            let x = ss * a("adj", 25_000.0);
            poly(
                &mut p,
                &[
                    (x, 0.0),
                    (w - x, 0.0),
                    (w, h / 2.0),
                    (w - x, h),
                    (x, h),
                    (0.0, h / 2.0),
                ],
            );
        }
        "heptagon" => regular(&mut p, w, h, 7),
        "octagon" => {
            let x = ss * a("adj", 29_289.0);
            poly(
                &mut p,
                &[
                    (x, 0.0),
                    (w - x, 0.0),
                    (w, x),
                    (w, h - x),
                    (w - x, h),
                    (x, h),
                    (0.0, h - x),
                    (0.0, x),
                ],
            );
        }
        "decagon" => regular(&mut p, w, h, 10),
        "dodecagon" => regular(&mut p, w, h, 12),
        "homePlate" => {
            let x = w - ss * a("adj", 50_000.0);
            poly(
                &mut p,
                &[(0.0, 0.0), (x, 0.0), (w, h / 2.0), (x, h), (0.0, h)],
            );
        }
        "chevron" => {
            let x = ss * a("adj", 50_000.0);
            poly(
                &mut p,
                &[
                    (0.0, 0.0),
                    (w - x, 0.0),
                    (w, h / 2.0),
                    (w - x, h),
                    (0.0, h),
                    (x, h / 2.0),
                ],
            );
        }
        "rightArrow" | "leftArrow" => {
            let shaft = h * a("adj1", 50_000.0);
            let head = ss * a("adj2", 50_000.0);
            let (t, b) = ((h - shaft) / 2.0, (h + shaft) / 2.0);
            let mut pts = vec![
                (0.0, t),
                (w - head, t),
                (w - head, 0.0),
                (w, h / 2.0),
                (w - head, h),
                (w - head, b),
                (0.0, b),
            ];
            if name == "leftArrow" {
                pts.iter_mut().for_each(|q| q.0 = w - q.0);
            }
            poly(&mut p, &pts);
        }
        "upArrow" | "downArrow" => {
            let shaft = w * a("adj1", 50_000.0);
            let head = ss * a("adj2", 50_000.0);
            let (l, r) = ((w - shaft) / 2.0, (w + shaft) / 2.0);
            let mut pts = vec![
                (l, h),
                (l, head),
                (0.0, head),
                (w / 2.0, 0.0),
                (w, head),
                (r, head),
                (r, h),
            ];
            if name == "downArrow" {
                pts.iter_mut().for_each(|q| q.1 = h - q.1);
            }
            poly(&mut p, &pts);
        }
        "leftRightArrow" => {
            let shaft = h * a("adj1", 50_000.0);
            let head = ss * a("adj2", 50_000.0);
            let (t, b) = ((h - shaft) / 2.0, (h + shaft) / 2.0);
            poly(
                &mut p,
                &[
                    (0.0, h / 2.0),
                    (head, 0.0),
                    (head, t),
                    (w - head, t),
                    (w - head, 0.0),
                    (w, h / 2.0),
                    (w - head, h),
                    (w - head, b),
                    (head, b),
                    (head, h),
                ],
            );
        }
        "star4" => star(&mut p, w, h, 4, a("adj", 12_500.0) * 2.0),
        "star5" => star(&mut p, w, h, 5, 0.382),
        "star6" => star(&mut p, w, h, 6, 0.577),
        "star7" | "star8" => star(&mut p, w, h, 8, 0.6),
        "star10" | "star12" | "star16" | "star24" | "star32" => star(&mut p, w, h, 12, 0.75),
        "plus" | "mathPlus" => {
            let x = ss * a("adj", 25_000.0);
            poly(
                &mut p,
                &[
                    (x, 0.0),
                    (w - x, 0.0),
                    (w - x, x),
                    (w, x),
                    (w, h - x),
                    (w - x, h - x),
                    (w - x, h),
                    (x, h),
                    (x, h - x),
                    (0.0, h - x),
                    (0.0, x),
                    (x, x),
                ],
            );
        }
        "donut" => {
            ellipse(&mut p, w / 2.0, h / 2.0, w / 2.0, h / 2.0);
            let t = ss * a("adj", 25_000.0);
            ellipse(
                &mut p,
                w / 2.0,
                h / 2.0,
                (w / 2.0 - t).max(0.0),
                (h / 2.0 - t).max(0.0),
            );
        }
        "frame" => {
            let t = ss * a("adj1", 12_500.0);
            poly(&mut p, &[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]);
            poly(&mut p, &[(t, t), (t, h - t), (w - t, h - t), (w - t, t)]);
        }
        "can" | "flowChartMagneticDisk" => {
            let ry = ss * a("adj", 25_000.0) / 2.0;
            let rx = w / 2.0;
            p.push(C::Move(0.0, ry));
            arc(&mut p, rx, ry, rx, ry, PI, -PI);
            p.push(C::Line(w, h - ry));
            arc(&mut p, rx, h - ry, rx, ry, 0.0, PI);
            p.push(C::Close);
            ellipse(&mut p, rx, ry, rx, ry);
        }
        "pie" | "chord" | "blockArc" => {
            let a1 = av.get("adj1").copied().unwrap_or(0.0) / 60_000.0;
            let a2 = av.get("adj2").copied().unwrap_or(16_200_000.0) / 60_000.0;
            let mut sweep = a2 - a1;
            if sweep <= 0.0 {
                sweep += 360.0;
            }
            let (rx, ry) = (w / 2.0, h / 2.0);
            let r0 = a1.to_radians();
            if name == "pie" {
                p.push(C::Move(rx, ry));
                p.push(C::Line(rx + rx * r0.cos(), ry + ry * r0.sin()));
            } else {
                p.push(C::Move(rx + rx * r0.cos(), ry + ry * r0.sin()));
            }
            arc(&mut p, rx, ry, rx, ry, r0, sweep.to_radians());
            p.push(C::Close);
        }
        "teardrop" => {
            let (rx, ry) = (w / 2.0, h / 2.0);
            p.push(C::Move(0.0, ry));
            arc(&mut p, rx, ry, rx, ry, PI, PI / 2.0 * 3.0);
            p.push(C::Line(w, 0.0));
            p.push(C::Line(w, ry));
            p.push(C::Close);
        }
        "heart" => {
            p.push(C::Move(w / 2.0, h * 0.25));
            p.push(C::Cubic(w * 0.5, 0.0, 0.0, 0.0, 0.0, h * 0.3));
            p.push(C::Cubic(0.0, h * 0.6, w * 0.4, h * 0.8, w / 2.0, h));
            p.push(C::Cubic(w * 0.6, h * 0.8, w, h * 0.6, w, h * 0.3));
            p.push(C::Cubic(w, 0.0, w * 0.5, 0.0, w / 2.0, h * 0.25));
            p.push(C::Close);
        }
        "foldedCorner" => {
            let f = ss * a("adj", 16_667.0);
            poly(
                &mut p,
                &[(0.0, 0.0), (w, 0.0), (w, h - f), (w - f, h), (0.0, h)],
            );
        }
        "plaque" => {
            let r = ss * a("adj", 16_667.0);
            p.push(C::Move(r, 0.0));
            p.push(C::Line(w - r, 0.0));
            arc(&mut p, w, 0.0, r, r, PI, -PI / 2.0);
            p.push(C::Line(w, h - r));
            arc(&mut p, w, h, r, r, -PI / 2.0, -PI / 2.0);
            p.push(C::Line(r, h));
            arc(&mut p, 0.0, h, r, r, 0.0, -PI / 2.0);
            p.push(C::Line(0.0, r));
            arc(&mut p, 0.0, 0.0, r, r, PI / 2.0, -PI / 2.0);
            p.push(C::Close);
        }
        "leftBracket" | "rightBracket" => {
            let r = (h * a("adj", 8_333.0)).min(w);
            let x = if name == "leftBracket" { w } else { 0.0 };
            let x2 = if name == "leftBracket" { 0.0 } else { w };
            p.push(C::Move(x, 0.0));
            p.push(C::Quad(x2, 0.0, x2, r));
            p.push(C::Line(x2, h - r));
            p.push(C::Quad(x2, h, x, h));
            open = true;
        }
        "leftBrace" | "rightBrace" => {
            let (x, m) = if name == "leftBrace" {
                (w, 0.0)
            } else {
                (0.0, w)
            };
            let mid = (x + m) / 2.0;
            p.push(C::Move(x, 0.0));
            p.push(C::Quad(mid, 0.0, mid, h * 0.15));
            p.push(C::Line(mid, h * 0.4));
            p.push(C::Quad(mid, h / 2.0, m, h / 2.0));
            p.push(C::Quad(mid, h / 2.0, mid, h * 0.6));
            p.push(C::Line(mid, h * 0.85));
            p.push(C::Quad(mid, h, x, h));
            open = true;
        }
        "cube" => {
            let d = ss * a("adj", 25_000.0);
            poly(&mut p, &[(0.0, d), (w - d, d), (w - d, h), (0.0, h)]);
            poly(&mut p, &[(0.0, d), (d, 0.0), (w, 0.0), (w - d, d)]);
            poly(&mut p, &[(w - d, d), (w, 0.0), (w, h - d), (w - d, h)]);
        }
        "wedgeRectCallout" | "wedgeRoundRectCallout" | "wedgeEllipseCallout" | "cloudCallout" => {
            let tx = w / 2.0 + w * a("adj1", -20_833.0);
            let ty = h / 2.0 + h * a("adj2", 62_500.0);
            if name == "wedgeEllipseCallout" || name == "cloudCallout" {
                ellipse(&mut p, w / 2.0, h / 2.0, w / 2.0, h / 2.0);
            } else if name == "wedgeRoundRectCallout" {
                round_rect(&mut p, w, h, [ss * 0.1667; 4]);
            } else {
                poly(&mut p, &[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]);
            }
            let (bx, by) = (w / 2.0, h / 2.0);
            poly(
                &mut p,
                &[(bx - w * 0.08, by), (bx + w * 0.08, by), (tx, ty)],
            );
        }
        "flowChartDocument" => {
            p.push(C::Move(0.0, 0.0));
            p.push(C::Line(w, 0.0));
            p.push(C::Line(w, h * 0.83));
            p.push(C::Cubic(w * 0.7, h * 0.7, w * 0.3, h * 1.1, 0.0, h * 0.9));
            p.push(C::Close);
        }
        "corner" => {
            let x = ss * a("adj1", 50_000.0);
            let y = ss * a("adj2", 50_000.0);
            poly(
                &mut p,
                &[
                    (0.0, 0.0),
                    (x, 0.0),
                    (x, h - y),
                    (w, h - y),
                    (w, h),
                    (0.0, h),
                ],
            );
        }
        "halfFrame" => {
            let t = ss * a("adj1", 33_333.0);
            poly(
                &mut p,
                &[
                    (0.0, 0.0),
                    (w, 0.0),
                    (w - t, t),
                    (t, t),
                    (t, h - t),
                    (0.0, h),
                ],
            );
        }
        "diagStripe" => {
            let x = ss * a("adj", 50_000.0);
            poly(&mut p, &[(0.0, x), (x, 0.0), (w, 0.0), (0.0, h)]);
        }
        "mathMinus" => {
            let t = h * a("adj1", 23_520.0);
            poly(
                &mut p,
                &[
                    (w * 0.1, h / 2.0 - t / 2.0),
                    (w * 0.9, h / 2.0 - t / 2.0),
                    (w * 0.9, h / 2.0 + t / 2.0),
                    (w * 0.1, h / 2.0 + t / 2.0),
                ],
            );
        }
        // Rectangles and everything we don't know yet: the box itself.
        _ => poly(&mut p, &[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]),
    }
    (p, open)
}

fn poly_open(p: &mut Vec<C>, pts: &[(f32, f32)]) {
    for (i, &(x, y)) in pts.iter().enumerate() {
        p.push(if i == 0 { C::Move(x, y) } else { C::Line(x, y) });
    }
}

/// `a:custGeom`: each `a:path` in its own coordinate space, scaled to the box. Returns
/// (commands, open) — open when no path is filled.
pub fn custom(geom: &El, w: f32, h: f32) -> (Vec<C>, bool) {
    let mut out = Vec::new();
    let mut filled = false;
    let Some(list) = geom.child("pathLst") else {
        return (out, true);
    };
    for path in list.all("path") {
        let pw = path
            .attr("w")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0);
        let ph = path
            .attr("h")
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0);
        // Without w/h, coordinates are in EMU of the shape.
        let sx = pw.map_or(1.0 / super::style::EMU, |pw| w / pw);
        let sy = ph.map_or(1.0 / super::style::EMU, |ph| h / ph);
        if path.attr("fill") != Some("none") {
            filled = true;
        }
        let pt = |e: &El| -> Option<(f32, f32)> {
            let x = e.attr("x")?.parse::<f32>().ok()? * sx;
            let y = e.attr("y")?.parse::<f32>().ok()? * sy;
            Some((x, y))
        };
        let mut cur = (0.0f32, 0.0f32);
        for c in path.children() {
            let pts: Vec<(f32, f32)> = c.all("pt").filter_map(pt).collect();
            match &*c.name {
                "moveTo" => {
                    if let Some(&q) = pts.first() {
                        out.push(C::Move(q.0, q.1));
                        cur = q;
                    }
                }
                "lnTo" => {
                    if let Some(&q) = pts.first() {
                        out.push(C::Line(q.0, q.1));
                        cur = q;
                    }
                }
                "quadBezTo" if pts.len() >= 2 => {
                    out.push(C::Quad(pts[0].0, pts[0].1, pts[1].0, pts[1].1));
                    cur = pts[1];
                }
                "cubicBezTo" if pts.len() >= 3 => {
                    out.push(C::Cubic(
                        pts[0].0, pts[0].1, pts[1].0, pts[1].1, pts[2].0, pts[2].1,
                    ));
                    cur = pts[2];
                }
                "arcTo" => {
                    let num =
                        |k: &str| c.attr(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                    let (rx, ry) = (num("wR") * sx, num("hR") * sy);
                    let st = (num("stAng") / 60_000.0).to_radians();
                    let sw = (num("swAng") / 60_000.0).to_radians();
                    // The arc starts at the current point, at angle `st` on its ellipse.
                    let (cx, cy) = (cur.0 - rx * st.cos(), cur.1 - ry * st.sin());
                    cur = arc(&mut out, cx, cy, rx, ry, st, sw);
                }
                "close" => out.push(C::Close),
                _ => {}
            }
        }
    }
    (out, !filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(p: &[C]) -> (f32, f32, f32, f32) {
        let mut b = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let mut add = |x: f32, y: f32| {
            b.0 = b.0.min(x);
            b.1 = b.1.min(y);
            b.2 = b.2.max(x);
            b.3 = b.3.max(y);
        };
        for c in p {
            match *c {
                C::Move(x, y) | C::Line(x, y) => add(x, y),
                C::Quad(_, _, x, y) | C::Cubic(_, _, _, _, x, y) => add(x, y),
                C::Close => {}
            }
        }
        b
    }

    #[test]
    fn presets_fill_their_box() {
        let av = HashMap::new();
        for name in [
            "rect",
            "roundRect",
            "ellipse",
            "triangle",
            "diamond",
            "hexagon",
            "rightArrow",
            "chevron",
            "star5",
            "plus",
            "can",
            "flowChartDecision",
            "homePlate",
        ] {
            let (p, open) = preset(name, 200.0, 100.0, &av);
            assert!(!open && !p.is_empty(), "{name}");
            let b = bounds(&p);
            assert!(
                b.0 >= -0.5 && b.1 >= -0.5 && b.2 <= 200.5 && b.3 <= 100.5,
                "{name}: {b:?}"
            );
            assert!(
                b.2 - b.0 > 150.0 && b.3 - b.1 > 80.0,
                "{name} fills its box: {b:?}"
            );
        }
        assert!(preset("line", 10.0, 10.0, &av).1);
    }

    #[test]
    fn quarter_arc_ends_where_it_should() {
        let mut p = Vec::new();
        let end = arc(&mut p, 0.0, 0.0, 10.0, 10.0, 0.0, PI / 2.0);
        assert!(
            (end.0).abs() < 1e-4 && (end.1 - 10.0).abs() < 1e-4,
            "{end:?}"
        );
    }
}
