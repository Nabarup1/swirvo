//! Spreadsheets: XLSX/XLS/ODS → CSV/TSV/JSON (calamine) and CSV/TSV/JSON → XLSX
//! (rust_xlsxwriter). LibreOffice handles layout-faithful PDF export (Office pack).

use std::io::{BufWriter, Write};

use calamine::{Data, Reader, open_workbook_auto};
use rust_xlsxwriter::{Format, Workbook};
use serde_json::{Map, Value};
use swirvo_core::{Capability, Engine, EngineError, StepRequest};

use crate::data;
use crate::text::read_text;

pub struct SheetEngine;

impl Engine for SheetEngine {
    fn id(&self) -> &'static str {
        "sheet"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for from in ["xlsx", "xls", "ods"] {
            for to in ["csv", "tsv", "json"] {
                caps.push(
                    Capability::new("sheet", from, to)
                        .loss(3)
                        .time(30, 60)
                        .note("first non-empty sheet, formulas as values"),
                );
            }
        }
        for from in ["csv", "tsv", "json"] {
            caps.push(
                Capability::new("sheet", from, "xlsx")
                    .time(30, 80)
                    .note("typed cells, bold header, autofit"),
            );
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        if req.to == "xlsx" {
            let v = data::read(&read_text(req.input)?, req.from)?;
            return write_xlsx(&v, req.output);
        }
        let rows = read_sheet(req.input)?;
        let mut out = BufWriter::new(std::fs::File::create(req.output)?);
        match req.to {
            "csv" | "tsv" => {
                let delim = if req.to == "tsv" { b'\t' } else { b',' };
                let mut w = csv::WriterBuilder::new()
                    .delimiter(delim)
                    .flexible(true)
                    .from_writer(&mut out);
                for r in &rows {
                    w.write_record(r.iter().map(cell_text))
                        .map_err(EngineError::encode)?;
                }
                w.flush()?;
            }
            "json" => data::write(&rows_to_objects(&rows), "json", &mut out)?,
            other => {
                return Err(EngineError::Invalid(format!(
                    "sheet engine can't write {other}"
                )));
            }
        }
        out.flush()?;
        Ok(())
    }
}

pub(crate) fn read_sheet(path: &std::path::Path) -> Result<Vec<Vec<Data>>, EngineError> {
    let mut wb = open_workbook_auto(path).map_err(EngineError::decode)?;
    for name in wb.sheet_names() {
        let range = wb.worksheet_range(&name).map_err(EngineError::decode)?;
        if !range.is_empty() {
            return Ok(range.rows().map(<[Data]>::to_vec).collect());
        }
    }
    Ok(Vec::new())
}

pub(crate) fn cell_text(d: &Data) -> String {
    match d {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Float(f) => f.to_string(),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

fn cell_json(d: &Data) -> Value {
    match d {
        Data::Empty => Value::Null,
        Data::Int(i) => Value::from(*i),
        Data::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        Data::Bool(b) => Value::Bool(*b),
        other => Value::String(cell_text(other)),
    }
}

fn rows_to_objects(rows: &[Vec<Data>]) -> Value {
    let Some((head, body)) = rows.split_first() else {
        return Value::Array(Vec::new());
    };
    let names: Vec<String> = head
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let t = cell_text(h);
            if t.is_empty() {
                format!("column{}", i + 1)
            } else {
                t
            }
        })
        .collect();
    Value::Array(
        body.iter()
            .map(|r| {
                Value::Object(
                    names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.clone(), r.get(i).map_or(Value::Null, cell_json)))
                        .collect::<Map<_, _>>(),
                )
            })
            .collect(),
    )
}

fn write_xlsx(v: &Value, path: &std::path::Path) -> Result<(), EngineError> {
    let enc = EngineError::encode;
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    let bold = Format::new().set_bold();
    let rows: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        other => vec![other],
    };
    let mut header: Vec<&str> = Vec::new();
    for r in &rows {
        if let Value::Object(m) = r {
            for k in m.keys() {
                if !header.contains(&k.as_str()) {
                    header.push(k);
                }
            }
        }
    }
    for (c, h) in header.iter().enumerate() {
        ws.write_string_with_format(0, c as u16, *h, &bold)
            .map_err(enc)?;
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, key) in header.iter().enumerate() {
            let (rr, cc) = (r as u32 + 1, c as u16);
            match row.get(*key) {
                None | Some(Value::Null) => {}
                Some(Value::Number(n)) => {
                    ws.write_number(rr, cc, n.as_f64().unwrap_or_default())
                        .map_err(enc)?;
                }
                Some(Value::Bool(b)) => {
                    ws.write_boolean(rr, cc, *b).map_err(enc)?;
                }
                Some(Value::String(s)) => {
                    ws.write_string(rr, cc, s).map_err(enc)?;
                }
                Some(other) => {
                    ws.write_string(rr, cc, other.to_string()).map_err(enc)?;
                }
            }
        }
    }
    ws.autofit();
    wb.save(path).map_err(enc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use swirvo_core::{CancelToken, Preset};

    #[test]
    fn csv_to_xlsx_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let (csv, xlsx, json) = (
            dir.path().join("a.csv"),
            dir.path().join("a.xlsx"),
            dir.path().join("b.json"),
        );
        std::fs::write(&csv, "city,pop\nPune,7.2\nKyoto,1.4\n").unwrap();
        let cancel = CancelToken::new();
        let step = |input: &std::path::Path, output: &std::path::Path, from: &str, to: &str| {
            SheetEngine.run(&StepRequest {
                input,
                output,
                from,
                to,
                preset: Preset::Balanced,
                pdf: Default::default(),
                cancel: &cancel,
            })
        };
        step(&csv, &xlsx, "csv", "xlsx").unwrap();
        step(&xlsx, &json, "xlsx", "json").unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
        assert_eq!(v[0]["city"], "Pune");
        assert_eq!(v[1]["pop"], 1.4);
    }
}
