use std::io::Write as _;

use super::*;

const MAIN: &str = r#"xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

fn rels(items: &[(&str, &str, &str)]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    );
    for (id, kind, target) in items {
        s.push_str(&format!(
            r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/{kind}" Target="{target}"/>"#
        ));
    }
    s + "</Relationships>"
}

/// A two-page sales sheet (A4, repeated header row, footer with page numbers) and a
/// hidden sheet.
pub(crate) fn sample_xlsx() -> Vec<u8> {
    let strings = [
        "Quarterly sales",
        "Region",
        "Date",
        "Amount",
        "Share",
        "North",
        "South",
        "A long note that wraps inside its cell instead of running on",
        "This label overflows into the empty cells to its right",
    ];
    let sst = format!(
        r#"<sst {MAIN} count="{n}" uniqueCount="{n}">{}</sst>"#,
        strings
            .iter()
            .map(|s| format!("<si><t>{s}</t></si>"))
            .collect::<String>(),
        n = strings.len()
    );
    let styles = format!(
        r##"<styleSheet {MAIN}><numFmts count="2"><numFmt numFmtId="164" formatCode="#,##0.00"/><numFmt numFmtId="165" formatCode="d-mmm-yy"/></numFmts>
<fonts count="3"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="16"/><color theme="4"/><name val="Calibri"/></font><font><b/><sz val="11"/><color rgb="FFFFFFFF"/><name val="Calibri"/></font></fonts>
<fills count="4"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill><fill><patternFill patternType="solid"><fgColor rgb="FF4472C4"/><bgColor indexed="64"/></patternFill></fill><fill><patternFill patternType="solid"><fgColor theme="4" tint="0.79998168889431442"/></patternFill></fill></fills>
<borders count="2"><border><left/><right/><top/><bottom/><diagonal/></border><border><left style="thin"><color indexed="64"/></left><right style="thin"><color indexed="64"/></right><top style="thin"><color indexed="64"/></top><bottom style="thin"><color indexed="64"/></bottom><diagonal/></border></borders>
<cellXfs count="8"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" applyFont="1"><alignment horizontal="center"/></xf><xf numFmtId="0" fontId="2" fillId="2" borderId="1"><alignment horizontal="center"/></xf><xf numFmtId="164" fontId="0" fillId="0" borderId="1"/><xf numFmtId="165" fontId="0" fillId="0" borderId="1"/><xf numFmtId="0" fontId="0" fillId="3" borderId="1"/><xf numFmtId="0" fontId="0" fillId="0" borderId="1"><alignment vertical="top" wrapText="1"/></xf><xf numFmtId="10" fontId="0" fillId="0" borderId="1"/></cellXfs></styleSheet>"##
    );
    let mut rows = String::new();
    rows.push_str(
        r#"<row r="1" ht="24" customHeight="1"><c r="A1" s="1" t="s"><v>0</v></c></row>"#,
    );
    rows.push_str(r#"<row r="3"><c r="A3" s="2" t="s"><v>1</v></c><c r="B3" s="2" t="s"><v>2</v></c><c r="C3" s="2" t="s"><v>3</v></c><c r="D3" s="2" t="s"><v>4</v></c></row>"#);
    for i in 0..60 {
        let r = 4 + i;
        let region = if i % 2 == 0 { 5 } else { 6 };
        let amount = 1234.5 + i as f64 * 101.25;
        let share = 0.0125 * (i % 9) as f64;
        rows.push_str(&format!(
            r#"<row r="{r}"><c r="A{r}" s="5" t="s"><v>{region}</v></c><c r="B{r}" s="4"><v>{}</v></c><c r="C{r}" s="3"><v>{amount}</v></c><c r="D{r}" s="7"><v>{share}</v></c></row>"#,
            45_000 + i
        ));
    }
    rows.push_str(
        r#"<row r="65" ht="45" customHeight="1"><c r="A65" s="6" t="s"><v>7</v></c></row>"#,
    );
    rows.push_str(r#"<row r="66"><c r="A66" t="s"><v>8</v></c></row>"#);
    rows.push_str(r#"<row r="67"><c r="C67" s="3"><v>99999999999</v></c></row>"#);
    let sheet1 = format!(
        r#"<worksheet {MAIN}><sheetPr><pageSetUpPr fitToPage="0"/></sheetPr><dimension ref="A1:D67"/><sheetFormatPr defaultRowHeight="15"/><cols><col min="1" max="1" width="18.7109375" customWidth="1"/><col min="2" max="2" width="12.7109375" customWidth="1"/><col min="3" max="3" width="9" customWidth="1"/><col min="4" max="4" width="10.7109375" customWidth="1"/></cols><sheetData>{rows}</sheetData><mergeCells count="1"><mergeCell ref="A1:D1"/></mergeCells><printOptions horizontalCentered="1"/><pageMargins left="0.7" right="0.7" top="0.75" bottom="0.75" header="0.3" footer="0.3"/><pageSetup paperSize="9" orientation="portrait"/><headerFooter><oddFooter>&amp;CPage &amp;P of &amp;N</oddFooter></headerFooter></worksheet>"#
    );
    let sheet2 = format!(
        r#"<worksheet {MAIN}><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>secret</t></is></c></row></sheetData></worksheet>"#
    );
    let workbook = format!(
        r#"<workbook {MAIN}><sheets><sheet name="Sales" sheetId="1" r:id="rId1"/><sheet name="Hidden" sheetId="2" state="hidden" r:id="rId2"/></sheets><definedNames><definedName name="_xlnm.Print_Titles" localSheetId="0">Sales!$3:$3</definedName></definedNames></workbook>"#
    );
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/worksheets/sheet2.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/></Types>"#;
    let parts: Vec<(&str, String)> = vec![
        ("[Content_Types].xml", types.to_string()),
        (
            "_rels/.rels",
            rels(&[("rId1", "officeDocument", "xl/workbook.xml")]),
        ),
        ("xl/workbook.xml", workbook),
        (
            "xl/_rels/workbook.xml.rels",
            rels(&[
                ("rId1", "worksheet", "worksheets/sheet1.xml"),
                ("rId2", "worksheet", "worksheets/sheet2.xml"),
                ("rId3", "styles", "styles.xml"),
                ("rId4", "sharedStrings", "sharedStrings.xml"),
            ]),
        ),
        ("xl/styles.xml", styles),
        ("xl/sharedStrings.xml", sst),
        ("xl/worksheets/sheet1.xml", sheet1),
        ("xl/worksheets/sheet2.xml", sheet2),
    ];
    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o = zip::write::SimpleFileOptions::default();
        for (name, body) in parts {
            z.start_file(name, o).unwrap();
            z.write_all(body.as_bytes()).unwrap();
        }
        z.finish().unwrap();
    }
    buf
}

#[test]
fn reads_the_workbook() {
    let b = read::read(&sample_xlsx()).unwrap();
    assert_eq!(b.sheets.len(), 1, "the hidden sheet isn't printed");
    let s = &b.sheets[0];
    assert_eq!(s.name, "Sales");
    assert_eq!(s.used, Some((0, 0, 66, 3)));
    assert_eq!(s.title_rows, Some((2, 2)));
    assert_eq!(s.merges, vec![[0, 0, 0, 3]]);
    assert_eq!(s.col_w[&0].0, 98.25, "18.71 → 131 px");
    assert_eq!(s.setup.paper, (595.28, 841.89));
    let amount = s.cells.get(&(3, 2)).unwrap();
    assert_eq!(display(&b, amount).0, "1,234.50");
    assert_eq!(display(&b, s.cells.get(&(3, 1)).unwrap()).0, "15-Mar-23");
    assert_eq!(display(&b, s.cells.get(&(4, 3)).unwrap()).0, "1.25%");
}

#[test]
fn prints_pages_with_repeated_titles() {
    if !Fonts::available() {
        eprintln!("SKIPPED: no fonts");
        return;
    }
    let bytes = sample_xlsx();
    if let Some(p) = std::env::var_os("XLSX_SAMPLE_OUT") {
        std::fs::write(p, &bytes).unwrap();
    }
    let d = tempfile::tempdir().unwrap();
    let out = d.path().join("book.pdf");
    let n = convert(&bytes, "book", &out, &CancelToken::new()).unwrap();
    assert_eq!(n, 2, "60 rows at 15 pt run onto a second A4 page");
    if let Some(p) = std::env::var_os("XLSX_PDF_OUT") {
        std::fs::copy(&out, p).unwrap();
    }
    let l = crate::pdflayout::analyze(&std::fs::read(&out).unwrap(), &CancelToken::new()).unwrap();
    assert!((l.pages[0].w - 595.3).abs() < 0.5);
    let all = format!("{l:?}");
    for needle in ["Quarterly sales", "1,234.50", "15-Mar-23", "1.25%", "North"] {
        assert!(all.contains(needle), "missing {needle}");
    }
    // The footer repeats on both pages: our analysis files it as a live page count.
    let footer = format!("{:?}", l.footer);
    assert!(footer.contains("PageCount"), "{footer}");
    // The header row repeats on page 2.
    let p2 = format!("{:?}", l.pages[1]);
    assert!(
        p2.contains("Region") && p2.contains("Amount"),
        "titles repeat on page 2"
    );
    assert!(!all.contains("secret"), "hidden sheets aren't printed");
    // A number too wide for its column shows as ####.
    assert!(all.contains("####"));
    let r = crate::pdf_tools::PageRenderer::open(&out).unwrap();
    let img = r.session(|s| s.render_dpi(0, 50.0)).unwrap();
    if let Some(p) = std::env::var_os("XLSX_PNG_OUT") {
        img.save(p).unwrap();
    }
}
