//! A lossless XML tree for editing Office parts in place: qualified names, attributes in
//! order, text, CDATA, comments and processing instructions all survive a parse/write
//! round trip, so tools change only what they mean to.

use quick_xml::events::{BytesCData, BytesEnd, BytesPI, BytesStart, BytesText, Event};
use swirvo_core::EngineError;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Elem {
    /// Qualified name as written (`w:p`).
    pub name: String,
    /// Attributes in order: (qualified name, unescaped value).
    pub attrs: Vec<(String, String)>,
    pub kids: Vec<XNode>,
    /// Written as `<x/>` when it has no children.
    pub empty: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum XNode {
    Elem(Elem),
    Text(String),
    CData(String),
    Comment(String),
    Pi(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doc {
    /// The XML declaration as written (without `<?` `?>`), if any.
    pub decl: Option<String>,
    pub root: Elem,
}

/// Deepest nesting accepted (hostile files; see docxlayout::xml).
const MAX_DEPTH: usize = 512;

impl Elem {
    pub fn local(&self) -> &str {
        self.name.rsplit(':').next().unwrap_or(&self.name)
    }

    pub fn attr(&self, qname: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == qname)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, qname: &str, v: &str) {
        match self.attrs.iter_mut().find(|(k, _)| k == qname) {
            Some(a) => a.1 = v.to_string(),
            None => self.attrs.push((qname.to_string(), v.to_string())),
        }
    }

    pub fn children(&self) -> impl Iterator<Item = &Elem> {
        self.kids.iter().filter_map(|n| match n {
            XNode::Elem(e) => Some(e),
            _ => None,
        })
    }

    pub fn children_mut(&mut self) -> impl Iterator<Item = &mut Elem> {
        self.kids.iter_mut().filter_map(|n| match n {
            XNode::Elem(e) => Some(e),
            _ => None,
        })
    }

    /// All text below this element.
    pub fn text(&self) -> String {
        let mut s = String::new();
        fn walk(e: &Elem, s: &mut String) {
            for k in &e.kids {
                match k {
                    XNode::Text(t) | XNode::CData(t) => s.push_str(t),
                    XNode::Elem(c) => walk(c, s),
                    _ => {}
                }
            }
        }
        walk(self, &mut s);
        s
    }

    /// Visit every element depth-first (parents before children), mutably.
    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut Elem)) {
        f(self);
        for k in self.kids.iter_mut() {
            if let XNode::Elem(e) = k {
                e.walk_mut(f);
            }
        }
    }

    /// Rebuild the children: `f` decides for each child element whether it's kept,
    /// dropped, or replaced by its own children (unwrapped). Applied recursively.
    pub fn filter(&mut self, f: &dyn Fn(&Elem) -> Keep) {
        let kids = std::mem::take(&mut self.kids);
        for k in kids {
            match k {
                XNode::Elem(mut e) => match f(&e) {
                    Keep::Yes => {
                        e.filter(f);
                        self.kids.push(XNode::Elem(e));
                    }
                    Keep::Drop => {}
                    Keep::Unwrap => {
                        e.filter(f);
                        self.kids.extend(e.kids);
                    }
                },
                other => self.kids.push(other),
            }
        }
        if !self.kids.is_empty() {
            self.empty = false;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keep {
    Yes,
    Drop,
    Unwrap,
}

fn start_elem(e: &BytesStart<'_>) -> Elem {
    Elem {
        name: e.name().as_ref().to_string(),
        attrs: e
            .attributes()
            .with_checks(false)
            .flatten()
            .map(|a| {
                (
                    a.key.as_ref().to_string(),
                    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|_| a.value.to_string()),
                )
            })
            .collect(),
        kids: Vec::new(),
        empty: false,
    }
}

pub fn parse(src: &[u8]) -> Result<Doc, EngineError> {
    let text = String::from_utf8_lossy(src);
    let text = text.trim_start_matches('\u{feff}');
    let mut r = quick_xml::Reader::from_str(text);
    r.config_mut().trim_text(false);
    let mut doc = Doc::default();
    let mut stack: Vec<Elem> = vec![Elem::default()];
    loop {
        match r.read_event().map_err(EngineError::decode)? {
            Event::Decl(d) => {
                doc.decl = Some(d.trim().to_string());
            }
            Event::Start(e) => {
                if stack.len() > MAX_DEPTH {
                    return Err(EngineError::Decode("the XML is nested too deeply".into()));
                }
                stack.push(start_elem(&e));
            }
            Event::Empty(e) => {
                let mut el = start_elem(&e);
                el.empty = true;
                if let Some(top) = stack.last_mut() {
                    top.kids.push(XNode::Elem(el));
                }
            }
            Event::End(_) => {
                if stack.len() > 1
                    && let Some(done) = stack.pop()
                    && let Some(top) = stack.last_mut()
                {
                    top.kids.push(XNode::Elem(done));
                }
            }
            Event::Text(t) => {
                let s = t.xml10_content().into_owned();
                push_text(&mut stack, s);
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
                if let Some(c) = c {
                    push_text(&mut stack, c.to_string());
                }
            }
            Event::CData(c) => {
                if let Some(top) = stack.last_mut() {
                    top.kids.push(XNode::CData(c.into_inner().into_owned()));
                }
            }
            Event::Comment(c) => {
                if let Some(top) = stack.last_mut() {
                    top.kids.push(XNode::Comment(c.to_string()));
                }
            }
            Event::PI(p) => {
                if let Some(top) = stack.last_mut() {
                    top.kids.push(XNode::Pi(p.to_string()));
                }
            }
            Event::DocType(_) => {
                return Err(EngineError::Decode(
                    "DTDs aren't allowed in Office Open XML parts".into(),
                ));
            }
            Event::Eof => break,
        }
    }
    while stack.len() > 1 {
        if let Some(done) = stack.pop()
            && let Some(top) = stack.last_mut()
        {
            top.kids.push(XNode::Elem(done));
        }
    }
    let holder = stack.pop().unwrap_or_default();
    doc.root = holder
        .kids
        .into_iter()
        .find_map(|n| match n {
            XNode::Elem(e) => Some(e),
            _ => None,
        })
        .ok_or_else(|| EngineError::Decode("empty XML part".into()))?;
    Ok(doc)
}

fn push_text(stack: &mut [Elem], s: String) {
    let Some(top) = stack.last_mut() else { return };
    if let Some(XNode::Text(prev)) = top.kids.last_mut() {
        prev.push_str(&s);
    } else {
        top.kids.push(XNode::Text(s));
    }
}

fn write_elem(w: &mut quick_xml::Writer<Vec<u8>>, e: &Elem) -> Result<(), EngineError> {
    let mut start = BytesStart::new(e.name.as_str());
    for (k, v) in &e.attrs {
        start.push_attribute((k.as_str(), v.as_str()));
    }
    if e.kids.is_empty() && e.empty {
        w.write_event(Event::Empty(start))
            .map_err(EngineError::encode)?;
        return Ok(());
    }
    w.write_event(Event::Start(start))
        .map_err(EngineError::encode)?;
    for k in &e.kids {
        match k {
            XNode::Elem(c) => write_elem(w, c)?,
            XNode::Text(t) => w
                .write_event(Event::Text(BytesText::new(t)))
                .map_err(EngineError::encode)?,
            XNode::CData(t) => w
                .write_event(Event::CData(BytesCData::new(t.as_str())))
                .map_err(EngineError::encode)?,
            XNode::Comment(t) => w
                .write_event(Event::Comment(BytesText::from_escaped(t.as_str())))
                .map_err(EngineError::encode)?,
            XNode::Pi(t) => w
                .write_event(Event::PI(BytesPI::new(t.as_str())))
                .map_err(EngineError::encode)?,
        }
    }
    w.write_event(Event::End(BytesEnd::new(e.name.as_str())))
        .map_err(EngineError::encode)?;
    Ok(())
}

impl Doc {
    pub fn write(&self) -> Result<Vec<u8>, EngineError> {
        let mut w = quick_xml::Writer::new(Vec::new());
        let mut out = Vec::new();
        if let Some(d) = &self.decl {
            out.extend_from_slice(b"<?");
            out.extend_from_slice(d.as_bytes());
            out.extend_from_slice(b"?>\r\n");
        }
        write_elem(&mut w, &self.root)?;
        out.extend(w.into_inner());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let src = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="urn:w" xmlns:r="urn:r"><w:body><w:p w:rsidR="00A1"><w:r><w:t xml:space="preserve"> a &amp; b &lt;c&gt; </w:t></w:r><!-- note --><w:r><w:t>“x”</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#.as_bytes();
        let d = parse(src).unwrap();
        let out = d.write().unwrap();
        let again = parse(&out).unwrap();
        assert_eq!(d, again);
        let s = String::from_utf8(out).unwrap();
        assert!(
            s.contains("<w:sectPr/>")
                && s.contains("a &amp; b &lt;c&gt;")
                && s.contains("<!-- note -->"),
            "{s}"
        );
    }

    #[test]
    fn filter_drops_and_unwraps() {
        let mut d = parse(b"<a><ins><r>1</r></ins><del><r>2</r></del><r>3</r></a>").unwrap();
        d.root.filter(&|e| match e.local() {
            "ins" => Keep::Unwrap,
            "del" => Keep::Drop,
            _ => Keep::Yes,
        });
        assert_eq!(d.root.text(), "13");
    }
}
