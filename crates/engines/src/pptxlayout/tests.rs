use std::io::Write as _;

use super::*;

const NS: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#;

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in items {
        s.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        ));
    }
    s.push_str("</Relationships>");
    s
}

fn theme() -> String {
    format!(
        r#"<a:theme {NS} name="Office"><a:themeElements>
<a:clrScheme name="Office"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme>
<a:fontScheme name="Office"><a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme>
<a:fmtScheme name="Office"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:gradFill><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"><a:lumMod val="110000"/></a:schemeClr></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:shade val="78000"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="5400000" scaled="0"/></a:gradFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst>
<a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst>
<a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst>
<a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:tint val="95000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme>
</a:themeElements></a:theme>"#
    )
}

fn sp(id: u32, name: &str, nvpr: &str, sppr: &str, style: &str, tx: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{name}"/><p:cNvSpPr/><p:nvPr>{nvpr}</p:nvPr></p:nvSpPr><p:spPr>{sppr}</p:spPr>{style}{tx}</p:sp>"#
    )
}

fn xfrm(x: i64, y: i64, w: i64, h: i64, extra: &str) -> String {
    format!(r#"<a:xfrm{extra}><a:off x="{x}" y="{y}"/><a:ext cx="{w}" cy="{h}"/></a:xfrm>"#)
}

fn para(text: &str, ppr: &str) -> String {
    format!(r#"<a:p>{ppr}<a:r><a:rPr lang="en-US"/><a:t>{text}</a:t></a:r></a:p>"#)
}

const E: i64 = 12_700;

/// A 16:9 deck with a master, a layout, two slides and a hidden one.
pub(crate) fn sample_pptx() -> Vec<u8> {
    let master = format!(
        r#"<p:sldMaster {NS}><p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
{title}{body}{band}</p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:txStyles><p:titleStyle><a:lvl1pPr algn="l"><a:defRPr sz="4000" b="0"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle>
<p:bodyStyle><a:lvl1pPr marL="228600" indent="-228600"><a:spcBef><a:spcPts val="1000"/></a:spcBef><a:buFont typeface="Arial"/><a:buChar char="•"/><a:defRPr sz="2800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr><a:lvl2pPr marL="685800" indent="-228600"><a:spcBef><a:spcPts val="500"/></a:spcBef><a:buFont typeface="Arial"/><a:buChar char="–"/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl2pPr></p:bodyStyle>
<p:otherStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></a:defRPr></a:lvl1pPr></p:otherStyle></p:txStyles></p:sldMaster>"#,
        title = sp(
            2,
            "Title",
            r#"<p:ph type="title"/>"#,
            &xfrm(60 * E, 30 * E, 840 * E, 90 * E, ""),
            "",
            r#"<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:r><a:t>Title</a:t></a:r></a:p></p:txBody>"#
        ),
        body = sp(
            3,
            "Body",
            r#"<p:ph type="body" idx="1"/>"#,
            &xfrm(60 * E, 130 * E, 840 * E, 340 * E, ""),
            "",
            r#"<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Body</a:t></a:r></a:p></p:txBody>"#
        ),
        band = sp(
            4,
            "Band",
            "",
            &format!(
                r#"{}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill><a:ln><a:noFill/></a:ln>"#,
                xfrm(0, 520 * E, 960 * E, 20 * E, "")
            ),
            "",
            ""
        ),
    );
    let layout = format!(
        r#"<p:sldLayout {NS} type="obj"><p:cSld name="Title and Content"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{}{}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#,
        sp(2, "Title 1", r#"<p:ph type="title"/>"#, "", "", ""),
        sp(3, "Content 2", r#"<p:ph idx="1"/>"#, "", "", "")
    );
    let style_ref = r#"<p:style><a:lnRef idx="2"><a:schemeClr val="accent1"><a:shade val="50000"/></a:schemeClr></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent2"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="lt1"/></a:fontRef></p:style>"#;
    let slide1 = format!(
        r#"<p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
{title}{body}{badge}
<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="6" name="Arrow"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr>{arrow_x}<a:prstGeom prst="straightConnector1"><a:avLst/></a:prstGeom><a:ln w="28575"><a:solidFill><a:srgbClr val="C00000"/></a:solidFill><a:tailEnd type="triangle"/></a:ln></p:spPr></p:cxnSp>
<p:grpSp><p:nvGrpSpPr><p:cNvPr id="7" name="Group"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="{gx}" y="{gy}"/><a:ext cx="{gw}" cy="{gh}"/><a:chOff x="0" y="0"/><a:chExt cx="{cw}" cy="{ch}"/></a:xfrm></p:grpSpPr>{dot1}{dot2}</p:grpSp>
<p:pic><p:nvPicPr><p:cNvPr id="10" name="Picture"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rIdImg"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr>{pic_x}<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>
</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#,
        title = sp(
            2,
            "Title 1",
            r#"<p:ph type="title"/>"#,
            "",
            "",
            &format!(
                r#"<p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody>"#,
                para("Quarterly results", "")
            )
        ),
        body = sp(
            3,
            "Content 2",
            r#"<p:ph idx="1"/>"#,
            &xfrm(60 * E, 130 * E, 480 * E, 340 * E, ""),
            "",
            &format!(
                r#"<p:txBody><a:bodyPr><a:normAutofit/></a:bodyPr><a:lstStyle/>{}{}{}<a:p><a:r><a:rPr lang="en-US"><a:hlinkClick r:id="rIdLink"/></a:rPr><a:t>Details online</a:t></a:r></a:p></p:txBody>"#,
                para("Revenue up 12 %", ""),
                para("Enterprise led the growth", r#"<a:pPr lvl="1"/>"#),
                para("Costs flat", "")
            )
        ),
        badge = sp(
            5,
            "Badge",
            "",
            &format!(
                r#"{}<a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom>"#,
                xfrm(620 * E, 140 * E, 220 * E, 110 * E, r#" rot="1800000""#)
            ),
            style_ref,
            r#"<p:txBody><a:bodyPr rtlCol="0" anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" sz="2400" b="1"/><a:t>NEW</a:t></a:r></a:p></p:txBody>"#
        ),
        arrow_x = xfrm(560 * E, 400 * E, 200 * E, 60 * E, ""),
        gx = 800 * E,
        gy = 380 * E,
        gw = 120 * E,
        gh = 60 * E,
        cw = 240 * E,
        ch = 120 * E,
        dot1 = sp(
            8,
            "Dot 1",
            "",
            &format!(
                r#"{}<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent6"/></a:solidFill>"#,
                xfrm(0, 0, 120 * E, 120 * E, "")
            ),
            "",
            ""
        ),
        dot2 = sp(
            9,
            "Dot 2",
            "",
            &format!(
                r#"{}<a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent4"/></a:solidFill>"#,
                xfrm(120 * E, 0, 120 * E, 120 * E, "")
            ),
            "",
            ""
        ),
        pic_x = xfrm(620 * E, 280 * E, 100 * E, 80 * E, ""),
    );
    let cell = |t: &str| {
        format!(
            r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1800"/><a:t>{t}</a:t></a:r></a:p></a:txBody><a:tcPr/></a:tc>"#
        )
    };
    let row = |a: &str, b: &str| format!(r#"<a:tr h="{}">{}{}</a:tr>"#, 40 * E, cell(a), cell(b));
    let slide2 = format!(
        r#"<p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
{title}
<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="4" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="{tx}" y="{ty}"/><a:ext cx="{tw}" cy="{th}"/></p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table"><a:tbl><a:tblPr firstRow="1" bandRow="1"><a:tableStyleId>{{TEST-STYLE}}</a:tableStyleId></a:tblPr><a:tblGrid><a:gridCol w="{c}"/><a:gridCol w="{c}"/></a:tblGrid>{r1}{r2}{r3}</a:tbl></a:graphicData></a:graphic></p:graphicFrame>
</p:spTree></p:cSld></p:sld>"#,
        title = sp(
            2,
            "Title 1",
            r#"<p:ph type="title"/>"#,
            "",
            "",
            &format!(
                r#"<p:txBody><a:bodyPr/><a:lstStyle/>{}</p:txBody>"#,
                para("By region", "")
            )
        ),
        tx = 60 * E,
        ty = 140 * E,
        tw = 600 * E,
        th = 120 * E,
        c = 300 * E,
        r1 = row("Region", "Sales"),
        r2 = row("North", "1,240"),
        r3 = row("South", "980"),
    );
    let hidden = slide2.replace("<p:sld ", r#"<p:sld show="0" "#);
    let table_styles = format!(
        r#"<a:tblStyleLst {NS} def="{{TEST-STYLE}}"><a:tblStyle styleId="{{TEST-STYLE}}" styleName="Test"><a:wholeTbl><a:tcTxStyle><a:fontRef idx="minor"/><a:schemeClr val="dk1"/></a:tcTxStyle><a:tcStyle><a:tcBdr><a:insideH><a:ln w="12700"><a:solidFill><a:schemeClr val="lt1"/></a:solidFill></a:ln></a:insideH></a:tcBdr><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="40000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:wholeTbl><a:band1H><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"><a:tint val="20000"/></a:schemeClr></a:solidFill></a:fill></a:tcStyle></a:band1H><a:firstRow><a:tcTxStyle b="on"><a:fontRef idx="minor"/><a:schemeClr val="lt1"/></a:tcTxStyle><a:tcStyle><a:fill><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></a:fill></a:tcStyle></a:firstRow></a:tblStyle></a:tblStyleLst>"#
    );
    let presentation = format!(
        r#"<p:presentation {NS}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst><p:sldIdLst><p:sldId id="256" r:id="rId2"/><p:sldId id="257" r:id="rId6"/><p:sldId id="258" r:id="rId3"/></p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:defaultTextStyle><a:lvl1pPr><a:defRPr sz="1800"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#
    );
    let mut png = Vec::new();
    image::RgbImage::from_fn(40, 30, |x, _| image::Rgb([(x * 6) as u8, 160, 60]))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let ct = |part: &str, kind: &str| {
        format!(
            r#"<Override PartName="/{part}" ContentType="application/vnd.openxmlformats-officedocument.{kind}"/>"#
        )
    };
    let types = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/>{}{}{}{}{}{}{}{}<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#,
        ct(
            "ppt/presentation.xml",
            "presentationml.presentation.main+xml"
        ),
        ct(
            "ppt/slideMasters/slideMaster1.xml",
            "presentationml.slideMaster+xml"
        ),
        ct(
            "ppt/slideLayouts/slideLayout1.xml",
            "presentationml.slideLayout+xml"
        ),
        ct("ppt/slides/slide1.xml", "presentationml.slide+xml"),
        ct("ppt/slides/slide2.xml", "presentationml.slide+xml"),
        ct("ppt/slides/slide3.xml", "presentationml.slide+xml"),
        ct("ppt/theme/theme1.xml", "theme+xml"),
        ct("ppt/tableStyles.xml", "presentationml.tableStyles+xml"),
    );
    let parts: Vec<(&str, Vec<u8>)> = vec![
        ("[Content_Types].xml", types.into_bytes()),
        ("_rels/.rels", rels(&[("rId1", "officeDocument", "ppt/presentation.xml")]).into_bytes()),
        ("ppt/presentation.xml", presentation.into_bytes()),
        (
            "ppt/_rels/presentation.xml.rels",
            rels(&[
                ("rId1", "slideMaster", "slideMasters/slideMaster1.xml"),
                ("rId2", "slide", "slides/slide1.xml"),
                ("rId3", "slide", "slides/slide2.xml"),
                ("rId6", "slide", "slides/slide3.xml"),
                ("rId4", "theme", "theme/theme1.xml"),
                ("rId5", "tableStyles", "tableStyles.xml"),
            ])
            .into_bytes(),
        ),
        ("ppt/slideMasters/slideMaster1.xml", master.into_bytes()),
        (
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            rels(&[
                ("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml"),
                ("rId2", "theme", "../theme/theme1.xml"),
            ])
            .into_bytes(),
        ),
        ("ppt/slideLayouts/slideLayout1.xml", layout.into_bytes()),
        (
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            rels(&[("rId1", "slideMaster", "../slideMasters/slideMaster1.xml")]).into_bytes(),
        ),
        ("ppt/theme/theme1.xml", theme().into_bytes()),
        ("ppt/tableStyles.xml", table_styles.into_bytes()),
        ("ppt/slides/slide1.xml", slide1.into_bytes()),
        ("ppt/slides/slide2.xml", slide2.into_bytes()),
        ("ppt/slides/slide3.xml", hidden.into_bytes()),
        (
            "ppt/slides/_rels/slide1.xml.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/><Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/><Relationship Id="rIdLink" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/" TargetMode="External"/></Relationships>"#.to_string()
            .into_bytes(),
        ),
        (
            "ppt/slides/_rels/slide2.xml.rels",
            rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]).into_bytes(),
        ),
        (
            "ppt/slides/_rels/slide3.xml.rels",
            rels(&[("rId1", "slideLayout", "../slideLayouts/slideLayout1.xml")]).into_bytes(),
        ),
        ("ppt/media/image1.png", png),
        (
            "docProps/core.xml",
            br#"<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Quarterly deck</dc:title><dc:creator>Test</dc:creator></cp:coreProperties>"#.to_vec(),
        ),
    ];
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o = zip::write::SimpleFileOptions::default();
        for (name, body) in parts {
            z.start_file(name, o).unwrap();
            z.write_all(&body).unwrap();
        }
        z.finish().unwrap();
    }
    buf
}

#[test]
fn reads_slides_through_layout_and_master() {
    let deck = read::read(&sample_pptx()).unwrap();
    assert_eq!(deck.slides.len(), 2, "the hidden slide is skipped");
    assert_eq!((deck.w, deck.h), (960.0, 540.0));
    assert_eq!(deck.meta.title.as_deref(), Some("Quarterly deck"));
    let s1 = &deck.slides[0];
    assert_eq!(s1.title.as_deref(), Some("Quarterly results"));
    // Master band, then title, body, badge, arrow, two dots, picture.
    assert_eq!(s1.items.len(), 8, "{}", s1.items.len());
    // The title inherits its box from the master through the layout.
    let title = &s1.items[1];
    assert_eq!((title.at.x, title.at.y, title.at.w), (60.0, 30.0, 840.0));
    // Body text: bullets from the master's body style, level 2 smaller.
    let Kind::Shape { text: Some(t), .. } = &s1.items[2].kind else {
        panic!("body has text");
    };
    let paras: Vec<&crate::docxlayout::model::Para> = t
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Para(p) => Some(&**p),
            _ => None,
        })
        .collect();
    assert_eq!(paras[0].label.as_ref().map(|l| l.text.as_str()), Some("•"));
    assert_eq!(paras[1].label.as_ref().map(|l| l.text.as_str()), Some("–"));
    let size = |p: &crate::docxlayout::model::Para| match &p.items[0] {
        crate::docxlayout::model::Item::Text { rs, .. } => rs.sz,
        _ => 0.0,
    };
    assert_eq!((size(paras[0]), size(paras[1])), (28.0, 24.0));
    assert_eq!(paras[1].p.left, 54.0);
    // The badge: style fill (accent2), white text from the style's font colour, rotated.
    let badge = &s1.items[3];
    assert!((badge.at.rot - 30.0).abs() < 0.01);
    let Kind::Shape {
        fill: Some(Paint::Solid(c, _)),
        text: Some(bt),
        pen: Some(pen),
        ..
    } = &badge.kind
    else {
        panic!("badge");
    };
    assert_eq!(*c, crate::docxlayout::props::Rgb(0xED, 0x7D, 0x31));
    assert_eq!(pen.width, 1.0);
    let Block::Para(bp) = &bt.blocks[0] else {
        panic!()
    };
    let crate::docxlayout::model::Item::Text { rs, .. } = &bp.items[0] else {
        panic!()
    };
    assert_eq!(rs.color, crate::docxlayout::props::Rgb(255, 255, 255));
    // The group's children are scaled into place.
    let dot2 = &s1.items[6];
    assert_eq!((dot2.at.x, dot2.at.y, dot2.at.w), (860.0, 380.0, 60.0));
    // Slide 2 has the table.
    assert!(matches!(
        deck.slides[1].items.last().unwrap().kind,
        Kind::Table(_)
    ));
}

#[test]
fn converts_to_pdf_pages() {
    if !Fonts::available() {
        eprintln!("SKIPPED: no fonts");
        return;
    }
    let bytes = sample_pptx();
    if let Some(p) = std::env::var_os("PPTX_SAMPLE_OUT") {
        std::fs::write(p, &bytes).unwrap();
    }
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("deck.pdf");
    assert_eq!(
        convert(&bytes, "deck", &out, &CancelToken::new()).unwrap(),
        2
    );
    let pdf = std::fs::read(&out).unwrap();
    let l = crate::pdflayout::analyze(&pdf, &CancelToken::new()).unwrap();
    assert_eq!(l.pages.len(), 2);
    assert!((l.pages[0].w - 960.0).abs() < 0.5);
    let text = format!("{l:?}");
    for needle in [
        "Quarterly results",
        "Revenue up 12",
        "Enterprise",
        "Region",
        "1,240",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    // Rendered: the accent band along the bottom, the orange badge, the green dot.
    let r = crate::pdf_tools::PageRenderer::open(&out).unwrap();
    let img = r.session(|s| s.render_dpi(0, 36.0)).unwrap();
    if let Some(p) = std::env::var_os("PPTX_PNG_OUT") {
        img.save(p).unwrap();
    }
    let px = |fx: f32, fy: f32| {
        *img.get_pixel(
            (img.width() as f32 * fx) as u32,
            (img.height() as f32 * fy) as u32,
        )
    };
    let band = px(0.5, 0.98);
    assert!(band[2] > 150 && band[0] < 120, "accent1 band {band:?}");
    let dot = px(0.865, 0.76);
    assert!(
        dot[1] > 140 && dot[0] < 160 && dot[2] < 120,
        "green dot {dot:?}"
    );
}
