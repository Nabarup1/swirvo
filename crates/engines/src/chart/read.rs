//! Reading a DrawingML chart part (`c:chartSpace`) into [`Chart`]: the cached values
//! Office stores with every chart, so no spreadsheet has to be evaluated.

use crate::docxlayout::props::Rgb;
use crate::docxlayout::xml::El;
use crate::pptxlayout::style::{Col, Colors, emu};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Bar { horizontal: bool },
    Line,
    Area,
    Pie,
    Doughnut,
    Scatter,
    Radar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    Clustered,
    Stacked,
    Percent,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Labels {
    pub value: bool,
    pub percent: bool,
    pub category: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub name: String,
    pub cats: Vec<String>,
    pub vals: Vec<Option<f64>>,
    /// Scatter x values.
    pub xs: Vec<Option<f64>>,
    pub format: String,
    pub fill: Option<Col>,
    pub no_fill: bool,
    pub line: Option<(Col, f32)>,
    pub no_line: bool,
    pub marker: bool,
    pub smooth: bool,
    /// Colours of single points (pie slices, highlighted bars).
    pub points: Vec<(usize, Col)>,
    pub labels: Labels,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plot {
    pub kind: Kind,
    pub grouping: Grouping,
    pub series: Vec<Series>,
    /// One colour per point (pies, or "vary colours by point").
    pub vary: bool,
    /// Gap between categories as a fraction of a bar's width; overlap of bars (−1..1).
    pub gap: f32,
    pub overlap: f32,
    /// Doughnut hole as a fraction of the radius.
    pub hole: f32,
    /// Degrees clockwise from 12 o'clock.
    pub first_angle: f32,
    pub labels: Labels,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Axis {
    pub deleted: bool,
    pub title: Option<String>,
    pub format: Option<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub major: Option<f64>,
    pub gridlines: bool,
    pub reverse: bool,
    pub line: Option<Col>,
    pub text: Option<Rgb>,
    pub size: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegendPos {
    Right,
    Left,
    Top,
    Bottom,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chart {
    pub title: Option<String>,
    pub title_size: f32,
    pub plots: Vec<Plot>,
    pub legend: Option<LegendPos>,
    pub cat_axis: Axis,
    pub val_axis: Axis,
    pub background: Option<Col>,
    pub border: Option<(Col, f32)>,
    pub plot_fill: Option<Col>,
    /// Default text: font, size (pt) and colour.
    pub font: String,
    pub size: f32,
    pub color: Rgb,
    /// Theme accents for automatic series colours.
    pub accents: [Rgb; 6],
    /// Join lines across missing values (`dispBlanksAs` span) instead of leaving a gap.
    pub span_gaps: bool,
}

/// Office's default accents (used when the theme has none).
pub const OFFICE_ACCENTS: [Rgb; 6] = [
    Rgb(0x44, 0x72, 0xC4),
    Rgb(0xED, 0x7D, 0x31),
    Rgb(0xA5, 0xA5, 0xA5),
    Rgb(0xFF, 0xC0, 0x00),
    Rgb(0x5B, 0x9B, 0xD5),
    Rgb(0x70, 0xAD, 0x47),
];

fn num(e: Option<&El>) -> Option<f64> {
    e?.val()?.trim().parse().ok()
}

fn flag(e: Option<&El>, default: bool) -> bool {
    match e {
        Some(e) => !matches!(e.val(), Some("0" | "false")),
        None => default,
    }
}

/// Cached points of a `c:strCache`/`c:numCache`/literal: (values by index, format code).
fn cache(e: &El) -> (Vec<Option<String>>, Option<String>) {
    let src = e
        .find("strCache")
        .or_else(|| e.find("numCache"))
        .or_else(|| e.find("strLit"))
        .or_else(|| e.find("numLit"));
    let Some(src) = src else {
        // Multi-level categories: the innermost level.
        if let Some(l) = e.find("multiLvlStrCache").and_then(|m| m.child("lvl")) {
            return points(l, None);
        }
        return (Vec::new(), None);
    };
    let fmt = src.child("formatCode").map(El::text);
    points(src, fmt)
}

fn points(src: &El, fmt: Option<String>) -> (Vec<Option<String>>, Option<String>) {
    let n = src
        .child("ptCount")
        .and_then(|c| c.val())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0)
        .min(100_000);
    let mut out = vec![None; n];
    for p in src.all("pt") {
        let Some(i) = p.attr("idx").and_then(|v| v.parse::<usize>().ok()) else {
            continue;
        };
        if i >= 100_000 {
            continue;
        }
        if i >= out.len() {
            out.resize(i + 1, None);
        }
        out[i] = p.child("v").map(El::text);
    }
    (out, fmt)
}

fn shape(colors: &Colors, sp: Option<&El>) -> (Option<Col>, bool, Option<(Col, f32)>, bool) {
    let Some(sp) = sp else {
        return (None, false, None, false);
    };
    let fill = sp
        .child("solidFill")
        .and_then(|f| colors.color_in(f, None))
        .or_else(|| {
            // A gradient: its first stop.
            sp.child("gradFill")
                .and_then(|g| g.find("gs"))
                .and_then(|gs| colors.color_in(gs, None))
        });
    let no_fill = sp.child("noFill").is_some();
    let ln = sp.child("ln");
    let width = ln.and_then(|l| l.attr("w")).and_then(emu).unwrap_or(0.75);
    let line = ln
        .and_then(|l| l.child("solidFill"))
        .and_then(|f| colors.color_in(f, None))
        .map(|c| (c, width));
    let no_line = ln.is_some_and(|l| l.child("noFill").is_some());
    (fill, no_fill, line, no_line)
}

fn labels(e: Option<&El>) -> Labels {
    let Some(d) = e else {
        return Labels::default();
    };
    if flag(d.child("delete"), false) {
        return Labels::default();
    }
    Labels {
        value: flag(d.child("showVal"), false),
        percent: flag(d.child("showPercent"), false),
        category: flag(d.child("showCatName"), false),
    }
}

fn rich_text(e: &El) -> String {
    e.all("p")
        .map(|p| {
            p.all("r")
                .map(|r| r.child("t").map(El::text).unwrap_or_default())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Text size (pt) and colour from a `c:txPr` or `c:rich` body.
fn text_props(colors: &Colors, e: Option<&El>) -> (Option<f32>, Option<Rgb>) {
    let Some(r) = e.and_then(|e| e.find("defRPr").or_else(|| e.find("rPr"))) else {
        return (None, None);
    };
    let size = r
        .attr("sz")
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| v / 100.0);
    let color = r
        .child("solidFill")
        .and_then(|f| colors.color_in(f, None))
        .map(|c| c.0);
    (size, color)
}

fn axis(colors: &Colors, e: Option<&El>) -> Axis {
    let Some(a) = e else {
        return Axis {
            deleted: true,
            ..Axis::default()
        };
    };
    let scaling = a.child("scaling");
    let (size, text) = text_props(colors, a.child("txPr"));
    let (_, _, line, no_line) = shape(colors, a.child("spPr"));
    Axis {
        deleted: flag(a.child("delete"), false),
        title: a
            .child("title")
            .and_then(|t| t.find("rich"))
            .map(rich_text)
            .filter(|t| !t.trim().is_empty()),
        format: a
            .child("numFmt")
            .filter(|f| f.attr("sourceLinked") != Some("1"))
            .and_then(|f| f.attr("formatCode"))
            .map(str::to_string),
        min: num(scaling.and_then(|s| s.child("min"))),
        max: num(scaling.and_then(|s| s.child("max"))),
        major: num(a.child("majorUnit")),
        gridlines: a.child("majorGridlines").is_some(),
        reverse: scaling
            .and_then(|s| s.child("orientation"))
            .and_then(|o| o.val())
            == Some("maxMin"),
        line: if no_line { None } else { line.map(|l| l.0) },
        text,
        size,
    }
}

fn series(colors: &Colors, s: &El) -> Series {
    let name = s
        .child("tx")
        .map(|t| {
            let (v, _) = cache(t);
            v.into_iter()
                .flatten()
                .next()
                .unwrap_or_else(|| t.child("v").map(El::text).unwrap_or_default())
        })
        .unwrap_or_default();
    let (cats, cat_fmt) = s
        .child("cat")
        .or_else(|| s.child("xVal"))
        .map(cache)
        .unwrap_or_default();
    let (vals, fmt) = s
        .child("val")
        .or_else(|| s.child("yVal"))
        .map(cache)
        .unwrap_or_default();
    let numeric = |v: &Vec<Option<String>>| -> Vec<Option<f64>> {
        v.iter()
            .map(|x| x.as_deref().and_then(|t| t.trim().parse().ok()))
            .collect()
    };
    let xs = s
        .child("xVal")
        .map(|x| numeric(&cache(x).0))
        .unwrap_or_default();
    let cat_text: Vec<String> = cats
        .iter()
        .map(|c| match (c, &cat_fmt) {
            (Some(t), Some(f)) => t
                .trim()
                .parse::<f64>()
                .map(|v| crate::xlsxlayout::numfmt::format(v, f, false).0)
                .unwrap_or_else(|_| t.clone()),
            (Some(t), None) => t.clone(),
            (None, _) => String::new(),
        })
        .collect();
    let (fill, no_fill, line, no_line) = shape(colors, s.child("spPr"));
    let marker = s.child("marker");
    let symbol = marker
        .and_then(|m| m.child("symbol"))
        .and_then(|m| m.val())
        .unwrap_or("auto");
    let points = s
        .all("dPt")
        .filter_map(|d| {
            let i = d.child("idx")?.val()?.parse().ok()?;
            let (f, ..) = shape(colors, d.child("spPr"));
            Some((i, f?))
        })
        .collect();
    Series {
        name,
        cats: cat_text,
        vals: numeric(&vals),
        xs,
        format: fmt.unwrap_or_else(|| "General".into()),
        fill,
        no_fill,
        line,
        no_line,
        marker: marker.is_some() && symbol != "none",
        smooth: flag(s.child("smooth"), false),
        points,
        labels: labels(s.child("dLbls")),
    }
}

fn plot(colors: &Colors, p: &El) -> Option<Plot> {
    let kind = match &*p.name {
        "barChart" | "bar3DChart" => Kind::Bar {
            horizontal: p.child("barDir").and_then(|d| d.val()) == Some("bar"),
        },
        "lineChart" | "line3DChart" | "stockChart" => Kind::Line,
        "areaChart" | "area3DChart" => Kind::Area,
        "pieChart" | "pie3DChart" | "ofPieChart" => Kind::Pie,
        "doughnutChart" => Kind::Doughnut,
        "scatterChart" | "bubbleChart" => Kind::Scatter,
        "radarChart" => Kind::Radar,
        _ => return None,
    };
    let grouping = match p.child("grouping").and_then(|g| g.val()) {
        Some("stacked") => Grouping::Stacked,
        Some("percentStacked") => Grouping::Percent,
        _ => Grouping::Clustered,
    };
    let mut series: Vec<(usize, Series)> = p
        .all("ser")
        .map(|s| {
            let order = s
                .child("order")
                .and_then(|o| o.val())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            (order, series(colors, s))
        })
        .collect();
    series.sort_by_key(|s| s.0);
    let line_markers = p.child("marker").is_some_and(|m| flag(Some(m), true));
    let mut series: Vec<Series> = series.into_iter().map(|s| s.1).collect();
    if kind == Kind::Line {
        // A line chart's markers: on unless the plot or the series turns them off.
        for (s, el) in series.iter_mut().zip(p.all("ser")) {
            let sym = el
                .child("marker")
                .and_then(|m| m.child("symbol"))
                .and_then(|m| m.val());
            s.marker = match sym {
                Some("none") => false,
                Some(_) => true,
                None => line_markers,
            };
        }
    }
    Some(Plot {
        kind,
        grouping,
        series,
        vary: matches!(kind, Kind::Pie | Kind::Doughnut)
            || flag(p.child("varyColors"), false) && p.all("ser").count() == 1,
        gap: p
            .child("gapWidth")
            .and_then(|g| g.val())
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(150.0)
            / 100.0,
        overlap: p
            .child("overlap")
            .and_then(|g| g.val())
            .and_then(|v| v.trim_end_matches('%').parse::<f32>().ok())
            .map_or(
                if grouping == Grouping::Clustered {
                    0.0
                } else {
                    1.0
                },
                |v| v / 100.0,
            ),
        hole: p
            .child("holeSize")
            .and_then(|g| g.val())
            .and_then(|v| v.trim_end_matches('%').parse::<f32>().ok())
            .unwrap_or(50.0)
            / 100.0,
        first_angle: p
            .child("firstSliceAng")
            .and_then(|g| g.val())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.0),
        labels: labels(p.child("dLbls")),
    })
}

/// Parse a chart part. `colors` resolves theme colours; `font` is the theme's body font.
pub fn parse(root: &El, colors: &Colors, font: &str) -> Option<Chart> {
    let chart = root.child("chart")?;
    let area = chart.child("plotArea")?;
    let plots: Vec<Plot> = area.children().filter_map(|p| plot(colors, p)).collect();
    if plots.is_empty() {
        return None;
    }
    let (size, color) = text_props(colors, root.child("txPr"));
    let title_el = chart.child("title");
    let auto_deleted = flag(chart.child("autoTitleDeleted"), false);
    let title = match title_el {
        Some(t) => t
            .find("rich")
            .map(rich_text)
            .or_else(|| {
                t.child("tx").map(|x| {
                    cache(x)
                        .0
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" ")
                })
            })
            .or_else(|| {
                // A title without text shows the only series' name.
                let all: Vec<&Series> = plots.iter().flat_map(|p| p.series.iter()).collect();
                (all.len() == 1).then(|| all[0].name.clone())
            })
            .filter(|t| !t.trim().is_empty()),
        None if !auto_deleted => {
            let all: Vec<&Series> = plots.iter().flat_map(|p| p.series.iter()).collect();
            // Office titles a single-series pie or chart with the series name.
            (all.len() == 1 && matches!(plots[0].kind, Kind::Pie | Kind::Doughnut))
                .then(|| all[0].name.clone())
                .filter(|t| !t.trim().is_empty())
        }
        None => None,
    };
    let (title_size, _) = text_props(colors, title_el);
    let legend = chart
        .child("legend")
        .map(|l| match l.child("legendPos").and_then(|p| p.val()) {
            Some("l") => LegendPos::Left,
            Some("t") => LegendPos::Top,
            Some("b") => LegendPos::Bottom,
            _ => LegendPos::Right,
        });
    let cat_axis = axis(
        colors,
        area.child("catAx")
            .or_else(|| area.child("dateAx"))
            .or_else(|| {
                // Scatter: the bottom value axis.
                area.all("valAx")
                    .find(|a| matches!(a.child("axPos").and_then(|p| p.val()), Some("b" | "t")))
            }),
    );
    let val_axis = axis(
        colors,
        area.all("valAx")
            .find(|a| matches!(a.child("axPos").and_then(|p| p.val()), Some("l" | "r")))
            .or_else(|| area.child("valAx")),
    );
    let (background, _, border, no_border) = shape(colors, root.child("spPr"));
    let (plot_fill, ..) = shape(colors, area.child("spPr"));
    let accent = |i: usize, d: Rgb| colors.theme.color(&format!("accent{}", i + 1)).unwrap_or(d);
    Some(Chart {
        title,
        title_size: title_size.unwrap_or(14.0),
        plots,
        legend,
        cat_axis,
        val_axis,
        background,
        border: if no_border { None } else { border },
        plot_fill,
        font: font.to_string(),
        size: size.unwrap_or(10.0),
        color: color.unwrap_or(Rgb(0x59, 0x59, 0x59)),
        accents: std::array::from_fn(|i| accent(i, OFFICE_ACCENTS[i])),
        span_gaps: chart.child("dispBlanksAs").and_then(|d| d.val()) == Some("span"),
    })
}
