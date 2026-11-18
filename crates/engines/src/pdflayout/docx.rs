//! Layout → DOCX (docs/13 §5). Each PDF page is its own section with the page's size and
//! margins; text flows in real paragraphs at exact line pitch; columns are continuous
//! sections; pictures and shapes are anchored to the page.

use super::fonts;
use super::model::*;
use crate::doc::esc;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use swirvo_core::EngineError;

const NS: &str = r#"xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" xmlns:wp14="http://schemas.microsoft.com/office/word/2010/wordprocessingDrawing" mc:Ignorable="w14 wp14""#;

/// Points → twentieths of a point.
fn tw(pt: f32) -> i64 {
    (pt * 20.0).round() as i64
}

/// Points → English Metric Units.
fn emu(pt: f32) -> i64 {
    ((pt * 12700.0).round() as i64).max(1)
}

fn hex(c: Rgb) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Half-points for `w:sz`. Word shrinks super/subscript to about 65%, so those are scaled up.
fn half_points(r: &Run) -> i64 {
    let s = if r.script == Script::Normal {
        r.size
    } else {
        r.size / 0.65
    };
    ((s * 2.0).round() as i64).clamp(2, 3276)
}

/// Border width in eighths of a point, as Word wants it.
fn eighths(w: f32) -> i64 {
    ((w * 8.0).round() as i64).clamp(2, 96)
}

struct Rel {
    id: String,
    kind: &'static str,
    target: String,
    external: bool,
}

#[derive(PartialEq)]
struct NumDef {
    glyph: char,
    font: String,
    left: i64,
    hanging: i64,
    color: Rgb,
    half: i64,
}

struct Writer<'a> {
    l: &'a Layout,
    names: Vec<String>,
    rels: Vec<Rel>,
    media_rel: HashMap<u32, String>,
    link_rel: HashMap<u32, String>,
    nums: Vec<NumDef>,
    next_id: u32,
    z_behind: u32,
    z_front: u32,
}

/// Where the next block in a column starts, for "space before".
struct Flow {
    x0: f32,
    x1: f32,
    cursor: f32,
}

impl Writer<'_> {
    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    fn rel(&mut self, kind: &'static str, target: String, external: bool) -> String {
        let id = format!("rId{}", self.rels.len() + 10);
        self.rels.push(Rel {
            id: id.clone(),
            kind,
            target,
            external,
        });
        id
    }

    fn media(&mut self, m: u32) -> String {
        if let Some(r) = self.media_rel.get(&m) {
            return r.clone();
        }
        let ext = self.l.media.get(m as usize).map_or("png", |x| x.ext);
        let r = self.rel("image", format!("media/image{m}.{ext}"), false);
        self.media_rel.insert(m, r.clone());
        r
    }

    fn link(&mut self, k: u32) -> Option<String> {
        if let Some(r) = self.link_rel.get(&k) {
            return Some(r.clone());
        }
        let target = self.l.links.get(k as usize)?.clone();
        let r = self.rel("hyperlink", target, true);
        self.link_rel.insert(k, r.clone());
        Some(r)
    }

    fn font(&self, f: u16) -> &str {
        self.names.get(f as usize).map_or("Arial", String::as_str)
    }

    // ------------------------------------------------------------ runs

    fn rpr(&self, r: &Run) -> String {
        let f = esc(self.font(r.font));
        let mut s = format!(
            r#"<w:rPr><w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/>"#
        );
        if r.bold {
            s.push_str("<w:b/><w:bCs/>");
        }
        if r.italic {
            s.push_str("<w:i/><w:iCs/>");
        }
        if r.strike {
            s.push_str("<w:strike/>");
        }
        if r.color != BLACK {
            let _ = write!(s, r#"<w:color w:val="{}"/>"#, hex(r.color));
        }
        let hp = half_points(r);
        let _ = write!(s, r#"<w:sz w:val="{hp}"/><w:szCs w:val="{hp}"/>"#);
        if let Some(u) = r.underline {
            if u == r.color {
                s.push_str(r#"<w:u w:val="single"/>"#);
            } else {
                let _ = write!(s, r#"<w:u w:val="single" w:color="{}"/>"#, hex(u));
            }
        }
        if let Some(h) = r.highlight {
            let _ = write!(
                s,
                r#"<w:shd w:val="clear" w:color="auto" w:fill="{}"/>"#,
                hex(h)
            );
        }
        match r.script {
            Script::Super => s.push_str(r#"<w:vertAlign w:val="superscript"/>"#),
            Script::Sub => s.push_str(r#"<w:vertAlign w:val="subscript"/>"#),
            Script::Normal => {}
        }
        s.push_str("</w:rPr>");
        s
    }

    fn run(&self, r: &Run) -> String {
        format!(
            r#"<w:r>{}<w:t xml:space="preserve">{}</w:t></w:r>"#,
            self.rpr(r),
            esc(&r.text)
        )
    }

    /// The runs of a paragraph: lines joined with spaces (hyphenation undone), links wrapped.
    fn runs(&mut self, p: &Para) -> String {
        let mut flat: Vec<Piece> = Vec::new();
        for (i, line) in p.lines.iter().enumerate() {
            if i > 0 {
                let next_lower = line
                    .iter()
                    .find_map(|x| match x {
                        Piece::Run(r) => r.text.chars().next(),
                        _ => None,
                    })
                    .is_some_and(char::is_lowercase);
                let mut joined = false;
                if let Some(Piece::Run(r)) = flat.last_mut() {
                    let t = r.text.trim_end();
                    let hyphen =
                        t.ends_with('-') && t.chars().rev().nth(1).is_some_and(char::is_alphabetic);
                    if hyphen && next_lower {
                        let n = t.len() - 1;
                        r.text.truncate(n);
                        joined = true;
                    } else if !r.text.ends_with(' ') {
                        r.text.push(' ');
                        joined = true;
                    } else {
                        joined = true;
                    }
                }
                if !joined {
                    flat.push(Piece::Run(Run {
                        text: " ".into(),
                        font: self.l.body_font,
                        size: p.size,
                        color: BLACK,
                        bold: false,
                        italic: false,
                        underline: None,
                        strike: false,
                        highlight: None,
                        script: Script::Normal,
                        link: None,
                    }));
                }
            }
            flat.extend(line.iter().cloned());
        }
        let mut s = String::new();
        let mut open_link: Option<u32> = None;
        for piece in &flat {
            let link = match piece {
                Piece::Run(r) => r.link,
                _ => None,
            };
            if link != open_link {
                if open_link.is_some() {
                    s.push_str("</w:hyperlink>");
                }
                open_link = None;
                if let Some(k) = link
                    && let Some(id) = self.link(k)
                {
                    let _ = write!(s, r#"<w:hyperlink r:id="{id}" w:history="1">"#);
                    open_link = Some(k);
                }
            }
            match piece {
                Piece::Run(r) => s.push_str(&self.run(r)),
                Piece::Tab => s.push_str("<w:r><w:tab/></w:r>"),
                Piece::PageNumber(r) | Piece::PageCount(r) => {
                    let instr = if matches!(piece, Piece::PageNumber(_)) {
                        "PAGE"
                    } else {
                        "NUMPAGES"
                    };
                    let _ = write!(
                        s,
                        r#"<w:fldSimple w:instr=" {instr} ">{}</w:fldSimple>"#,
                        self.run(r)
                    );
                }
            }
        }
        if open_link.is_some() {
            s.push_str("</w:hyperlink>");
        }
        s
    }

    // ------------------------------------------------------------ paragraphs

    fn num_id(&mut self, b: &Bullet, left: i64, hanging: i64) -> usize {
        let fam = self.font(b.font).to_string();
        let symbolic = [
            "symbol",
            "wingdings",
            "dingbat",
            "segoe ui symbol",
            "opensymbol",
            "open symbol",
        ]
        .iter()
        .any(|s| fam.to_ascii_lowercase().contains(s));
        let font = match b.glyph {
            '•' | '◦' | '▪' | '·' | '–' | '-' | '*' if symbolic => "Arial".to_string(),
            '•' | '◦' | '▪' | '·' | '–' | '-' | '*' => fam,
            _ => "Segoe UI Symbol".to_string(),
        };
        let d = NumDef {
            glyph: b.glyph,
            font,
            left,
            hanging,
            color: b.color,
            half: ((b.size * 2.0).round() as i64).clamp(2, 3276),
        };
        if let Some(i) = self.nums.iter().position(|n| *n == d) {
            return i + 1;
        }
        self.nums.push(d);
        self.nums.len()
    }

    /// A paragraph. `extra` is inserted before the runs (anchors, column breaks after).
    fn para(
        &mut self,
        p: &Para,
        f: &mut Flow,
        sect: Option<&str>,
        before_runs: &str,
        after_runs: &str,
    ) -> String {
        let space = (p.top() - f.cursor).max(0.0);
        f.cursor = p.bottom();
        let (x0, x1) = (f.x0, f.x1);
        let mut ppr = String::new();
        if let Some(h) = p.heading {
            let _ = write!(ppr, r#"<w:pStyle w:val="Heading{h}"/>"#);
        }
        let (mut left, mut right, mut first) = (p.left - x0, 0.0f32, p.first - p.left);
        match p.align {
            Align::Center => {
                let c = (p.left.min(p.first) + p.right) / 2.0;
                let hw = (c - x0).min(x1 - c).max(1.0);
                left = c - hw - x0;
                right = x1 - (c + hw);
                first = 0.0;
            }
            Align::Right => {
                left = 0.0;
                right = x1 - p.right;
                first = 0.0;
            }
            Align::Justify => right = x1 - p.right,
            Align::Left if p.lines.len() > 1 => {
                let slack = 0.015 * (p.right - p.left);
                right = x1 - p.right - slack;
            }
            Align::Left => {}
        }
        if let Some(b) = &p.bullet {
            let n = self.num_id(b, tw(left), tw(p.left - p.first));
            let _ = write!(
                ppr,
                r#"<w:numPr><w:ilvl w:val="0"/><w:numId w:val="{n}"/></w:numPr>"#
            );
        }
        if !p.tabs.is_empty() {
            ppr.push_str("<w:tabs>");
            for t in &p.tabs {
                let _ = write!(
                    ppr,
                    r#"<w:tab w:val="{}"{} w:pos="{}"/>"#,
                    if t.right { "right" } else { "left" },
                    if t.dots { r#" w:leader="dot""# } else { "" },
                    tw(t.x - x0).max(0)
                );
            }
            ppr.push_str("</w:tabs>");
        }
        let tallest = p
            .lines
            .iter()
            .flatten()
            .filter_map(|x| match x {
                Piece::Run(r) if r.script == Script::Normal => Some(r.size),
                _ => None,
            })
            .fold(0.0, f32::max);
        let rule = if tallest > p.pitch * 1.05 {
            "atLeast"
        } else {
            "exact"
        };
        let _ = write!(
            ppr,
            r#"<w:spacing w:before="{}" w:after="0" w:line="{}" w:lineRule="{rule}"/>"#,
            tw(space),
            tw(p.pitch).max(20)
        );
        let _ = write!(
            ppr,
            r#"<w:ind w:left="{}" w:right="{}" {}/>"#,
            tw(left),
            tw(right.max(0.0)),
            if first >= 0.0 {
                format!(r#"w:firstLine="{}""#, tw(first))
            } else {
                format!(r#"w:hanging="{}""#, tw(-first))
            }
        );
        let jc = match p.align {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "both",
        };
        let _ = write!(ppr, r#"<w:jc w:val="{jc}"/>"#);
        if let Some(s) = sect {
            ppr.push_str(s);
        }
        let runs = self.runs(p);
        format!("<w:p><w:pPr>{ppr}</w:pPr>{before_runs}{runs}{after_runs}</w:p>")
    }

    /// An empty paragraph of a given exact height (spacers, section and column carriers).
    fn carrier(&self, height: f32, sect: Option<&str>, runs: &str) -> String {
        format!(
            r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="{}" w:lineRule="exact"/><w:ind w:left="0" w:right="0"/><w:rPr><w:sz w:val="2"/><w:szCs w:val="2"/></w:rPr>{}</w:pPr>{runs}</w:p>"#,
            tw(height.max(0.05)).max(1),
            sect.unwrap_or("")
        )
    }

    // ------------------------------------------------------------ drawings

    fn pic_graphic(&mut self, media: u32, w: f32, h: f32) -> String {
        let rid = self.media(media);
        let id = self.id();
        format!(
            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{id}" name="Picture {id}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>"#,
            emu(w),
            emu(h)
        )
    }

    fn inline_picture(&mut self, media: u32, w: f32, h: f32) -> String {
        let g = self.pic_graphic(media, w, h);
        let id = self.id();
        format!(
            r#"<w:r><w:rPr><w:sz w:val="2"/></w:rPr><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{}" cy="{}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{id}" name="Picture {id}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>{g}</wp:inline></w:drawing></w:r>"#,
            emu(w),
            emu(h)
        )
    }

    fn anchor(&mut self, b: BBox, behind: bool, graphic: &str) -> String {
        let id = self.id();
        let z = if behind {
            self.z_behind += 1;
            self.z_behind
        } else {
            self.z_front += 1;
            self.z_front
        };
        format!(
            r#"<wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="{z}" behindDoc="{}" locked="0" layoutInCell="0" allowOverlap="1"><wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>{}</wp:posOffset></wp:positionH><wp:positionV relativeFrom="page"><wp:posOffset>{}</wp:posOffset></wp:positionV><wp:extent cx="{}" cy="{}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="{id}" name="Shape {id}"/><wp:cNvGraphicFramePr/>{graphic}</wp:anchor>"#,
            u8::from(behind),
            emu(b.x0),
            emu(b.y0),
            emu(b.w()),
            emu(b.h())
        )
    }

    fn float(&mut self, f: &Float) -> String {
        match *f {
            Float::Picture {
                media,
                bbox,
                behind,
            } => {
                let g = self.pic_graphic(media, bbox.w(), bbox.h());
                let a = self.anchor(bbox, behind, &g);
                format!("<w:r><w:drawing>{a}</w:drawing></w:r>")
            }
            Float::Rect { bbox, fill, stroke } => {
                let fill = match fill {
                    Some(c) => format!(
                        r#"<a:solidFill><a:srgbClr val="{}"/></a:solidFill>"#,
                        hex(c)
                    ),
                    None => "<a:noFill/>".into(),
                };
                let line = match stroke {
                    Some(s) => format!(
                        r#"<a:ln w="{}"><a:solidFill><a:srgbClr val="{}"/></a:solidFill></a:ln>"#,
                        emu(s.width),
                        hex(s.color)
                    ),
                    None => "<a:ln><a:noFill/></a:ln>".into(),
                };
                let g = format!(
                    r#"<a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"><wps:wsp><wps:cNvSpPr/><wps:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom>{fill}{line}</wps:spPr><wps:bodyPr/></wps:wsp></a:graphicData></a:graphic>"#,
                    emu(bbox.w()),
                    emu(bbox.h())
                );
                let a = self.anchor(bbox, true, &g);
                format!(
                    r#"<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing>{a}</w:drawing></mc:Choice></mc:AlternateContent></w:r>"#
                )
            }
        }
    }

    // ------------------------------------------------------------ tables

    fn border(side: &str, b: Option<Border>) -> String {
        match b {
            Some(b) => format!(
                r#"<w:{side} w:val="single" w:sz="{}" w:space="0" w:color="{}"/>"#,
                eighths(b.width),
                hex(b.color)
            ),
            None => format!(r#"<w:{side} w:val="nil"/>"#),
        }
    }

    fn table(&mut self, t: &Table, f: &mut Flow) -> String {
        let total: f32 = t.cols.iter().sum();
        let mut s = format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="{}" w:type="dxa"/><w:tblInd w:w="{}" w:type="dxa"/><w:tblBorders><w:top w:val="nil"/><w:left w:val="nil"/><w:bottom w:val="nil"/><w:right w:val="nil"/><w:insideH w:val="nil"/><w:insideV w:val="nil"/></w:tblBorders><w:tblLayout w:type="fixed"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="{p}" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="{p}" w:type="dxa"/></w:tblCellMar><w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid>"#,
            tw(total),
            tw(t.bbox.x0 - f.x0),
            p = tw(t.pad)
        );
        for c in &t.cols {
            let _ = write!(s, r#"<w:gridCol w:w="{}"/>"#, tw(*c));
        }
        s.push_str("</w:tblGrid>");
        let mut y = t.bbox.y0;
        for row in &t.rows {
            let _ = write!(
                s,
                r#"<w:tr><w:trPr><w:cantSplit/><w:trHeight w:val="{}" w:hRule="atLeast"/></w:trPr>"#,
                tw(row.height)
            );
            let mut col = 0usize;
            for cell in &row.cells {
                let span = cell.span.max(1);
                let w: f32 = t.cols[col.min(t.cols.len())..(col + span).min(t.cols.len())]
                    .iter()
                    .sum();
                let x = t.bbox.x0 + t.cols[..col.min(t.cols.len())].iter().sum::<f32>();
                col += span;
                let _ = write!(s, r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/>"#, tw(w));
                if span > 1 {
                    let _ = write!(s, r#"<w:gridSpan w:val="{span}"/>"#);
                }
                match cell.vmerge {
                    VMerge::Start => s.push_str(r#"<w:vMerge w:val="restart"/>"#),
                    VMerge::Continue => s.push_str("<w:vMerge/>"),
                    VMerge::None => {}
                }
                let [top, left, bottom, right] = cell.borders;
                let _ = write!(
                    s,
                    "<w:tcBorders>{}{}{}{}</w:tcBorders>",
                    Self::border("top", top),
                    Self::border("left", left),
                    Self::border("bottom", bottom),
                    Self::border("right", right)
                );
                if let Some(fill) = cell.fill {
                    let _ = write!(
                        s,
                        r#"<w:shd w:val="clear" w:color="auto" w:fill="{}"/>"#,
                        hex(fill)
                    );
                }
                let va = match cell.valign {
                    VAlign::Top => "top",
                    VAlign::Center => "center",
                    VAlign::Bottom => "bottom",
                };
                let _ = write!(s, r#"<w:vAlign w:val="{va}"/></w:tcPr>"#);
                if cell.paras.is_empty() {
                    s.push_str(&self.carrier(1.0, None, ""));
                } else {
                    let mut cf = Flow {
                        x0: x + t.pad,
                        x1: x + w - t.pad,
                        cursor: 0.0,
                    };
                    let first_top = cell.paras[0].top();
                    cf.cursor = if cell.valign == VAlign::Top {
                        first_top - cell.pad_top
                    } else {
                        first_top
                    };
                    for p in &cell.paras {
                        s.push_str(&self.para(p, &mut cf, None, "", ""));
                    }
                }
                s.push_str("</w:tc>");
            }
            s.push_str("</w:tr>");
            y += row.height;
        }
        s.push_str("</w:tbl>");
        f.cursor = y.max(t.bbox.y1);
        s
    }

    // ------------------------------------------------------------ sections

    /// Section properties for one page. Each page is its own section, so each keeps its size
    /// and margins.
    fn sect_pr(&self, page: &Page, first: bool) -> String {
        let mut s = String::from("<w:sectPr>");
        if self.l.header.is_some() {
            let _ = write!(
                s,
                r#"<w:headerReference w:type="default" r:id="{}"/>"#,
                if page.header { "rIdHdr1" } else { "rIdHdr0" }
            );
        }
        if self.l.footer.is_some() {
            let _ = write!(
                s,
                r#"<w:footerReference w:type="default" r:id="{}"/>"#,
                if page.footer { "rIdFtr1" } else { "rIdFtr0" }
            );
        }
        if !first {
            s.push_str(r#"<w:type w:val="nextPage"/>"#);
        }
        let (w, h) = (tw(page.w), tw(page.h));
        let _ = write!(
            s,
            r#"<w:pgSz w:w="{w}" w:h="{h}"{}/>"#,
            if w > h {
                r#" w:orient="landscape""#
            } else {
                ""
            }
        );
        let m = page.margins;
        let header = self.l.header.as_ref().map_or(36.0, |f| f.offset);
        let footer = self.l.footer.as_ref().map_or(36.0, |f| f.offset);
        let _ = write!(
            s,
            r#"<w:pgMar w:top="{}" w:right="{}" w:bottom="{}" w:left="{}" w:header="{}" w:footer="{}" w:gutter="0"/>"#,
            tw(m.top),
            tw(m.right),
            tw(m.bottom),
            tw(m.left),
            tw(header),
            tw(footer)
        );
        s.push_str(r#"<w:cols w:space="720"/>"#);
        s.push_str("</w:sectPr>");
        s
    }

    /// One block in a flow. `lead` (anchors) goes into the first paragraph written.
    fn block(
        &mut self,
        b: &Block,
        f: &mut Flow,
        lead: &mut String,
        prev_table: &mut bool,
    ) -> String {
        let lead_now = std::mem::take(lead);
        match b {
            Block::Para(p) => {
                *prev_table = false;
                self.para(p, f, None, &lead_now, "")
            }
            Block::Picture(pic) => {
                *prev_table = false;
                let space = (pic.bbox.y0 - f.cursor).max(0.0);
                let run = self.inline_picture(pic.media, pic.bbox.w(), pic.bbox.h());
                f.cursor = pic.bbox.y1;
                format!(
                    r#"<w:p><w:pPr><w:spacing w:before="{}" w:after="0" w:line="240" w:lineRule="auto"/><w:ind w:left="{}" w:right="0"/><w:rPr><w:sz w:val="2"/><w:szCs w:val="2"/></w:rPr></w:pPr>{lead_now}{run}</w:p>"#,
                    tw(space),
                    tw(pic.bbox.x0 - f.x0).max(0)
                )
            }
            Block::Table(t) => {
                let mut s = String::new();
                let gap = t.bbox.y0 - f.cursor;
                // A paragraph between tables keeps Word from merging them.
                if gap > 0.5 || *prev_table || !lead_now.is_empty() {
                    s.push_str(&self.carrier(gap.max(0.05), None, &lead_now));
                    f.cursor += gap.max(0.05);
                }
                s.push_str(&self.table(t, f));
                *prev_table = true;
                s
            }
        }
    }

    /// Side-by-side columns as an invisible one-row table: every editor lays that out the
    /// same way, unlike section columns and column breaks.
    fn columns(&mut self, band: &Band, left: f32, right: f32, f: &mut Flow) -> String {
        let n = band.cols.len();
        let starts: Vec<f32> = (0..n)
            .map(|i| if i == 0 { left } else { band.cols[i].x0 })
            .collect();
        let ends: Vec<f32> = (0..n)
            .map(|i| if i + 1 == n { right } else { starts[i + 1] })
            .collect();
        let total = right - left;
        let mut s = format!(
            r#"<w:tbl><w:tblPr><w:tblW w:w="{}" w:type="dxa"/><w:tblInd w:w="0" w:type="dxa"/><w:tblBorders><w:top w:val="nil"/><w:left w:val="nil"/><w:bottom w:val="nil"/><w:right w:val="nil"/><w:insideH w:val="nil"/><w:insideV w:val="nil"/></w:tblBorders><w:tblLayout w:type="fixed"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="0" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="0" w:type="dxa"/></w:tblCellMar><w:tblLook w:val="0000" w:firstRow="0" w:lastRow="0" w:firstColumn="0" w:lastColumn="0" w:noHBand="1" w:noVBand="1"/></w:tblPr><w:tblGrid>"#,
            tw(total)
        );
        for i in 0..n {
            let _ = write!(
                s,
                r#"<w:gridCol w:w="{}"/>"#,
                tw(ends[i] - starts[i]).max(20)
            );
        }
        s.push_str("</w:tblGrid><w:tr>");
        let top = f.cursor;
        let mut bottom = top;
        for (i, col) in band.cols.iter().enumerate() {
            // Indents are measured from the cell's edges.
            let mut cf = Flow {
                x0: starts[i],
                x1: ends[i],
                cursor: top,
            };
            let _ = write!(
                s,
                r#"<w:tc><w:tcPr><w:tcW w:w="{}" w:type="dxa"/><w:tcBorders><w:top w:val="nil"/><w:left w:val="nil"/><w:bottom w:val="nil"/><w:right w:val="nil"/></w:tcBorders></w:tcPr>"#,
                tw(ends[i] - starts[i]).max(20)
            );
            let mut prev_table = false;
            let mut none = String::new();
            let mut last_table = false;
            for b in &col.blocks {
                s.push_str(&self.block(b, &mut cf, &mut none, &mut prev_table));
                last_table = matches!(b, Block::Table(_));
            }
            // A cell must end with a paragraph.
            if col.blocks.is_empty() || last_table {
                s.push_str(&self.carrier(0.05, None, ""));
            }
            s.push_str("</w:tc>");
            bottom = bottom.max(cf.cursor);
        }
        s.push_str("</w:tr></w:tbl>");
        f.cursor = bottom;
        s
    }

    /// Body XML for one page: a single section; floats are anchored in its first paragraph.
    /// Returns the section properties when this is the document's last page (they go at the
    /// end of the body there), else puts them in the page's last paragraph.
    fn page(
        &mut self,
        page: &Page,
        first_page: bool,
        last_page: bool,
        body: &mut String,
    ) -> Option<String> {
        let left = page.margins.left;
        let right = page.w - page.margins.right;
        let mut lead = String::new();
        for f in &page.floats {
            lead.push_str(&self.float(f));
        }
        let sect = self.sect_pr(page, first_page);
        let mut f = Flow {
            x0: left,
            x1: right,
            cursor: page.margins.top,
        };
        let mut parts: Vec<(String, bool)> = Vec::new(); // (xml, is a top-level paragraph)
        let mut prev_table = false;
        for band in &page.bands {
            if band.cols.len() == 1 {
                for b in &band.cols[0].blocks {
                    let x = self.block(b, &mut f, &mut lead, &mut prev_table);
                    parts.push((x, !matches!(b, Block::Table(_))));
                }
            } else {
                let gap = band.top() - f.cursor;
                if gap > 0.5 || prev_table || !lead.is_empty() {
                    let l = std::mem::take(&mut lead);
                    parts.push((self.carrier(gap.max(0.05), None, &l), true));
                    f.cursor += gap.max(0.05);
                }
                let x = self.columns(band, left, right, &mut f);
                parts.push((x, false));
                prev_table = true;
            }
        }
        if parts.is_empty() || !lead.is_empty() || parts.last().is_some_and(|p| !p.1) {
            let l = std::mem::take(&mut lead);
            parts.push((self.carrier(0.05, None, &l), true));
        }
        if !last_page
            && let Some((xml, _)) = parts.last_mut()
            && let Some(i) = xml.find("</w:pPr>")
        {
            xml.insert_str(i, &sect);
        }
        for (x, _) in parts {
            body.push_str(&x);
        }
        last_page.then_some(sect)
    }
}

fn fonts_xml(names: &[String], infos: &[FontInfo]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:fonts {NS}>"#);
    for (n, f) in names.iter().zip(infos) {
        if seen.contains(&n.as_str()) {
            continue;
        }
        seen.push(n);
        let family = if f.mono {
            "modern"
        } else if f.serif {
            "roman"
        } else {
            "swiss"
        };
        let _ = write!(
            s,
            r#"<w:font w:name="{}"><w:family w:val="{family}"/><w:pitch w:val="{}"/></w:font>"#,
            esc(n),
            if f.mono { "fixed" } else { "variable" }
        );
    }
    s.push_str("</w:fonts>");
    s
}

fn styles_xml(body_font: &str, body_half: i64) -> String {
    let f = esc(body_font);
    let mut s = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:eastAsia="{f}" w:cs="{f}"/><w:sz w:val="{body_half}"/><w:szCs w:val="{body_half}"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:widowControl w:val="0"/><w:spacing w:after="0" w:line="240" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/><w:pPr><w:widowControl w:val="0"/></w:pPr></w:style>"#
    );
    for k in 1..=3 {
        let _ = write!(
            s,
            r#"<w:style w:type="paragraph" w:styleId="Heading{k}"><w:name w:val="heading {k}"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:uiPriority w:val="9"/><w:qFormat/><w:pPr><w:outlineLvl w:val="{}"/></w:pPr></w:style>"#,
            k - 1
        );
    }
    s.push_str(r#"<w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont"><w:name w:val="Default Paragraph Font"/><w:uiPriority w:val="1"/><w:semiHidden/></w:style><w:style w:type="table" w:default="1" w:styleId="TableNormal"><w:name w:val="Normal Table"/><w:semiHidden/><w:tblPr><w:tblInd w:w="0" w:type="dxa"/><w:tblCellMar><w:top w:w="0" w:type="dxa"/><w:left w:w="108" w:type="dxa"/><w:bottom w:w="0" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style></w:styles>"#);
    s
}

const SETTINGS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat><w:compatSetting w:name="compatibilityMode" w:uri="http://schemas.microsoft.com/office/word" w:val="15"/></w:compat></w:settings>"#;

impl Writer<'_> {
    fn numbering_xml(&self) -> String {
        let mut s =
            format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering {NS}>"#);
        for (i, n) in self.nums.iter().enumerate() {
            let f = esc(&n.font);
            let _ = write!(
                s,
                r#"<w:abstractNum w:abstractNumId="{i}"><w:multiLevelType w:val="singleLevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="{}"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="{}" w:hanging="{}"/></w:pPr><w:rPr><w:rFonts w:ascii="{f}" w:hAnsi="{f}" w:cs="{f}" w:hint="default"/><w:color w:val="{}"/><w:sz w:val="{}"/><w:szCs w:val="{}"/></w:rPr></w:lvl></w:abstractNum>"#,
                esc(&n.glyph.to_string()),
                n.left,
                n.hanging,
                hex(n.color),
                n.half,
                n.half
            );
        }
        for i in 0..self.nums.len() {
            let _ = write!(
                s,
                r#"<w:num w:numId="{}"><w:abstractNumId w:val="{i}"/></w:num>"#,
                i + 1
            );
        }
        s.push_str("</w:numbering>");
        s
    }

    fn furniture_xml(
        &mut self,
        f: Option<&Furniture>,
        header: bool,
        page: Option<&Page>,
    ) -> String {
        let tag = if header { "hdr" } else { "ftr" };
        let mut s =
            format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:{tag} {NS}>"#);
        match (f, page) {
            (Some(f), Some(page)) if !f.paras.is_empty() => {
                let mut flow = Flow {
                    x0: page.margins.left,
                    x1: page.w - page.margins.right,
                    cursor: f.paras[0].top(),
                };
                for p in &f.paras {
                    s.push_str(&self.para(p, &mut flow, None, "", ""));
                }
            }
            _ => s.push_str(&self.carrier(1.0, None, "")),
        }
        let _ = write!(s, "</w:{tag}>");
        s
    }
}

/// Write the layout as a DOCX package at `out`.
pub fn write(l: &Layout, fallback_title: &str, out: &Path) -> Result<(), EngineError> {
    let names: Vec<String> = l.fonts.iter().map(fonts::substitute).collect();
    let mut w = Writer {
        l,
        names,
        rels: Vec::new(),
        media_rel: HashMap::new(),
        link_rel: HashMap::new(),
        nums: Vec::new(),
        next_id: 0,
        z_behind: 1000,
        z_front: 500_000,
    };
    let mut body = String::new();
    let n = l.pages.len();
    let mut final_sect = None;
    for (i, page) in l.pages.iter().enumerate() {
        if let Some(s) = w.page(page, i == 0, i + 1 == n, &mut body) {
            final_sect = Some(s);
        }
    }
    if body.is_empty() {
        body.push_str("<w:p/>");
    }
    let final_sect = final_sect.unwrap_or_default();
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}{final_sect}</w:body></w:document>"#
    );
    // Headers and footers use the first page that has them for their geometry.
    let hpage = l.pages.iter().find(|p| p.header);
    let fpage = l.pages.iter().find(|p| p.footer);
    let parts = [
        (
            "header1.xml",
            w.furniture_xml(l.header.as_ref(), true, hpage),
        ),
        ("header0.xml", w.furniture_xml(None, true, None)),
        (
            "footer1.xml",
            w.furniture_xml(l.footer.as_ref(), false, fpage),
        ),
        ("footer0.xml", w.furniture_xml(None, false, None)),
    ];
    let numbering = w.numbering_xml();
    let body_name = w.font(l.body_font).to_string();
    let styles = styles_xml(
        &body_name,
        ((l.body_size * 2.0).round() as i64).clamp(2, 3276),
    );

    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdSet" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings" Target="settings.xml"/><Relationship Id="rIdF" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/fontTable" Target="fontTable.xml"/><Relationship Id="rIdN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    let has_header = l.header.is_some();
    let has_footer = l.footer.is_some();
    if has_header {
        rels.push_str(r#"<Relationship Id="rIdHdr1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/><Relationship Id="rIdHdr0" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header0.xml"/>"#);
    }
    if has_footer {
        rels.push_str(r#"<Relationship Id="rIdFtr1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/><Relationship Id="rIdFtr0" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer0.xml"/>"#);
    }
    for r in &w.rels {
        let _ = write!(
            rels,
            r#"<Relationship Id="{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{}" Target="{}"{}/>"#,
            r.id,
            r.kind,
            esc(&r.target),
            if r.external {
                r#" TargetMode="External""#
            } else {
                ""
            }
        );
    }
    rels.push_str("</Relationships>");

    let mut types = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="jpeg" ContentType="image/jpeg"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/><Override PartName="/word/fontTable.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.fontTable+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#,
    );
    for (name, _) in &parts {
        let header = name.starts_with("header");
        if (header && has_header) || (!header && has_footer) {
            let _ = write!(
                types,
                r#"<Override PartName="/word/{name}" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.{}+xml"/>"#,
                if header { "header" } else { "footer" }
            );
        }
    }
    types.push_str("</Types>");

    let title = l
        .title
        .clone()
        .unwrap_or_else(|| fallback_title.to_string());
    let core = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>Swirvo</dc:creator></cp:coreProperties>"#,
        esc(&title)
    );

    let file = std::fs::File::create(out)?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let deflate = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut put = |name: &str, data: &[u8], compress: bool| -> Result<(), EngineError> {
        z.start_file(name, if compress { deflate } else { stored })
            .map_err(EngineError::encode)?;
        z.write_all(data)?;
        Ok(())
    };
    put("[Content_Types].xml", types.as_bytes(), true)?;
    put(
        "_rels/.rels",
        br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#,
        true,
    )?;
    put("docProps/core.xml", core.as_bytes(), true)?;
    put("word/document.xml", document.as_bytes(), true)?;
    put("word/styles.xml", styles.as_bytes(), true)?;
    put("word/settings.xml", SETTINGS.as_bytes(), true)?;
    put(
        "word/fontTable.xml",
        fonts_xml(&w.names, &l.fonts).as_bytes(),
        true,
    )?;
    put("word/numbering.xml", numbering.as_bytes(), true)?;
    for (name, xml) in &parts {
        let header = name.starts_with("header");
        if (header && has_header) || (!header && has_footer) {
            put(&format!("word/{name}"), xml.as_bytes(), true)?;
        }
    }
    put("word/_rels/document.xml.rels", rels.as_bytes(), true)?;
    let mut used: Vec<u32> = w.media_rel.keys().copied().collect();
    used.sort_unstable();
    for m in used {
        if let Some(media) = l.media.get(m as usize) {
            put(
                &format!("word/media/image{m}.{}", media.ext),
                &media.bytes,
                false,
            )?;
        }
    }
    z.finish().map_err(EngineError::encode)?.flush()?;
    Ok(())
}
