//! PDF → Excel: the tables the layout analysis found become real cells (numbers as
//! numbers, merged cells merged), one sheet per page with the page's other text in reading
//! order around them. When tables continue over several pages (statements, reports), an
//! "All tables" sheet first joins them into one, dropping repeated header rows.

use std::path::Path;

use rust_xlsxwriter::{Format, Workbook, Worksheet};
use swirvo_core::EngineError;

use super::model::{Block, Layout, Para, Table, VMerge};

/// A cell's text: its paragraphs on separate lines.
fn cell_text(paras: &[Para]) -> String {
    paras
        .iter()
        .map(|p| p.text().trim().to_string())
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A number as printed (`1,234.50`, `(12)`, `-3%`, `$ 1,200`, `1.234,56`) and an Excel
/// number format that prints it the same way. Text that only looks numeric (leading
/// zeros, long ids, dates, phone numbers) stays text.
pub(crate) fn parse_number(s: &str) -> Option<(f64, String)> {
    let t = s.trim();
    if t.is_empty() || t.len() > 32 {
        return None;
    }
    let mut body = t;
    let mut neg = false;
    let mut paren = false;
    if body.starts_with('(') && body.ends_with(')') {
        body = &body[1..body.len() - 1];
        neg = true;
        paren = true;
    }
    let mut prefix = String::new();
    let mut suffix = String::new();
    // A sign, then a currency symbol (either order).
    for _ in 0..2 {
        if let Some(r) = body.strip_prefix(['-', '\u{2212}']) {
            neg = true;
            body = r.trim_start();
        }
        if let Some(c) = body.chars().next()
            && "$€£¥₹".contains(c)
        {
            prefix = c.to_string();
            body = body[c.len_utf8()..].trim_start();
        }
    }
    if let Some(r) = body.strip_suffix('%') {
        suffix = "%".into();
        body = r.trim_end();
    } else if let Some(c) = body.chars().last()
        && "€£".contains(c)
    {
        suffix = format!(" {c}");
        body = body[..body.len() - c.len_utf8()].trim_end();
    }
    if body.is_empty()
        || !body
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '.')
    {
        return None;
    }
    if !body.starts_with(|c: char| c.is_ascii_digit())
        || !body.ends_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    // Which mark is the decimal point: the last one, if it isn't a group of three.
    let last_dot = body.rfind('.');
    let last_comma = body.rfind(',');
    let (dec, group) = match (last_dot, last_comma) {
        (Some(d), Some(c)) if d > c => ('.', ','),
        (Some(_), Some(_)) => (',', '.'),
        // One dot is a decimal point (1.5, 1.234); several are groups (1.234.567).
        (Some(_), None) => {
            if body.matches('.').count() > 1 {
                ('\0', '.')
            } else {
                ('.', ',')
            }
        }
        // One comma before three digits groups (1,234); otherwise it's decimal (12,5).
        (None, Some(c)) => {
            if body.matches(',').count() > 1 || body.len() - c - 1 == 3 {
                ('\0', ',')
            } else {
                (',', '.')
            }
        }
        (None, None) => ('\0', ','),
    };
    let (int, frac) = match body.rfind(dec).filter(|_| dec != '\0') {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => (body, ""),
    };
    let grouped = int.contains(group);
    if grouped {
        // Groups of exactly three after the first.
        let parts: Vec<&str> = int.split(group).collect();
        if parts[0].is_empty() || parts[0].len() > 3 || parts[1..].iter().any(|p| p.len() != 3) {
            return None;
        }
    }
    let digits: String = int.chars().filter(char::is_ascii_digit).collect();
    if digits.len() > 15 || (digits.len() > 1 && digits.starts_with('0') && !grouped) {
        return None;
    }
    let mut v: f64 = format!("{digits}.{}", if frac.is_empty() { "0" } else { frac })
        .parse()
        .ok()?;
    if neg {
        v = -v;
    }
    let mut num = if grouped {
        "#,##0".to_string()
    } else {
        "0".to_string()
    };
    if !frac.is_empty() {
        num.push('.');
        num.push_str(&"0".repeat(frac.len().min(10)));
    }
    if suffix == "%" {
        v /= 100.0;
    }
    let pos = format!(
        "{}{num}{}",
        if prefix.is_empty() {
            String::new()
        } else {
            format!("\"{prefix}\"")
        },
        match suffix.as_str() {
            "%" => "%".to_string(),
            "" => String::new(),
            s => format!("\"{s}\""),
        }
    );
    let fmt = if paren { format!("{pos};({pos})") } else { pos };
    Some((v, fmt))
}

struct Writer {
    plain: Format,
    bold: Format,
    wrap: Format,
    head: Format,
    widths: Vec<f64>,
}

impl Writer {
    fn cell(
        &self,
        ws: &mut Worksheet,
        r: u32,
        c: u16,
        text: &str,
        header: bool,
    ) -> Result<(), EngineError> {
        let enc = EngineError::encode;
        if let Some((v, f)) = parse_number(text) {
            let mut fmt = Format::new().set_num_format(f);
            if header {
                fmt = fmt.set_bold();
            }
            ws.write_number_with_format(r, c, v, &fmt).map_err(enc)?;
        } else if !text.is_empty() {
            let fmt = match (header, text.contains('\n')) {
                (true, _) => &self.head,
                (false, true) => &self.wrap,
                (false, false) => &self.plain,
            };
            ws.write_string_with_format(r, c, text, fmt).map_err(enc)?;
        }
        Ok(())
    }

    /// Write `t` from row `r` (skipping its first `skip` rows); returns the next free row.
    fn table(
        &mut self,
        ws: &mut Worksheet,
        t: &Table,
        mut r: u32,
        skip: usize,
    ) -> Result<u32, EngineError> {
        let enc = EngineError::encode;
        for (k, w) in t.cols.iter().enumerate() {
            let chars = (*w as f64 / 5.6).clamp(4.0, 80.0);
            if self.widths.len() <= k {
                self.widths.resize(k + 1, 0.0);
            }
            self.widths[k] = self.widths[k].max(chars);
        }
        // Grid column of each cell, and where vertical merges start.
        let grid: Vec<Vec<usize>> = t
            .rows
            .iter()
            .map(|row| {
                let mut c = 0;
                row.cells
                    .iter()
                    .map(|cell| {
                        let at = c;
                        c += cell.span.max(1);
                        at
                    })
                    .collect()
            })
            .collect();
        for (ri, row) in t.rows.iter().enumerate().skip(skip) {
            let header = ri == 0 && row.cells.iter().all(|c| c.paras.iter().all(|p| p.bold));
            for (ci, cell) in row.cells.iter().enumerate() {
                if cell.vmerge == VMerge::Continue {
                    continue;
                }
                let col = grid[ri][ci] as u16;
                let text = cell_text(&cell.paras);
                let span = cell.span.max(1) as u16;
                // Rows below that continue this cell.
                let down = if cell.vmerge == VMerge::Start {
                    (ri + 1..t.rows.len())
                        .take_while(|&rr| {
                            grid[rr]
                                .iter()
                                .position(|&g| g == grid[ri][ci])
                                .is_some_and(|k| t.rows[rr].cells[k].vmerge == VMerge::Continue)
                        })
                        .count() as u32
                } else {
                    0
                };
                if span > 1 || down > 0 {
                    let fmt = if header { &self.head } else { &self.wrap };
                    ws.merge_range(r, col, r + down, col + span - 1, "", fmt)
                        .map_err(enc)?;
                }
                self.cell(ws, r, col, &text, header)?;
            }
            r += 1;
        }
        Ok(r)
    }
}

/// Are these the same header row (a table continued on the next page)?
fn same_header(a: &Table, b: &Table) -> bool {
    let row = |t: &Table| {
        t.rows.first().map(|r| {
            r.cells
                .iter()
                .map(|c| cell_text(&c.paras))
                .collect::<Vec<_>>()
        })
    };
    a.cols.len() == b.cols.len() && row(a).is_some() && row(a) == row(b)
}

/// Write the layout as an Excel workbook. Fails when the PDF has no tables and no text.
pub fn write(l: &Layout, out: &Path) -> Result<(), EngineError> {
    let enc = EngineError::encode;
    let mut wb = Workbook::new();
    let mut w = Writer {
        plain: Format::new(),
        bold: Format::new().set_bold(),
        wrap: Format::new().set_text_wrap(),
        head: Format::new().set_bold().set_text_wrap(),
        widths: Vec::new(),
    };
    let tables: Vec<&Table> = l
        .pages
        .iter()
        .flat_map(|p| p.bands.iter())
        .flat_map(|b| b.cols.iter())
        .flat_map(|c| c.blocks.iter())
        .filter_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .collect();
    let pages_with_tables = l
        .pages
        .iter()
        .filter(|p| {
            p.bands
                .iter()
                .flat_map(|b| b.cols.iter())
                .any(|c| c.blocks.iter().any(|b| matches!(b, Block::Table(_))))
        })
        .count();
    if pages_with_tables >= 2 {
        let ws = wb.add_worksheet();
        ws.set_name("All tables").map_err(enc)?;
        let mut r = 0;
        let mut prev: Option<&Table> = None;
        for t in &tables {
            let cont = prev.is_some_and(|p| p.cols.len() == t.cols.len());
            let skip = usize::from(cont && prev.is_some_and(|p| same_header(p, t)));
            if prev.is_some() && !cont {
                r += 1;
            }
            r = w.table(ws, t, r, skip)?;
            prev = Some(t);
        }
        for (k, cw) in w.widths.iter().enumerate() {
            ws.set_column_width(k as u16, *cw).map_err(enc)?;
        }
        ws.set_freeze_panes(1, 0).map_err(enc)?;
    }
    let mut wrote_any = !tables.is_empty();
    for (pi, page) in l.pages.iter().enumerate() {
        let ws = wb.add_worksheet();
        ws.set_name(format!("Page {}", pi + 1)).map_err(enc)?;
        w.widths.clear();
        let mut r: u32 = 0;
        let mut last_table = false;
        for band in &page.bands {
            for col in &band.cols {
                for b in &col.blocks {
                    match b {
                        Block::Para(p) => {
                            let t = p.text();
                            let t = t.trim();
                            if t.is_empty() {
                                continue;
                            }
                            if last_table {
                                r += 1;
                            }
                            // Tab-separated text (columns laid out with tabs) gets a cell each.
                            for (k, part) in t.split('\t').enumerate().take(200) {
                                if p.bold || p.heading.is_some() {
                                    ws.write_string_with_format(r, k as u16, part.trim(), &w.bold)
                                        .map_err(enc)?;
                                } else {
                                    w.cell(ws, r, k as u16, part.trim(), false)?;
                                }
                            }
                            r += 1;
                            wrote_any = true;
                            last_table = false;
                        }
                        Block::Table(t) => {
                            if r > 0 {
                                r += 1;
                            }
                            r = w.table(ws, t, r, 0)?;
                            last_table = true;
                        }
                        Block::Picture(_) => {}
                    }
                }
            }
        }
        for (k, cw) in w.widths.iter().enumerate() {
            ws.set_column_width(k as u16, *cw).map_err(enc)?;
        }
    }
    if !wrote_any {
        return Err(EngineError::Invalid(
            "no text or tables were found in the PDF".into(),
        ));
    }
    wb.save(out).map_err(enc)
}

#[cfg(test)]
mod tests {
    use super::parse_number;

    #[test]
    fn numbers_keep_their_look() {
        let p = |s: &str| parse_number(s);
        assert_eq!(p("1,234.50"), Some((1234.5, "#,##0.00".into())));
        assert_eq!(p("(1,200)"), Some((-1200.0, "#,##0;(#,##0)".into())));
        assert_eq!(p("-3.5%"), Some((-0.035, "0.0%".into())));
        assert_eq!(p("$ 1,200.00"), Some((1200.0, "\"$\"#,##0.00".into())));
        assert_eq!(p("1.234,56"), Some((1234.56, "#,##0.00".into())));
        assert_eq!(p("12,5"), Some((12.5, "0.0".into())));
        assert_eq!(p("42"), Some((42.0, "0".into())));
        for text in [
            "007",
            "2024-01-15",
            "555-1234",
            "12/03/2024",
            "1234567890123456",
            "abc",
            "1,23,4",
        ] {
            assert_eq!(p(text), None, "{text}");
        }
    }

    #[test]
    fn a_ruled_table_becomes_cells() {
        use crate::pdfedit::stamp::{self, HAlign, Item};
        use swirvo_core::CancelToken;
        if !crate::docxlayout::fonts::Fonts::available() {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("in.pdf");
        crate::pdfedit::testing::sample(&src, 1, false);
        let cells = [
            ["Item", "Qty", "Price"],
            ["Apples", "3", "1,200.50"],
            ["Pears", "10", "(15.00)"],
        ];
        // Well below the sample's black square, which would join the grid.
        let (x0, top, cw, rh) = (72.0f32, 500.0f32, 120.0f32, 24.0f32);
        let mut items = Vec::new();
        for r in 0..=3 {
            let y = top - r as f32 * rh;
            items.push(Item::Rect {
                rect: [x0, y - 0.5, x0 + 3.0 * cw, y + 0.5],
                color: [0, 0, 0],
                opacity: 1.0,
            });
        }
        for c in 0..=3 {
            let x = x0 + c as f32 * cw;
            items.push(Item::Rect {
                rect: [x - 0.5, top - 3.0 * rh, x + 0.5, top],
                color: [0, 0, 0],
                opacity: 1.0,
            });
        }
        for (r, row) in cells.iter().enumerate() {
            for (c, t) in row.iter().enumerate() {
                items.push(Item::Text {
                    text: t.to_string(),
                    size: 11.0,
                    color: [0, 0, 0],
                    opacity: 1.0,
                    x: x0 + c as f32 * cw + 6.0,
                    y: top - (r as f32 + 1.0) * rh + 8.0,
                    align: HAlign::Left,
                    angle: 0.0,
                    bold: r == 0,
                });
            }
        }
        let pdf = d.path().join("table.pdf");
        stamp::stamp(&src, &pdf, None, &CancelToken::new(), |_| items.clone()).unwrap();
        let l =
            crate::pdflayout::analyze(&std::fs::read(&pdf).unwrap(), &CancelToken::new()).unwrap();
        let out = d.path().join("out.xlsx");
        super::write(&l, &out).unwrap();
        let rows = crate::sheet::read_sheet(&out).unwrap();
        let text: Vec<Vec<String>> = rows
            .iter()
            .map(|r| r.iter().map(crate::sheet::cell_text).collect())
            .collect();
        let find = |s: &str| text.iter().position(|r| r.iter().any(|c| c == s));
        let apples = find("Apples").unwrap_or_else(|| panic!("{text:?}"));
        assert_eq!(text[apples].len(), 3, "three columns: {text:?}");
        assert_eq!(text[apples][1], "3", "{text:?}");
        assert_eq!(text[apples][2], "1200.5", "a real number: {text:?}");
        let pears = find("Pears").unwrap();
        assert_eq!(text[pears][2], "-15", "{text:?}");
    }
}
