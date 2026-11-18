//! Drawing a [`Chart`] into a box as layout ops: title, legend, axes with "nice" scales,
//! gridlines, bars, lines, areas, pies and doughnuts, scatter points and data labels.
//! Proportions follow Office's defaults (gap width, label sizes, colours).

use std::f32::consts::PI;
use std::sync::Arc;

use super::read::{Chart, Grouping, Kind, LegendPos, Plot, Series};
use crate::docxlayout::layout::{Figure, LayText, Op, Paint, PathCmd, Pen};
use crate::docxlayout::model::{Block, Item, Para, ParaStyle, RunStyle};
use crate::docxlayout::props::{Align, LineRule, Rgb};
use crate::pptxlayout::style::Col;

const GRID: Rgb = Rgb(0xD9, 0xD9, 0xD9);

pub(crate) fn style(font: &str, size: f32, color: Rgb, bold: bool) -> Arc<RunStyle> {
    Arc::new(RunStyle {
        fonts: [font.into(), font.into(), font.into(), font.into()],
        b: bold,
        i: false,
        b_cs: bold,
        i_cs: false,
        sz: size.clamp(1.0, 400.0),
        sz_cs: size.clamp(1.0, 400.0),
        color,
        auto_color: false,
        u: Default::default(),
        u_color: None,
        strike: false,
        dstrike: false,
        caps: false,
        small_caps: false,
        vanish: false,
        vert: Default::default(),
        position: 0.0,
        spacing: 0.0,
        scale: 1.0,
        kern: None,
        highlight: None,
        shd: None,
        bdr: None,
        rtl: false,
        cs: false,
    })
}

fn para(text: &str, rs: Arc<RunStyle>, align: Align) -> Block {
    Block::Para(Box::new(Para {
        p: ParaStyle {
            align,
            left: 0.0,
            right: 0.0,
            first: 0.0,
            before: 0.0,
            after: 0.0,
            line: LineRule::Auto(1.0),
            contextual: false,
            keep_next: false,
            keep_lines: false,
            page_break_before: false,
            widow: false,
            tabs: Vec::new(),
            shd: None,
            borders: [None; 5],
            bidi: false,
            style: String::new(),
        },
        mark: rs.clone(),
        items: text
            .split('\n')
            .enumerate()
            .flat_map(|(i, line)| {
                let mut v = Vec::new();
                if i > 0 {
                    v.push(Item::Break(crate::docxlayout::model::BreakKind::Line));
                }
                if !line.is_empty() {
                    v.push(Item::Text {
                        text: line.to_string(),
                        rs: rs.clone(),
                        link: None,
                    });
                }
                v
            })
            .collect(),
        label: None,
        outline: None,
    }))
}

/// Laid-out text: its ops (top-left at 0,0), width and height.
struct Label {
    ops: Vec<Op>,
    w: f32,
    h: f32,
}

struct Ctx<'b> {
    boxes: &'b mut dyn LayText,
    font: String,
    ops: Vec<Op>,
}

impl Ctx<'_> {
    fn label(
        &mut self,
        text: &str,
        size: f32,
        color: Rgb,
        bold: bool,
        max_w: Option<f32>,
    ) -> Label {
        let rs = style(&self.font, size, color, bold);
        let block = [para(text, rs, Align::Left)];
        let (ops, h) = self.boxes.lay_text(&block, max_w.unwrap_or(100_000.0));
        let w = ops
            .iter()
            .filter_map(|o| match o {
                Op::Text { x, seg, .. } => Some(x + seg.width),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        Label { ops, w, h }
    }

    fn put(&mut self, l: Label, x: f32, y: f32) {
        self.ops.extend(l.ops.into_iter().map(|o| o.moved(x, y)));
    }

    fn figure(&mut self, path: Vec<PathCmd>, fill: Option<Col>, stroke: Option<(Col, f32)>) {
        self.ops.push(Op::Figure(Arc::new(Figure {
            x: 0.0,
            y: 0.0,
            path,
            fill: fill.map(|(c, a)| Paint::Solid(c, a)),
            stroke: stroke.map(|((c, a), w)| Pen {
                color: c,
                alpha: a,
                width: w,
                dash: Vec::new(),
                round: true,
            }),
            even_odd: false,
        })));
    }

    fn rect(
        &mut self,
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        fill: Option<Col>,
        stroke: Option<(Col, f32)>,
    ) {
        self.figure(
            vec![
                PathCmd::Move(x0, y0),
                PathCmd::Line(x1, y0),
                PathCmd::Line(x1, y1),
                PathCmd::Line(x0, y1),
                PathCmd::Close,
            ],
            fill,
            stroke,
        );
    }

    fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Rgb, w: f32) {
        self.figure(
            vec![PathCmd::Move(x0, y0), PathCmd::Line(x1, y1)],
            None,
            Some(((c, 1.0), w)),
        );
    }
}

/// Automatic colour of series or point `i`: the accents in turn, then darker and
/// lighter rounds of them, as Office does.
pub fn auto_color(accents: &[Rgb; 6], i: usize) -> Rgb {
    let base = accents[i % 6];
    let round = i / 6;
    if round == 0 {
        return base;
    }
    let (h, s, l) = crate::docxlayout::theme::hsl(base);
    let l = match round % 3 {
        1 => l * 0.6,
        2 => l * 0.8 + 0.2,
        _ => l * 0.5,
    };
    crate::docxlayout::theme::from_hsl(h, s, l.clamp(0.0, 1.0))
}

fn series_fill(c: &Chart, s: &Series, i: usize) -> Col {
    s.fill.unwrap_or((auto_color(&c.accents, i), 1.0))
}

/// A tidy axis scale: (min, max, step) with steps of 1, 2 or 5 × 10ⁿ.
pub fn nice_scale(
    lo: f64,
    hi: f64,
    fixed: (Option<f64>, Option<f64>, Option<f64>),
) -> (f64, f64, f64) {
    let (mut lo, mut hi) = (lo.min(hi), lo.max(hi));
    if lo > 0.0 && fixed.0.is_none() {
        lo = 0.0;
    }
    if hi < 0.0 && fixed.1.is_none() {
        hi = 0.0;
    }
    if (hi - lo).abs() < f64::EPSILON {
        hi = lo + 1.0;
    }
    let lo = fixed.0.unwrap_or(lo);
    let hi_target = fixed.1.unwrap_or(hi + (hi - lo) * 0.05);
    // The smallest 1, 2 or 5 × 10ⁿ step that needs at most ten intervals.
    let step = fixed.2.filter(|s| *s > 0.0).unwrap_or_else(|| {
        let range = (hi_target - lo).max(f64::MIN_POSITIVE);
        let mut mag = 10f64.powf((range / 10.0).log10().floor());
        loop {
            for m in [1.0, 2.0, 5.0] {
                if range / (m * mag) <= 10.0 + 1e-9 {
                    return m * mag;
                }
            }
            mag *= 10.0;
        }
    });
    // Rounded to the step's decimals, so 6 × 0.2 is 1.2 and not 1.2000000000000002.
    let lo = if fixed.0.is_some() {
        lo
    } else {
        snap((lo / step).floor() * step, step)
    };
    let hi = if fixed.1.is_some() {
        hi_target
    } else {
        snap((hi_target / step).ceil() * step, step)
    };
    (lo, hi.max(lo + step), step)
}

/// `v` rounded to one decimal more than `step` has.
fn snap(v: f64, step: f64) -> f64 {
    let decimals = (-step.log10().floor()).clamp(0.0, 12.0) as i32 + 1;
    let k = 10f64.powi(decimals);
    (v * k).round() / k
}

fn fmt_value(v: f64, code: &str) -> String {
    crate::xlsxlayout::numfmt::format(v, code, false).0
}

/// Draw `c` into a `w` × `h` box at (0, 0); the host moves the ops into place.
pub(crate) fn draw(c: &Chart, w: f32, h: f32, boxes: &mut dyn LayText) -> Vec<Op> {
    let mut cx = Ctx {
        boxes,
        font: c.font.clone(),
        ops: Vec::new(),
    };
    if c.background.is_some() || c.border.is_some() {
        cx.rect(0.0, 0.0, w, h, c.background, c.border);
    }
    let pad = 7.0;
    let (mut x0, mut y0, mut x1, mut y1) = (pad, pad, w - pad, h - pad);
    if let Some(t) = &c.title {
        let l = cx.label(t, c.title_size, c.color, false, Some(w - 2.0 * pad));
        let lh = l.h;
        let lw = l.w;
        cx.put(l, (w - lw) / 2.0, y0);
        y0 += lh + 6.0;
    }
    // Legend entries: (colour, text).
    let pie = matches!(c.plots[0].kind, Kind::Pie | Kind::Doughnut);
    let entries: Vec<(Col, String)> = if c.plots[0].vary && c.plots[0].series.len() == 1 {
        let s = &c.plots[0].series[0];
        s.cats
            .iter()
            .enumerate()
            .map(|(i, cat)| (point_color(c, s, i), cat.clone()))
            .collect()
    } else {
        let mut k = 0;
        c.plots
            .iter()
            .flat_map(|p| p.series.iter())
            .map(|s| {
                let col = if s.no_fill || matches!(c.plots[0].kind, Kind::Line | Kind::Scatter) {
                    // As the line is drawn: its own colour, else the fill, else automatic.
                    s.line
                        .map(|l| l.0)
                        .or(s.fill)
                        .unwrap_or((auto_color(&c.accents, k), 1.0))
                } else {
                    series_fill(c, s, k)
                };
                k += 1;
                (col, s.name.clone())
            })
            .collect()
    };
    let ls = (c.size * 0.9).max(6.0);
    if let Some(pos) = c.legend
        && !entries.is_empty()
    {
        let sw = ls * 0.7;
        let labels: Vec<Label> = entries
            .iter()
            .map(|(_, t)| cx.label(t, ls, c.color, false, None))
            .collect();
        let lh = labels.iter().map(|l| l.h).fold(0.0, f32::max);
        match pos {
            LegendPos::Bottom | LegendPos::Top => {
                // Rows of entries, centred.
                let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
                let mut rw = 0.0;
                for (i, l) in labels.iter().enumerate() {
                    let ew = sw + 4.0 + l.w + 10.0;
                    if rw + ew > x1 - x0 && !rows.last().is_some_and(Vec::is_empty) {
                        rows.push(Vec::new());
                        rw = 0.0;
                    }
                    if let Some(r) = rows.last_mut() {
                        r.push(i);
                    }
                    rw += ew;
                }
                let total = rows.len() as f32 * lh;
                let mut y = if pos == LegendPos::Bottom {
                    y1 - total
                } else {
                    y0
                };
                let mut labels: Vec<Option<Label>> = labels.into_iter().map(Some).collect();
                for row in &rows {
                    let width: f32 = row
                        .iter()
                        .map(|&i| sw + 4.0 + labels[i].as_ref().map_or(0.0, |l| l.w) + 10.0)
                        .sum::<f32>()
                        - 10.0;
                    let mut x = (x0 + x1 - width) / 2.0;
                    for &i in row {
                        let Some(l) = labels[i].take() else { continue };
                        let lw = l.w;
                        cx.rect(
                            x,
                            y + (lh - sw) / 2.0,
                            x + sw,
                            y + (lh + sw) / 2.0,
                            Some(entries[i].0),
                            None,
                        );
                        cx.put(l, x + sw + 4.0, y);
                        x += sw + 4.0 + lw + 10.0;
                    }
                    y += lh;
                }
                if pos == LegendPos::Bottom {
                    y1 -= total + 6.0;
                } else {
                    y0 += total + 6.0;
                }
            }
            LegendPos::Right | LegendPos::Left => {
                let lw = labels.iter().map(|l| l.w).fold(0.0, f32::max) + sw + 4.0;
                let total = labels.len() as f32 * lh;
                let mut y = (y0 + y1 - total) / 2.0;
                let x = if pos == LegendPos::Right { x1 - lw } else { x0 };
                for (i, l) in labels.into_iter().enumerate() {
                    cx.rect(
                        x,
                        y + (lh - sw) / 2.0,
                        x + sw,
                        y + (lh + sw) / 2.0,
                        Some(entries[i].0),
                        None,
                    );
                    cx.put(l, x + sw + 4.0, y);
                    y += lh;
                }
                if pos == LegendPos::Right {
                    x1 -= lw + 10.0;
                } else {
                    x0 += lw + 10.0;
                }
            }
        }
    }
    if x1 - x0 < 10.0 || y1 - y0 < 10.0 {
        return cx.ops;
    }
    if pie {
        for p in &c.plots {
            pie_plot(&mut cx, c, p, (x0, y0, x1, y1));
        }
    } else {
        axes_plot(&mut cx, c, (x0, y0, x1, y1));
    }
    cx.ops
}

fn point_color(c: &Chart, s: &Series, i: usize) -> Col {
    s.points
        .iter()
        .find(|(k, _)| *k == i)
        .map(|p| p.1)
        .unwrap_or((auto_color(&c.accents, i), 1.0))
}

fn arc(path: &mut Vec<PathCmd>, cx: f32, cy: f32, r: f32, a0: f32, a1: f32) {
    let n = ((a1 - a0).abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = (a1 - a0) / n as f32;
    let mut a = a0;
    for _ in 0..n {
        let b = a + step;
        let k = 4.0 / 3.0 * ((b - a) / 4.0).tan();
        let (c0, s0, c1, s1) = (a.cos(), a.sin(), b.cos(), b.sin());
        path.push(PathCmd::Cubic(
            cx + r * (c0 - k * s0),
            cy + r * (s0 + k * c0),
            cx + r * (c1 + k * s1),
            cy + r * (s1 - k * c1),
            cx + r * c1,
            cy + r * s1,
        ));
        a = b;
    }
}

fn pie_plot(cx: &mut Ctx<'_>, c: &Chart, p: &Plot, (x0, y0, x1, y1): (f32, f32, f32, f32)) {
    let Some(s) = p.series.first() else { return };
    let total: f64 = s.vals.iter().flatten().filter(|v| **v > 0.0).sum();
    if total <= 0.0 {
        return;
    }
    let r = ((x1 - x0).min(y1 - y0) / 2.0 - 2.0).max(1.0);
    let (ccx, ccy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let hole = if p.kind == Kind::Doughnut {
        r * p.hole.clamp(0.1, 0.9)
    } else {
        0.0
    };
    // Angles in radians, y down: 12 o'clock is −π/2, clockwise is increasing.
    let mut a = -PI / 2.0 + p.first_angle.to_radians();
    let white = ((Rgb(255, 255, 255), 1.0), 1.0);
    let mut labels = Vec::new();
    for (i, v) in s.vals.iter().enumerate() {
        let Some(v) = v.filter(|v| *v > 0.0) else {
            continue;
        };
        let sweep = (v / total) as f32 * 2.0 * PI;
        let b = a + sweep;
        let mut path = Vec::new();
        if hole > 0.0 {
            path.push(PathCmd::Move(ccx + r * a.cos(), ccy + r * a.sin()));
            arc(&mut path, ccx, ccy, r, a, b);
            path.push(PathCmd::Line(ccx + hole * b.cos(), ccy + hole * b.sin()));
            arc(&mut path, ccx, ccy, hole, b, a);
        } else {
            path.push(PathCmd::Move(ccx, ccy));
            path.push(PathCmd::Line(ccx + r * a.cos(), ccy + r * a.sin()));
            arc(&mut path, ccx, ccy, r, a, b);
        }
        path.push(PathCmd::Close);
        cx.figure(path, Some(point_color(c, s, i)), Some(white));
        let lab = if s.labels.percent || p.labels.percent {
            Some(format!("{:.0}%", v / total * 100.0))
        } else if s.labels.value || p.labels.value {
            Some(fmt_value(v, &s.format))
        } else {
            None
        };
        if let Some(t) = lab {
            let mid = (a + b) / 2.0;
            let rr = if hole > 0.0 {
                (r + hole) / 2.0
            } else {
                r * 0.65
            };
            labels.push((t, ccx + rr * mid.cos(), ccy + rr * mid.sin()));
        }
        a = b;
    }
    for (t, x, y) in labels {
        let l = cx.label(&t, c.size * 0.9, Rgb(255, 255, 255), false, None);
        let (lw, lh) = (l.w, l.h);
        cx.put(l, x - lw / 2.0, y - lh / 2.0);
    }
}

/// Bars, lines, areas and scatter on a category (or x value) axis and a value axis.
fn axes_plot(cx: &mut Ctx<'_>, c: &Chart, (mut x0, y0, x1, mut y1): (f32, f32, f32, f32)) {
    let horizontal = c
        .plots
        .iter()
        .any(|p| matches!(p.kind, Kind::Bar { horizontal: true }));
    let scatter = c.plots.iter().all(|p| p.kind == Kind::Scatter);
    let ncat = c
        .plots
        .iter()
        .flat_map(|p| p.series.iter())
        .map(|s| s.vals.len().max(s.cats.len()))
        .max()
        .unwrap_or(0)
        .max(1);
    let cats: Vec<String> = c
        .plots
        .iter()
        .flat_map(|p| p.series.iter())
        .find(|s| !s.cats.is_empty())
        .map(|s| s.cats.clone())
        .unwrap_or_else(|| (1..=ncat).map(|i| i.to_string()).collect());
    // Value range (stacked: sums per category; percent: 0–100 %).
    let percent = c.plots.iter().any(|p| p.grouping == Grouping::Percent);
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &c.plots {
        if p.grouping == Grouping::Clustered || matches!(p.kind, Kind::Scatter) {
            for v in p.series.iter().flat_map(|s| s.vals.iter().flatten()) {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
        } else if p.grouping == Grouping::Stacked {
            for i in 0..ncat {
                let (mut pos, mut neg) = (0.0, 0.0);
                for s in &p.series {
                    let v = s.vals.get(i).copied().flatten().unwrap_or(0.0);
                    if v >= 0.0 {
                        pos += v;
                    } else {
                        neg += v;
                    }
                }
                lo = lo.min(neg);
                hi = hi.max(pos);
            }
        }
    }
    if percent {
        lo = lo.min(0.0);
        hi = hi.max(1.0);
    }
    if !lo.is_finite() || !hi.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    let va = &c.val_axis;
    let (vmin, vmax, step) = if percent {
        (0.0, 1.0, 0.1)
    } else {
        nice_scale(lo, hi, (va.min, va.max, va.major))
    };
    let vfmt = va
        .format
        .clone()
        .or_else(|| percent.then(|| "0%".to_string()))
        .or_else(|| {
            c.plots
                .iter()
                .flat_map(|p| p.series.first())
                .map(|s| s.format.clone())
                .next()
        })
        .unwrap_or_else(|| "General".into());
    let lsize = va.size.unwrap_or(c.size * 0.9);
    let lcolor = va.text.unwrap_or(c.color);
    let ticks: Vec<f64> = {
        let n = ((vmax - vmin) / step).round().clamp(1.0, 200.0) as usize;
        (0..=n)
            .map(|k| snap(vmin + k as f64 * step, step))
            .collect()
    };
    let vlabels: Vec<String> = ticks.iter().map(|t| fmt_value(*t, &vfmt)).collect();
    // Scatter x axis.
    let (xmin, xmax, xstep) = if scatter {
        let xs: Vec<f64> = c
            .plots
            .iter()
            .flat_map(|p| p.series.iter())
            .flat_map(|s| s.xs.iter().flatten().copied())
            .collect();
        let (a, b) = xs
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
                (a.min(*v), b.max(*v))
            });
        if a.is_finite() {
            nice_scale(a, b, (c.cat_axis.min, c.cat_axis.max, c.cat_axis.major))
        } else {
            (0.0, ncat as f64, 1.0)
        }
    } else {
        (0.0, 0.0, 1.0)
    };
    // Room for the axis labels.
    let csize = c.cat_axis.size.unwrap_or(c.size * 0.9);
    let ccolor = c.cat_axis.text.unwrap_or(c.color);
    let probe = cx.label("0", lsize, lcolor, false, None);
    let line_h = probe.h;
    if let Some(t) = &c.val_axis.title {
        let l = cx.label(t, lsize, lcolor, true, None);
        let lh = l.h;
        let lw = l.w;
        if horizontal {
            cx.put(l, (x0 + x1 - lw) / 2.0, y1 - lh);
            y1 -= lh + 4.0;
        } else {
            // Turned a quarter: drawn upright, beside the axis, to stay simple.
            cx.put(l, x0, (y0 + y1 - lh) / 2.0 - lh);
            x0 += lw.min(80.0) + 4.0;
        }
    }
    if let Some(t) = &c.cat_axis.title {
        let l = cx.label(t, csize, ccolor, true, None);
        let (lw, lh) = (l.w, l.h);
        cx.put(l, (x0 + x1 - lw) / 2.0, y1 - lh);
        y1 -= lh + 4.0;
    }
    let value_labels_w = if va.deleted {
        0.0
    } else {
        vlabels
            .iter()
            .map(|t| cx.label(t, lsize, lcolor, false, None).w)
            .fold(0.0, f32::max)
    };
    let cat_labels_h = if c.cat_axis.deleted { 0.0 } else { line_h };
    let cat_labels_w = if horizontal && !c.cat_axis.deleted {
        cats.iter()
            .map(|t| cx.label(t, csize, ccolor, false, None).w)
            .fold(0.0, f32::max)
            .min((x1 - x0) * 0.4)
    } else {
        0.0
    };
    let (px0, py0, px1, py1) = if horizontal {
        (x0 + cat_labels_w + 6.0, y0, x1 - 4.0, y1 - line_h - 4.0)
    } else {
        (
            x0 + value_labels_w + 6.0,
            y0 + line_h / 2.0,
            x1 - 4.0,
            y1 - cat_labels_h - 4.0,
        )
    };
    if px1 - px0 < 4.0 || py1 - py0 < 4.0 {
        return;
    }
    if let Some(f) = c.plot_fill {
        cx.rect(px0, py0, px1, py1, Some(f), None);
    }
    // Value → position along the value axis.
    let vpos = |v: f64| -> f32 {
        let t = ((v - vmin) / (vmax - vmin)) as f32;
        let t = if va.reverse { 1.0 - t } else { t };
        if horizontal {
            px0 + t * (px1 - px0)
        } else {
            py1 - t * (py1 - py0)
        }
    };
    // Gridlines and value labels.
    for (t, lab) in ticks.iter().zip(&vlabels) {
        let p = vpos(*t);
        if va.gridlines || !va.deleted {
            if horizontal {
                if va.gridlines {
                    cx.line(p, py0, p, py1, GRID, 0.75);
                }
            } else if va.gridlines {
                cx.line(px0, p, px1, p, GRID, 0.75);
            }
        }
        if !va.deleted {
            let l = cx.label(lab, lsize, lcolor, false, None);
            let (lw, lh) = (l.w, l.h);
            if horizontal {
                cx.put(l, p - lw / 2.0, py1 + 3.0);
            } else {
                cx.put(l, px0 - 5.0 - lw, p - lh / 2.0);
            }
        }
    }
    // Category axis line at the value zero (or the bottom).
    let zero = vpos(0.0f64.clamp(vmin, vmax));
    let axis_color = c.cat_axis.line.map(|c| c.0).unwrap_or(GRID);
    if !c.cat_axis.deleted || c.cat_axis.line.is_some() {
        if horizontal {
            cx.line(zero, py0, zero, py1, axis_color, 0.75);
        } else {
            cx.line(px0, zero, px1, zero, axis_color, 0.75);
        }
    }
    // Category bands.
    let len = if horizontal { py1 - py0 } else { px1 - px0 };
    let band = len / ncat as f32;
    let band_start = |i: usize| -> f32 {
        let i = if c.cat_axis.reverse ^ horizontal {
            ncat - 1 - i
        } else {
            i
        };
        if horizontal {
            py0 + i as f32 * band
        } else {
            px0 + i as f32 * band
        }
    };
    let xpos = |x: f64| -> f32 { px0 + ((x - xmin) / (xmax - xmin)) as f32 * (px1 - px0) };
    // Category labels: thinned so they don't overlap.
    if !c.cat_axis.deleted && !scatter {
        let widths: Vec<f32> = cats
            .iter()
            .map(|t| cx.label(t, csize, ccolor, false, None).w)
            .collect();
        let widest = widths.iter().copied().fold(0.0, f32::max);
        let every = if horizontal {
            ((line_h + 2.0) / band).ceil().max(1.0) as usize
        } else {
            ((widest + 6.0) / band).ceil().max(1.0) as usize
        };
        for (i, t) in cats.iter().enumerate().take(ncat) {
            if i % every != 0 {
                continue;
            }
            let l = cx.label(t, csize, ccolor, false, None);
            let (lw, lh) = (l.w, l.h);
            let mid = band_start(i) + band / 2.0;
            if horizontal {
                cx.put(l, px0 - 6.0 - lw, mid - lh / 2.0);
            } else {
                cx.put(l, mid - lw / 2.0, py1 + 3.0);
            }
        }
    } else if scatter && !c.cat_axis.deleted {
        let n = ((xmax - xmin) / xstep).round().clamp(1.0, 200.0) as usize;
        for k in 0..=n {
            let x = xmin + k as f64 * xstep;
            let l = cx.label(&fmt_value(x, "General"), csize, ccolor, false, None);
            let lw = l.w;
            cx.put(l, xpos(x) - lw / 2.0, py1 + 3.0);
        }
    }
    // The series.
    let mut k = 0;
    for p in &c.plots {
        let nser = p.series.len().max(1);
        match p.kind {
            Kind::Bar { .. } => {
                let stacked = p.grouping != Grouping::Clustered;
                let slots = if stacked {
                    1.0
                } else {
                    nser as f32 - (nser as f32 - 1.0) * p.overlap
                };
                let bar = band / (slots + p.gap).max(0.1);
                let lead = (band - bar * slots) / 2.0;
                let mut base_pos = vec![0.0f64; ncat];
                let mut base_neg = vec![0.0f64; ncat];
                let totals: Vec<f64> = (0..ncat)
                    .map(|i| {
                        p.series
                            .iter()
                            .map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0).abs())
                            .sum()
                    })
                    .collect();
                for (si, s) in p.series.iter().enumerate() {
                    let fill = series_fill(c, s, k + si);
                    for i in 0..ncat {
                        let Some(mut v) = s.vals.get(i).copied().flatten() else {
                            continue;
                        };
                        if p.grouping == Grouping::Percent && totals[i] > 0.0 {
                            v /= totals[i];
                        }
                        let (from, to) = if stacked {
                            let b = if v >= 0.0 {
                                &mut base_pos[i]
                            } else {
                                &mut base_neg[i]
                            };
                            let r = (*b, *b + v);
                            *b += v;
                            r
                        } else {
                            (0.0, v)
                        };
                        let col = if p.vary && nser == 1 {
                            point_color(c, s, i)
                        } else {
                            s.points
                                .iter()
                                .find(|(q, _)| *q == i)
                                .map(|q| q.1)
                                .unwrap_or(fill)
                        };
                        let off = if stacked {
                            lead
                        } else {
                            lead + si as f32 * bar * (1.0 - p.overlap)
                        };
                        let a = band_start(i) + off;
                        let (va0, va1) = (vpos(from.clamp(vmin, vmax)), vpos(to.clamp(vmin, vmax)));
                        let line = s.line.filter(|_| !s.no_line);
                        if horizontal {
                            cx.rect(
                                va0.min(va1),
                                a,
                                va0.max(va1),
                                a + bar,
                                (!s.no_fill).then_some(col),
                                line,
                            );
                        } else {
                            cx.rect(
                                a,
                                va0.min(va1),
                                a + bar,
                                va0.max(va1),
                                (!s.no_fill).then_some(col),
                                line,
                            );
                        }
                        if s.labels.value || p.labels.value {
                            let l = cx.label(
                                &fmt_value(v, &s.format),
                                c.size * 0.85,
                                c.color,
                                false,
                                None,
                            );
                            let (lw, lh) = (l.w, l.h);
                            if horizontal {
                                cx.put(l, va0.max(va1) + 3.0, a + bar / 2.0 - lh / 2.0);
                            } else {
                                cx.put(l, a + bar / 2.0 - lw / 2.0, va0.min(va1) - lh - 1.0);
                            }
                        }
                    }
                }
            }
            Kind::Line | Kind::Area | Kind::Radar | Kind::Scatter => {
                let mut base = vec![0.0f64; ncat];
                let totals: Vec<f64> = (0..ncat)
                    .map(|i| {
                        p.series
                            .iter()
                            .map(|s| s.vals.get(i).copied().flatten().unwrap_or(0.0))
                            .sum()
                    })
                    .collect();
                for (si, s) in p.series.iter().enumerate() {
                    let color = s
                        .line
                        .map(|l| l.0)
                        .or(s.fill)
                        .unwrap_or((auto_color(&c.accents, k + si), 1.0));
                    let width = s.line.map_or(2.25, |l| l.1);
                    let mut pts: Vec<(f32, f32)> = Vec::new();
                    // Where a missing value breaks the line (index into `pts`).
                    let mut breaks: Vec<usize> = Vec::new();
                    for i in 0..s.vals.len().min(if p.kind == Kind::Scatter {
                        usize::MAX
                    } else {
                        ncat
                    }) {
                        let Some(mut v) = s.vals.get(i).copied().flatten() else {
                            if !c.span_gaps && breaks.last() != Some(&pts.len()) {
                                breaks.push(pts.len());
                            }
                            continue;
                        };
                        if p.grouping == Grouping::Percent
                            && totals.get(i).is_some_and(|t| *t != 0.0)
                        {
                            v /= totals[i];
                        }
                        if p.grouping != Grouping::Clustered && i < ncat {
                            base[i] += v;
                            v = base[i];
                        }
                        let along = if p.kind == Kind::Scatter {
                            match s.xs.get(i).copied().flatten() {
                                Some(x) => xpos(x),
                                None => xpos((i + 1) as f64),
                            }
                        } else {
                            band_start(i) + band / 2.0
                        };
                        let vv = vpos(v.clamp(vmin, vmax));
                        pts.push(if horizontal { (vv, along) } else { (along, vv) });
                    }
                    if pts.is_empty() {
                        continue;
                    }
                    if p.kind == Kind::Area {
                        let mut path = vec![PathCmd::Move(pts[0].0, zero)];
                        path.extend(pts.iter().map(|&(x, y)| PathCmd::Line(x, y)));
                        path.push(PathCmd::Line(pts[pts.len() - 1].0, zero));
                        path.push(PathCmd::Close);
                        let fill = series_fill(c, s, k + si);
                        cx.figure(path, Some(fill), None);
                    } else if !s.no_line
                        && !(p.kind == Kind::Scatter && s.line.is_none() && s.marker)
                    {
                        let mut path = Vec::new();
                        for (n, &(x, y)) in pts.iter().enumerate() {
                            if n == 0 || breaks.contains(&n) {
                                path.push(PathCmd::Move(x, y));
                            } else {
                                path.push(PathCmd::Line(x, y));
                            }
                        }
                        cx.figure(path, None, Some((color, width)));
                    }
                    if s.marker || p.kind == Kind::Scatter {
                        let r = 2.5;
                        for &(x, y) in &pts {
                            let mut path = vec![PathCmd::Move(x + r, y)];
                            arc(&mut path, x, y, r, 0.0, 2.0 * PI);
                            path.push(PathCmd::Close);
                            cx.figure(path, Some(color), Some((color, 0.75)));
                        }
                    }
                    if s.labels.value || p.labels.value {
                        for (&(x, y), v) in pts.iter().zip(s.vals.iter().flatten()) {
                            let l = cx.label(
                                &fmt_value(*v, &s.format),
                                c.size * 0.85,
                                c.color,
                                false,
                                None,
                            );
                            let (lw, lh) = (l.w, l.h);
                            cx.put(l, x - lw / 2.0, y - lh - 3.0);
                        }
                    }
                }
            }
            Kind::Pie | Kind::Doughnut => {}
        }
        k += p.series.len();
    }
}
