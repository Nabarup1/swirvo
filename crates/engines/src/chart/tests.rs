use super::*;
use crate::docxlayout::layout::Op;
use crate::docxlayout::props::Rgb;
use crate::pptxlayout::style::{Colors, Fmt};

pub const COLUMN: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart><c:title><c:tx><c:rich><a:p><a:r><a:t>Sales by quarter</a:t></a:r></a:p></c:rich></c:tx></c:title>
<c:plotArea><c:barChart><c:barDir val="col"/><c:grouping val="clustered"/>
<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:strRef><c:strCache><c:ptCount val="1"/><c:pt idx="0"><c:v>North</c:v></c:pt></c:strCache></c:strRef></c:tx>
<c:cat><c:strRef><c:strCache><c:ptCount val="4"/><c:pt idx="0"><c:v>Q1</c:v></c:pt><c:pt idx="1"><c:v>Q2</c:v></c:pt><c:pt idx="2"><c:v>Q3</c:v></c:pt><c:pt idx="3"><c:v>Q4</c:v></c:pt></c:strCache></c:strRef></c:cat>
<c:val><c:numRef><c:numCache><c:formatCode>#,##0</c:formatCode><c:ptCount val="4"/><c:pt idx="0"><c:v>1200</c:v></c:pt><c:pt idx="1"><c:v>1500</c:v></c:pt><c:pt idx="2"><c:v>900</c:v></c:pt><c:pt idx="3"><c:v>2100</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>
<c:ser><c:idx val="1"/><c:order val="1"/><c:tx><c:v>South</c:v></c:tx><c:spPr><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></c:spPr>
<c:val><c:numRef><c:numCache><c:ptCount val="4"/><c:pt idx="0"><c:v>800</c:v></c:pt><c:pt idx="2"><c:v>1100</c:v></c:pt><c:pt idx="3"><c:v>1300</c:v></c:pt></c:numCache></c:numRef></c:val></c:ser>
<c:gapWidth val="219"/><c:axId val="1"/><c:axId val="2"/></c:barChart>
<c:catAx><c:axId val="1"/><c:axPos val="b"/></c:catAx><c:valAx><c:axId val="2"/><c:axPos val="l"/><c:majorGridlines/></c:valAx></c:plotArea>
<c:legend><c:legendPos val="b"/></c:legend></c:chart></c:chartSpace>"#;

pub const PIE: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:chart><c:plotArea><c:pieChart><c:varyColors val="1"/>
<c:ser><c:idx val="0"/><c:order val="0"/><c:tx><c:v>Share</c:v></c:tx>
<c:dLbls><c:showPercent val="1"/></c:dLbls>
<c:cat><c:strLit><c:ptCount val="3"/><c:pt idx="0"><c:v>A</c:v></c:pt><c:pt idx="1"><c:v>B</c:v></c:pt><c:pt idx="2"><c:v>C</c:v></c:pt></c:strLit></c:cat>
<c:val><c:numLit><c:ptCount val="3"/><c:pt idx="0"><c:v>50</c:v></c:pt><c:pt idx="1"><c:v>30</c:v></c:pt><c:pt idx="2"><c:v>20</c:v></c:pt></c:numLit></c:val></c:ser>
</c:pieChart></c:plotArea><c:legend><c:legendPos val="r"/></c:legend></c:chart></c:chartSpace>"#;

pub fn colors() -> Colors {
    Colors {
        theme: Default::default(),
        fmt: Fmt::default(),
        map: Default::default(),
    }
}

fn parse_str(s: &str) -> Chart {
    let root = crate::docxlayout::xml::parse(s).unwrap();
    parse(&root, &colors(), "Calibri").unwrap()
}

#[test]
fn reads_series_categories_and_styles() {
    let c = parse_str(COLUMN);
    assert_eq!(c.title.as_deref(), Some("Sales by quarter"));
    assert_eq!(c.legend, Some(LegendPos::Bottom));
    let p = &c.plots[0];
    assert_eq!(p.kind, Kind::Bar { horizontal: false });
    assert_eq!(p.series.len(), 2);
    assert_eq!(p.series[0].name, "North");
    assert_eq!(p.series[0].cats, ["Q1", "Q2", "Q3", "Q4"]);
    assert_eq!(
        p.series[1].vals,
        [Some(800.0), None, Some(1100.0), Some(1300.0)]
    );
    assert_eq!(p.series[1].fill.map(|f| f.0), Some(Rgb(255, 0, 0)));
    assert!((p.gap - 2.19).abs() < 1e-6);
    assert!(c.val_axis.gridlines);
    assert_eq!(
        c.accents,
        read::OFFICE_ACCENTS,
        "no theme: Office's colours"
    );
    let pie = parse_str(PIE);
    assert!(pie.plots[0].vary && pie.plots[0].series[0].labels.percent);
    assert_eq!(
        pie.title.as_deref(),
        Some("Share"),
        "a single pie is titled by its series"
    );
}

#[test]
fn scales_are_tidy() {
    assert_eq!(
        nice_scale(900.0, 2100.0, (None, None, None)),
        (0.0, 2500.0, 500.0)
    );
    assert_eq!(nice_scale(0.0, 1.0, (None, None, None)), (0.0, 1.2, 0.2));
    assert_eq!(
        nice_scale(-35.0, 80.0, (None, None, None)),
        (-40.0, 100.0, 20.0)
    );
    assert_eq!(
        nice_scale(5.0, 9.0, (Some(4.0), Some(10.0), Some(2.0))),
        (4.0, 10.0, 2.0)
    );
}

#[test]
fn draws_bars_labels_and_slices() {
    if !crate::docxlayout::fonts::Fonts::available() {
        return;
    }
    use crate::docxlayout::model::{Document, SectPr, Section};
    let doc = Document {
        sections: vec![Section {
            sect: SectPr {
                w: 600.0,
                h: 400.0,
                ..SectPr::default()
            },
            blocks: Vec::new(),
        }],
        ..Document::default()
    };
    let fonts = crate::docxlayout::fonts::Fonts::new(Default::default());
    let mut boxes = crate::docxlayout::layout::Boxes::new(&doc, fonts);
    let c = parse_str(COLUMN);
    let ops = draw(&c, 480.0, 300.0, &mut boxes);
    let text: String = ops
        .iter()
        .filter_map(|o| match o {
            Op::Text { seg, .. } => Some(seg.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");
    for want in [
        "Sales by quarter",
        "Q1",
        "Q4",
        "North",
        "South",
        "2,500",
        "0",
    ] {
        assert!(text.contains(want), "{want} missing from {text}");
    }
    let figures = ops.iter().filter(|o| matches!(o, Op::Figure(_))).count();
    // 7 bars (one value missing), gridlines, axis line, 2 legend swatches.
    assert!(figures >= 7 + 6 + 2, "{figures}");
    // Everything inside the box.
    for o in &ops {
        if let Op::Figure(f) = o {
            for p in &f.path {
                if let crate::docxlayout::layout::PathCmd::Move(x, y)
                | crate::docxlayout::layout::PathCmd::Line(x, y) = p
                {
                    assert!(
                        (-1.0..=481.0).contains(x) && (-1.0..=301.0).contains(y),
                        "{x},{y}"
                    );
                }
            }
        }
    }
    let pie = draw(&parse_str(PIE), 300.0, 200.0, &mut boxes);
    let labels: Vec<String> = pie
        .iter()
        .filter_map(|o| match o {
            Op::Text { seg, .. } => Some(seg.text.clone()),
            _ => None,
        })
        .collect();
    for want in ["50%", "30%", "20%", "A", "B", "C"] {
        assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
    }
}
