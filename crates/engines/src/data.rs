//! JSON ⇄ NDJSON ⇄ YAML ⇄ TOML ⇄ CSV ⇄ TSV, hub-and-spoke through `serde_json::Value`
//! (docs/05 §1, §6).
//!
//! CSV in: delimiter sniffing plus per-column type inference over the lattice
//! `null < bool | int < float < string` (bool and int join to string). Codes with leading
//! zeros ("007", ZIP codes) stay strings.

use std::io::{BufWriter, Write};

use serde_json::{Map, Value};
use swirvo_core::{Capability, Engine, EngineError, StepRequest};

use crate::text::read_text;

pub struct DataEngine;

const FORMATS: &[&str] = &["json", "ndjson", "yaml", "toml", "csv", "tsv"];

impl Engine for DataEngine {
    fn id(&self) -> &'static str {
        "data"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in FORMATS {
            for &to in FORMATS {
                if from == to {
                    continue;
                }
                let cap = Capability::new("data", from, to).time(5, 20);
                let flat = matches!(from, "csv" | "tsv");
                caps.push(match to {
                    "csv" | "tsv" if flat => cap.note("delimiter change, quoting preserved"),
                    "csv" | "tsv" => cap.loss(5).note("nested values become JSON text"),
                    "toml" => cap.loss(2).note("TOML has no null; nulls are dropped"),
                    _ => cap,
                });
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let text = read_text(req.input)?;
        let value = read(&text, req.from)?;
        req.cancel.check()?;
        let mut out = BufWriter::new(std::fs::File::create(req.output)?);
        write(&value, req.to, &mut out)?;
        out.flush()?;
        Ok(())
    }
}

pub fn read(text: &str, from: &str) -> Result<Value, EngineError> {
    Ok(match from {
        "json" => serde_json::from_str(text).map_err(EngineError::decode)?,
        "ndjson" => Value::Array(
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(serde_json::from_str)
                .collect::<Result<_, _>>()
                .map_err(EngineError::decode)?,
        ),
        "yaml" => serde_saphyr::from_str_with_options(text, yaml_options())
            .map_err(EngineError::decode)?,
        "toml" => toml::from_str(text).map_err(EngineError::decode)?,
        "csv" => read_delimited(text, sniff_delimiter(text))?,
        "tsv" => read_delimited(text, b'\t')?,
        other => {
            return Err(EngineError::Invalid(format!(
                "data engine can't read {other}"
            )));
        }
    })
}

/// YAML from user files: safe-Rust parser with its default budget (alias bombs) and
/// duplicate-key rejection; YAML 1.2 booleans (`no` stays a string, the "Norway problem");
/// `007` stays the string "007" so ZIP codes and IDs survive.
fn yaml_options() -> serde_saphyr::Options {
    let mut o = serde_saphyr::Options::default();
    o.strict_booleans = true;
    o.leading_zero_numbers_as_strings = true;
    o
}

pub fn write(v: &Value, to: &str, out: &mut impl Write) -> Result<(), EngineError> {
    match to {
        "json" => {
            serde_json::to_writer_pretty(&mut *out, v).map_err(EngineError::encode)?;
            out.write_all(b"\n")?;
        }
        "ndjson" => {
            let items: Vec<&Value> = match v {
                Value::Array(a) => a.iter().collect(),
                other => vec![other],
            };
            for item in items {
                serde_json::to_writer(&mut *out, item).map_err(EngineError::encode)?;
                out.write_all(b"\n")?;
            }
        }
        "yaml" => out.write_all(
            serde_saphyr::to_string(v)
                .map_err(EngineError::encode)?
                .as_bytes(),
        )?,
        "toml" => {
            let mut v = strip_nulls(v.clone());
            if !v.is_object() {
                v = Value::Object(Map::from_iter([("rows".to_string(), v)]));
            }
            out.write_all(
                toml::to_string_pretty(&v)
                    .map_err(EngineError::encode)?
                    .as_bytes(),
            )?;
        }
        "csv" => write_delimited(v, b',', out)?,
        "tsv" => write_delimited(v, b'\t', out)?,
        other => {
            return Err(EngineError::Invalid(format!(
                "data engine can't write {other}"
            )));
        }
    }
    Ok(())
}

fn strip_nulls(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(
            a.into_iter()
                .filter(|v| !v.is_null())
                .map(strip_nulls)
                .collect(),
        ),
        other => other,
    }
}

/// Pick the delimiter whose per-line field count is most consistent (then most frequent)
/// over the first 20 lines, counting only characters outside quotes.
pub fn sniff_delimiter(text: &str) -> u8 {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(20)
        .collect();
    let mut best = (b',', 0.0f64, 0usize);
    for d in *b",;\t|" {
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| {
                let mut quoted = false;
                l.bytes()
                    .filter(|&c| {
                        if c == b'"' {
                            quoted = !quoted;
                        }
                        !quoted && c == d
                    })
                    .count()
            })
            .collect();
        if counts.is_empty() || counts[0] == 0 {
            continue;
        }
        let mode = counts[0];
        let consistency =
            counts.iter().filter(|&&c| c == mode).count() as f64 / counts.len() as f64;
        if consistency > best.1 || (consistency == best.1 && mode > best.2) {
            best = (d, consistency, mode);
        }
    }
    best.0
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Ty {
    Null,
    Bool,
    Int,
    Float,
    Str,
}

fn cell_type(s: &str) -> Ty {
    let t = s.trim();
    if t.is_empty() {
        return Ty::Null;
    }
    if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("false") {
        return Ty::Bool;
    }
    let digits = t.strip_prefix('-').unwrap_or(t);
    let leading_zero = digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.");
    if leading_zero || t.starts_with('+') {
        return Ty::Str;
    }
    if t.parse::<i64>().is_ok() {
        return Ty::Int;
    }
    if t.parse::<f64>().is_ok_and(f64::is_finite) && t.bytes().any(|b| b.is_ascii_digit()) {
        return Ty::Float;
    }
    Ty::Str
}

fn join(a: Ty, b: Ty) -> Ty {
    use Ty::*;
    match (a, b) {
        (Null, x) | (x, Null) => x,
        (Bool, Int | Float) | (Int | Float, Bool) => Str,
        (x, y) => x.max(y),
    }
}

fn typed(s: &str, ty: Ty) -> Value {
    let t = s.trim();
    if t.is_empty() {
        return Value::Null;
    }
    match ty {
        Ty::Bool => Value::Bool(t.eq_ignore_ascii_case("true")),
        Ty::Int => t
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(s.to_string())),
        Ty::Float => t
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(s.to_string())),
        Ty::Null | Ty::Str => Value::String(s.to_string()),
    }
}

fn read_delimited(text: &str, delim: u8) -> Result<Value, EngineError> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .from_reader(text.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(EngineError::decode)?
        .iter()
        .map(str::to_string)
        .collect();
    let rows: Vec<csv::StringRecord> = rdr
        .records()
        .collect::<Result<_, _>>()
        .map_err(EngineError::decode)?;
    let width = rows
        .iter()
        .map(|r| r.len())
        .max()
        .unwrap_or(0)
        .max(headers.len());
    let names: Vec<String> = (0..width)
        .map(|i| {
            headers
                .get(i)
                .filter(|h| !h.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("column{}", i + 1))
        })
        .collect();
    let mut types = vec![Ty::Null; width];
    for r in &rows {
        for (i, cell) in r.iter().enumerate() {
            types[i] = join(types[i], cell_type(cell));
        }
    }
    Ok(Value::Array(
        rows.iter()
            .map(|r| {
                Value::Object(
                    names
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.clone(), typed(r.get(i).unwrap_or(""), types[i])))
                        .collect(),
                )
            })
            .collect(),
    ))
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn write_delimited(v: &Value, delim: u8, out: &mut impl Write) -> Result<(), EngineError> {
    // Accept an array, a single object, or an object wrapping exactly one array.
    let rows: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(m) if m.len() == 1 && m.values().next().is_some_and(Value::is_array) => m
            .values()
            .next()
            .and_then(Value::as_array)
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
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
    let mut w = csv::WriterBuilder::new().delimiter(delim).from_writer(out);
    if header.is_empty() {
        for r in rows {
            match r {
                Value::Array(cells) => w
                    .write_record(cells.iter().map(scalar))
                    .map_err(EngineError::encode)?,
                other => w
                    .write_record([scalar(other)])
                    .map_err(EngineError::encode)?,
            }
        }
    } else {
        w.write_record(&header).map_err(EngineError::encode)?;
        for r in rows {
            let rec: Vec<String> = header
                .iter()
                .map(|k| r.get(*k).map(scalar).unwrap_or_default())
                .collect();
            w.write_record(rec).map_err(EngineError::encode)?;
        }
    }
    w.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip_keeps_lookalike_strings() {
        let src = serde_json::json!([
            {"zip": "007", "flag": "no", "t": "true", "n": 3, "f": 1.5, "nil": null, "s": "a: b", "multi": "x\ny"},
            {"nested": {"list": [1, "2", false]}}
        ]);
        let mut buf = Vec::new();
        write(&src, "yaml", &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        let back = read(&text, "yaml").unwrap();
        assert_eq!(back, src, "YAML was:\n{text}");
        // YAML 1.2 core schema: `no`/`yes` are strings, not booleans (the "Norway problem").
        assert_eq!(
            read("country: no\nok: true\n", "yaml").unwrap(),
            serde_json::json!({"country": "no", "ok": true})
        );
        assert_eq!(
            read("zip: 007\n", "yaml").unwrap(),
            serde_json::json!({"zip": "007"})
        );
        assert!(read("a: [unclosed", "yaml").is_err());
        assert!(
            read("a: 1\na: 2\n", "yaml").is_err(),
            "duplicate keys are ambiguous"
        );
    }

    #[test]
    fn csv_types_and_leading_zeros() {
        let v = read(
            "zip,qty,price,ok,note\n007,3,1.5,true,\n10115,4,2,false,hi\n",
            "csv",
        )
        .unwrap();
        assert_eq!(v[0]["zip"], "007");
        assert_eq!(v[1]["zip"], "10115"); // column joined to string because of "007"
        assert_eq!(v[0]["qty"], 3);
        assert_eq!(v[1]["price"], 2.0);
        assert_eq!(v[0]["ok"], true);
        assert!(v[0]["note"].is_null());
    }

    #[test]
    fn sniffs_semicolons() {
        assert_eq!(sniff_delimiter("a;b;c\n1;2;3\n\"x;y\";2;3\n"), b';');
        assert_eq!(sniff_delimiter("a,b\n1,2\n"), b',');
    }

    #[test]
    fn round_trips() {
        let json = r#"[{"name":"Ada","tags":["x"],"n":1},{"name":"Lin","n":null}]"#;
        let v = read(json, "json").unwrap();
        for to in ["yaml", "toml", "ndjson", "json"] {
            let mut buf = Vec::new();
            write(&v, to, &mut buf).unwrap();
            let back = read(std::str::from_utf8(&buf).unwrap(), to).unwrap();
            let back = if to == "toml" {
                back["rows"].clone()
            } else {
                back
            };
            assert_eq!(back[0]["name"], "Ada", "{to}");
        }
        let mut csv = Vec::new();
        write(&v, "csv", &mut csv).unwrap();
        assert_eq!(
            String::from_utf8(csv).unwrap(),
            "name,tags,n\nAda,\"[\"\"x\"\"]\",1\nLin,,\n"
        );
    }
}
