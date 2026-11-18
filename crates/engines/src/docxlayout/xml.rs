//! A compact element tree for WordprocessingML parts.
//!
//! Names are local names (`w:p` → `p`, `r:embed` → `embed`): Word's vocabularies don't
//! clash where we look, and it keeps unusual namespace prefixes from mattering.
//! `mc:AlternateContent` is resolved while building: the DrawingML `Choice` is spliced in
//! unless it needs something we don't draw (groups, canvases, chartex), else the
//! `Fallback`.

use quick_xml::events::Event;
use swirvo_core::EngineError;

#[derive(Debug, Default, Clone)]
pub struct El {
    pub name: Box<str>,
    pub attrs: Vec<(Box<str>, Box<str>)>,
    pub kids: Vec<Node>,
}

#[derive(Debug, Clone)]
pub enum Node {
    El(El),
    Text(String),
}

impl El {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| &**k == key)
            .map(|(_, v)| &**v)
    }

    /// `w:val`, the attribute most properties carry.
    pub fn val(&self) -> Option<&str> {
        self.attr("val")
    }

    pub fn children(&self) -> impl Iterator<Item = &El> {
        self.kids.iter().filter_map(|n| match n {
            Node::El(e) => Some(e),
            Node::Text(_) => None,
        })
    }

    pub fn child(&self, name: &str) -> Option<&El> {
        self.children().find(|e| &*e.name == name)
    }

    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a El> + 'a {
        self.children().filter(move |e| &*e.name == name)
    }

    /// First descendant with this name (depth-first).
    pub fn find(&self, name: &str) -> Option<&El> {
        for c in self.children() {
            if &*c.name == name {
                return Some(c);
            }
            if let Some(f) = c.find(name) {
                return Some(f);
            }
        }
        None
    }

    /// Concatenated text of all descendants.
    pub fn text(&self) -> String {
        let mut s = String::new();
        self.collect_text(&mut s);
        s
    }

    fn collect_text(&self, s: &mut String) {
        for n in &self.kids {
            match n {
                Node::Text(t) => s.push_str(t),
                Node::El(e) => e.collect_text(s),
            }
        }
    }

    /// An on/off property: present without `w:val`, or with a true value.
    pub fn on(&self) -> bool {
        !matches!(self.val(), Some("0" | "false" | "off" | "none"))
    }
}

fn local(name: &str) -> Box<str> {
    name.rsplit(':').next().unwrap_or(name).into()
}

fn attrs(e: &quick_xml::events::BytesStart<'_>) -> Vec<(Box<str>, Box<str>)> {
    e.attributes()
        .flatten()
        .map(|a| {
            let v = a
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .map(|v| v.into_owned())
                .unwrap_or_default();
            (local(a.key.as_ref()), v.into_boxed_str())
        })
        .collect()
}

/// Elements whose text we keep. Everything else drops whitespace-only text nodes.
fn keeps_text(name: &str) -> bool {
    matches!(name, "t" | "delText" | "instrText" | "tab" | "sym")
        || name == "posOffset"
        || name == "align"
}

/// Deepest element nesting we accept. Real documents stay well under 100 (body, nested
/// tables, a text box, runs); a hostile part nested 100 000 deep would otherwise overflow the
/// stack in the recursive walks below and in `Drop`, which `catch_unwind` can't catch.
const MAX_DEPTH: usize = 256;

/// Parse a part into its root element.
pub fn parse(src: &str) -> Result<El, EngineError> {
    let mut r = quick_xml::Reader::from_str(src);
    r.config_mut().trim_text(false);
    let mut stack: Vec<El> = vec![El {
        name: "#doc".into(),
        ..Default::default()
    }];
    loop {
        let ev = r.read_event().map_err(EngineError::decode)?;
        match ev {
            Event::Start(e) => {
                if stack.len() > MAX_DEPTH {
                    return Err(EngineError::Decode(
                        "this document is nested too deeply to be a real DOCX file".into(),
                    ));
                }
                stack.push(El {
                    name: local(e.name().as_ref()),
                    attrs: attrs(&e),
                    kids: Vec::new(),
                });
            }
            Event::Empty(e) => {
                let el = El {
                    name: local(e.name().as_ref()),
                    attrs: attrs(&e),
                    kids: Vec::new(),
                };
                if let Some(top) = stack.last_mut() {
                    top.kids.push(Node::El(el));
                }
            }
            Event::End(_) => {
                if stack.len() > 1
                    && let Some(done) = stack.pop()
                    && let Some(top) = stack.last_mut()
                {
                    if &*done.name == "AlternateContent" {
                        top.kids.extend(alternate(done));
                    } else {
                        top.kids.push(Node::El(done));
                    }
                }
            }
            Event::Text(t) => push_text(&mut stack, &t.xml10_content()),
            Event::CData(t) => push_text(&mut stack, &t.into_inner()),
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
                if let Some(c) = c {
                    push_text(&mut stack, c.encode_utf8(&mut [0; 4]));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    // Unclosed elements (truncated part): fold them up so nothing is lost.
    while stack.len() > 1 {
        if let Some(done) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.kids.push(Node::El(done));
        }
    }
    let mut doc = stack.pop().unwrap_or_default();
    let root = doc.kids.drain(..).find_map(|n| match n {
        Node::El(e) => Some(e),
        Node::Text(_) => None,
    });
    root.ok_or_else(|| EngineError::Decode("empty XML part".into()))
}

fn push_text(stack: &mut [El], t: &str) {
    let Some(top) = stack.last_mut() else { return };
    if !keeps_text(&top.name) && t.trim().is_empty() {
        return;
    }
    if let Some(Node::Text(prev)) = top.kids.last_mut() {
        prev.push_str(t);
    } else {
        top.kids.push(Node::Text(t.to_string()));
    }
}

/// The children of the `mc:AlternateContent` branch we can render, spliced into the
/// parent in its place.
fn alternate(el: El) -> Vec<Node> {
    let mut choice = None;
    let mut fallback = None;
    for n in el.kids {
        if let Node::El(e) = n {
            match &*e.name {
                "Choice" if choice.is_none() => {
                    let needs = e.attr("Requires").unwrap_or("");
                    let unsupported = needs
                        .split_whitespace()
                        .any(|p| matches!(p, "wpg" | "wpc" | "cx" | "cx1" | "cx2" | "aink"));
                    if !unsupported {
                        choice = Some(e);
                    }
                }
                "Fallback" => fallback = Some(e),
                _ => {}
            }
        }
    }
    choice.or(fallback).map(|e| e.kids).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_text_and_alternates() {
        let x = r#"<w:document xmlns:w="w" xmlns:mc="mc"><w:body><w:p><w:r><w:t xml:space="preserve"> a &amp; b </w:t></w:r></w:p>
            <mc:AlternateContent><mc:Choice Requires="wps"><w:p><w:r><w:t>new</w:t></w:r></w:p></mc:Choice>
            <mc:Fallback><w:p><w:r><w:t>old</w:t></w:r></w:p></mc:Fallback></mc:AlternateContent></w:body></w:document>"#;
        let d = parse(x).unwrap();
        let body = d.child("body").unwrap();
        assert_eq!(body.child("p").unwrap().text(), " a & b ");
        let ps: Vec<String> = body.all("p").map(El::text).collect();
        assert_eq!(ps, [" a & b ", "new"]);
    }

    #[test]
    fn deep_nesting_is_refused_not_a_crash() {
        let n = 200_000;
        let x = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        assert!(matches!(parse(&x), Err(EngineError::Decode(_))));
        let ok = format!("{}{}", "<a>".repeat(200), "</a>".repeat(200));
        assert!(parse(&ok).is_ok());
    }
}
