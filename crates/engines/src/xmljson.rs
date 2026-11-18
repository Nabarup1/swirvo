//! XML ⇄ JSON (docs/12 §8), the common "xmltodict" convention:
//! attributes become `"@name"`, text next to attributes or children becomes `"#text"`,
//! repeated child elements become arrays, an empty element is `null`. Comments and
//! processing instructions are dropped. Element names keep their namespace prefixes.

use quick_xml::events::Event;
use serde_json::{Map, Value};
use swirvo_core::{Capability, Engine, EngineError, StepRequest};

const MAX_DEPTH: usize = 512;
const MAX_BYTES: u64 = 256 << 20;

pub struct XmlJsonEngine;

#[derive(Default)]
struct Node {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    text: String,
}

fn parse(src: &str) -> Result<Node, EngineError> {
    let mut r = quick_xml::Reader::from_str(src);
    r.config_mut().trim_text(false);
    let mut stack: Vec<Node> = Vec::new();
    let mut root: Option<Node> = None;
    let attrs = |e: &quick_xml::events::BytesStart<'_>| -> Vec<(String, String)> {
        e.attributes()
            .flatten()
            .map(|a| {
                (
                    a.key.as_ref().to_string(),
                    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|v| v.into_owned())
                        .unwrap_or_default(),
                )
            })
            .collect()
    };
    let close = |stack: &mut Vec<Node>, root: &mut Option<Node>, n: Node| match stack.last_mut() {
        Some(parent) => parent.children.push(n),
        None => {
            if root.is_none() {
                *root = Some(n);
            }
        }
    };
    loop {
        match r
            .read_event()
            .map_err(|e| EngineError::Decode(format!("not valid XML: {e}")))?
        {
            Event::Start(e) => {
                if stack.len() >= MAX_DEPTH {
                    return Err(EngineError::Invalid("XML nested too deeply".into()));
                }
                stack.push(Node {
                    name: e.name().as_ref().to_string(),
                    attrs: attrs(&e),
                    ..Node::default()
                });
            }
            Event::Empty(e) => {
                let n = Node {
                    name: e.name().as_ref().to_string(),
                    attrs: attrs(&e),
                    ..Node::default()
                };
                close(&mut stack, &mut root, n);
            }
            Event::End(_) => {
                let n = stack
                    .pop()
                    .ok_or_else(|| EngineError::Decode("unbalanced XML".into()))?;
                close(&mut stack, &mut root, n);
            }
            Event::Text(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&t.xml10_content());
                }
            }
            Event::CData(t) => {
                if let Some(n) = stack.last_mut() {
                    n.text.push_str(&t);
                }
            }
            Event::GeneralRef(g) => {
                let c = if g.is_char_ref() {
                    g.resolve_char_ref().ok().flatten()
                } else {
                    match &*g {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        _ => None,
                    }
                };
                if let (Some(c), Some(n)) = (c, stack.last_mut()) {
                    n.text.push(c);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err(EngineError::Decode(
            "XML ends before its elements close".into(),
        ));
    }
    root.ok_or_else(|| EngineError::Decode("no XML element found".into()))
}

fn to_json(n: Node) -> Value {
    let text = n.text.trim().to_string();
    if n.attrs.is_empty() && n.children.is_empty() {
        return if text.is_empty() {
            Value::Null
        } else {
            Value::String(text)
        };
    }
    let mut m = Map::new();
    for (k, v) in n.attrs {
        m.insert(format!("@{k}"), Value::String(v));
    }
    for c in n.children {
        let name = c.name.clone();
        let v = to_json(c);
        match m.get_mut(&name) {
            Some(Value::Array(a)) => a.push(v),
            Some(prev) => {
                let first = prev.take();
                *prev = Value::Array(vec![first, v]);
            }
            None => {
                m.insert(name, v);
            }
        }
    }
    if !text.is_empty() {
        m.insert("#text".into(), Value::String(text));
    }
    Value::Object(m)
}

pub fn xml_to_json(src: &str) -> Result<Value, EngineError> {
    let root = parse(src.trim_start_matches('\u{feff}'))?;
    let mut m = Map::new();
    let name = root.name.clone();
    m.insert(name, to_json(root));
    Ok(Value::Object(m))
}

/// A valid XML name from a JSON key.
fn xml_name(k: &str) -> String {
    let mut s: String = k
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | ':') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if !s.starts_with(|c: char| c.is_alphabetic() || c == '_') {
        s.insert(0, '_');
    }
    s
}

fn esc(s: &str, attr: bool) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' if attr => o.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\r' | '\t') => {}
            c => o.push(c),
        }
    }
    o
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn write(name: &str, v: &Value, depth: usize, out: &mut String) -> Result<(), EngineError> {
    if depth > MAX_DEPTH {
        return Err(EngineError::Invalid("JSON nested too deeply".into()));
    }
    let pad = "  ".repeat(depth);
    let tag = xml_name(name);
    match v {
        Value::Array(items) => {
            for it in items {
                write(name, it, depth, out)?;
            }
        }
        Value::Object(m) => {
            out.push_str(&pad);
            out.push('<');
            out.push_str(&tag);
            for (k, a) in m.iter().filter(|(k, _)| k.starts_with('@')) {
                out.push_str(&format!(
                    " {}=\"{}\"",
                    xml_name(&k[1..]),
                    esc(&scalar(a), true)
                ));
            }
            let text = m.get("#text").map(scalar);
            let kids: Vec<(&String, &Value)> = m
                .iter()
                .filter(|(k, _)| !k.starts_with('@') && k.as_str() != "#text")
                .collect();
            if kids.is_empty() && text.is_none() {
                out.push_str("/>\n");
                return Ok(());
            }
            out.push('>');
            if let Some(t) = &text {
                out.push_str(&esc(t, false));
            }
            if !kids.is_empty() {
                out.push('\n');
                for (k, c) in kids {
                    write(k, c, depth + 1, out)?;
                }
                out.push_str(&pad);
            }
            out.push_str(&format!("</{tag}>\n"));
        }
        Value::Null => out.push_str(&format!("{pad}<{tag}/>\n")),
        other => out.push_str(&format!(
            "{pad}<{tag}>{}</{tag}>\n",
            esc(&scalar(other), false)
        )),
    }
    Ok(())
}

pub fn json_to_xml(v: &Value) -> Result<String, EngineError> {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    match v {
        Value::Object(m)
            if m.len() == 1
                && !m.keys().any(|k| k.starts_with('@'))
                && !m.values().any(Value::is_array) =>
        {
            let (k, c) = m
                .iter()
                .next()
                .ok_or_else(|| EngineError::Invalid("empty".into()))?;
            write(k, c, 0, &mut out)?;
        }
        Value::Array(items) => {
            out.push_str("<root>\n");
            for it in items {
                write("item", it, 1, &mut out)?;
            }
            out.push_str("</root>\n");
        }
        other => write("root", other, 0, &mut out)?,
    }
    Ok(out)
}

impl Engine for XmlJsonEngine {
    fn id(&self) -> &'static str {
        "xmljson"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("xmljson", "xml", "json")
                .loss(4)
                .time(10, 30)
                .note("attributes become @keys; comments are dropped"),
            Capability::new("xmljson", "json", "xml")
                .loss(4)
                .time(10, 30)
                .note("@keys become attributes"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        if std::fs::metadata(req.input)?.len() > MAX_BYTES {
            return Err(EngineError::Invalid("over 256 MB".into()));
        }
        let bytes = std::fs::read(req.input)?;
        let text = crate::text_from_bytes(&bytes);
        let out = match (req.from, req.to) {
            ("xml", "json") => {
                let v = xml_to_json(&text)?;
                serde_json::to_string_pretty(&v).map_err(EngineError::encode)? + "\n"
            }
            ("json", "xml") => {
                let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
                    .map_err(|e| EngineError::Decode(format!("not valid JSON: {e}")))?;
                json_to_xml(&v)?
            }
            (f, t) => return Err(EngineError::Invalid(format!("xmljson can't do {f} → {t}"))),
        };
        std::fs::write(req.output, out)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_to_json_conventions() {
        let x = r#"<?xml version="1.0"?>
<catalog xmlns:dc="http://purl.org/dc">
  <!-- comment -->
  <book id="1" lang="en"><dc:title>Rust &amp; You</dc:title><price>9.5</price></book>
  <book id="2"><dc:title>Two</dc:title><empty/></book>
  <note>plain</note>
</catalog>"#;
        let v = xml_to_json(x).unwrap();
        let c = &v["catalog"];
        assert_eq!(c["@xmlns:dc"], "http://purl.org/dc");
        assert_eq!(c["book"][0]["@id"], "1");
        assert_eq!(c["book"][0]["dc:title"], "Rust & You");
        assert_eq!(c["book"][1]["empty"], Value::Null);
        assert_eq!(c["note"], "plain");
    }

    #[test]
    fn round_trips() {
        let x = r#"<a k="v &quot;q&quot;"><b>1</b><b>2</b><c x="y">text</c><d/></a>"#;
        let v = xml_to_json(x).unwrap();
        let back = json_to_xml(&v).unwrap();
        assert_eq!(xml_to_json(&back).unwrap(), v);
        // JSON that isn't shaped like XML still converts.
        let j: Value = serde_json::from_str(r#"[{"name":"a b","1st":true},{"n":null}]"#).unwrap();
        let x = json_to_xml(&j).unwrap();
        assert!(
            x.contains("<root>") && x.contains("<_1st>true</_1st>"),
            "{x}"
        );
        assert!(xml_to_json("<a><b></a>").is_err());
        assert!(xml_to_json("").is_err());
    }
}
