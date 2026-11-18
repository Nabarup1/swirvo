//! Readers: every supported source format → [`Doc`].

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::Path;

use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use quick_xml::events::Event as XmlEvent;
use swirvo_core::EngineError;

use super::{Block, Builder, Doc, PICTURE_LIMIT, PICTURES_LIMIT, Picture};
use crate::text::read_text;

/// Biggest single XML part we'll inflate from a DOCX/ODT/EPUB (zip-bomb guard).
const PART_LIMIT: u64 = 256 << 20;

/// Pictures taken from one document, within the per-picture and per-document limits.
struct Pics {
    left: u64,
}

impl Pics {
    fn new() -> Pics {
        Pics {
            left: PICTURES_LIMIT,
        }
    }

    /// `bytes`, if they're a picture we can show and there's room for them.
    fn take(&mut self, bytes: Vec<u8>) -> Option<Vec<u8>> {
        let n = bytes.len() as u64;
        if n == 0 || n > PICTURE_LIMIT || n > self.left || !usable(&bytes) {
            return None;
        }
        self.left -= n;
        Some(bytes)
    }

    /// A file inside the package (never one it links to elsewhere).
    fn read_zip(&mut self, z: &mut Zip<'_>, name: &str) -> Option<Vec<u8>> {
        let f = z.by_name(name).ok()?;
        if f.size() > PICTURE_LIMIT.min(self.left) {
            return None;
        }
        let mut v = Vec::new();
        f.take(PICTURE_LIMIT).read_to_end(&mut v).ok()?;
        self.take(v)
    }
}

/// Bitmaps the image decoders know, SVG, and Windows metafiles.
fn usable(b: &[u8]) -> bool {
    if image::guess_format(b).is_ok() || crate::emf::is_metafile(b) {
        return true;
    }
    let head = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    head.contains("<svg")
}

fn picture(bytes: Vec<u8>, alt: &str, width_pt: Option<f32>) -> Picture {
    Picture {
        bytes: std::sync::Arc::new(bytes),
        alt: alt.trim().to_string(),
        width_pt: width_pt
            .filter(|w| w.is_finite() && *w >= 1.0)
            .map(|w| w.round().min(5000.0) as u32),
    }
}

/// An OpenDocument or CSS length (`6.5cm`, `2in`, `120pt`, `40mm`, `300px`) in points.
fn length_pt(s: &str) -> Option<f32> {
    let s = s.trim();
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(s.len());
    let v: f32 = s[..split].parse().ok()?;
    let k = match s[split..].trim() {
        "pt" => 1.0,
        "in" => 72.0,
        "cm" => 72.0 / 2.54,
        "mm" => 72.0 / 25.4,
        "pc" => 12.0,
        "px" | "" => 0.75,
        _ => return None,
    };
    Some(v * k)
}

/// The bytes of a `data:` URI's payload (after `data:`): base64 or percent-encoded.
fn data_uri(d: &str) -> Option<Vec<u8>> {
    let (meta, payload) = d.split_once(',')?;
    if meta.to_ascii_lowercase().ends_with(";base64") {
        base64_decode(payload)
    } else {
        Some(percent_decode(payload).into_bytes())
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            c if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

pub fn read_path(path: &Path, from: &str) -> Result<Doc, EngineError> {
    match from {
        "md" => Ok(markdown_at(&read_text(path)?, Some(path))),
        "html" => Ok(html(&read_text(path)?)),
        "txt" => Ok(txt(&read_text(path)?)),
        "rtf" => Ok(rtf(&std::fs::read(path)?)),
        "docx" => docx(&std::fs::read(path)?),
        "odt" => odt(&std::fs::read(path)?),
        "epub" => epub(&std::fs::read(path)?),
        "pdf" => pdf(&std::fs::read(path)?),
        "csv" | "tsv" => delimited(&read_text(path)?, from),
        "xlsx" | "xls" | "ods" => sheet(path),
        other => Err(EngineError::Invalid(format!(
            "document engine can't read {other}"
        ))),
    }
}

// ---------------------------------------------------------------- plain text

pub fn txt(src: &str) -> Doc {
    let mut blocks = Vec::new();
    for para in src.replace("\r\n", "\n").split("\n\n") {
        let p = para.trim_matches('\n');
        if p.trim().is_empty() {
            continue;
        }
        if p.contains('\u{c}') {
            blocks.push(Block::PageBreak);
        }
        let text = p.replace('\u{c}', "");
        blocks.push(Block::Para(vec![super::Inline {
            text: text.trim_end().to_string(),
            style: Default::default(),
        }]));
    }
    Doc {
        title: None,
        blocks,
    }
}

// ---------------------------------------------------------------- Markdown

/// Markdown text with no file around it (no pictures from files).
#[cfg(test)]
pub fn markdown(src: &str) -> Doc {
    markdown_at(src, None)
}

/// The file a Markdown picture names, if it's inside the folder `<stem> files` next to
/// `md` (where our Markdown writer's pictures would sit). Anything else (web addresses,
/// absolute paths, other folders, `..` out of it) is never opened.
fn md_local(dest: &str, md: &Path) -> Option<std::path::PathBuf> {
    let d = percent_decode(dest.trim());
    if d.contains("://")
        || d.starts_with(['/', '\\'])
        || d.contains(':')
        || d.contains('?')
        || d.contains('#')
    {
        return None;
    }
    let dir = md.parent()?;
    let stem = md.file_stem()?.to_string_lossy();
    let folder = dir.join(format!("{stem} files")).canonicalize().ok()?;
    let file = dir.join(d.replace('\\', "/")).canonicalize().ok()?;
    (file.starts_with(&folder) && file.is_file()).then_some(file)
}

/// Markdown; `path` is the file's own location, for pictures in its `<stem> files` folder.
pub fn markdown_at(src: &str, path: Option<&Path>) -> Doc {
    let mut b = Builder::new();
    let mut pics = Pics::new();
    // A picture being read: (where it is, its alt text so far).
    let mut image: Option<(String, String)> = None;
    let mut code: Option<String> = None;
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    for ev in Parser::new_ext(src, opts) {
        match ev {
            MdEvent::Start(tag) => match tag {
                Tag::Paragraph | Tag::Heading { .. } => b.para(),
                Tag::BlockQuote(_) => b.open_quote(),
                Tag::CodeBlock(_) => {
                    b.para();
                    code = Some(String::new());
                }
                Tag::List(start) => b.open_list(start.is_some()),
                Tag::Item => b.open_item(),
                Tag::Table(_) => b.open_table(),
                Tag::TableHead => b.set_table_header(),
                Tag::TableCell => b.open_cell(),
                Tag::Emphasis => b.push_style(|s| s.italic = true),
                Tag::Strong => b.push_style(|s| s.bold = true),
                Tag::Link { dest_url, .. } => b.push_style(|s| s.link = Some(dest_url.to_string())),
                Tag::Image { dest_url, .. } => image = Some((dest_url.to_string(), String::new())),
                _ => {}
            },
            MdEvent::End(tag) => match tag {
                TagEnd::Paragraph => b.para(),
                TagEnd::Heading(level) => b.heading(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                }),
                TagEnd::BlockQuote(_) => b.close_quote(),
                TagEnd::CodeBlock => {
                    let c = code.take().unwrap_or_default();
                    b.block(Block::Code(c.trim_end_matches('\n').to_string()));
                }
                TagEnd::List(_) => b.close_list(),
                TagEnd::Item => b.close_item(),
                TagEnd::Table => b.close_table(),
                TagEnd::TableHead | TagEnd::TableRow => b.close_row(),
                TagEnd::TableCell => b.close_cell(),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Link => b.pop_style(),
                TagEnd::Image => {
                    let Some((dest, alt)) = image.take() else {
                        continue;
                    };
                    let bytes = match dest.get(..5) {
                        Some(h) if h.eq_ignore_ascii_case("data:") => data_uri(&dest[5..]),
                        _ => path.and_then(|md| md_local(&dest, md)).and_then(|f| {
                            let n = std::fs::metadata(&f).ok()?.len();
                            (n <= PICTURE_LIMIT).then(|| std::fs::read(f).ok())?
                        }),
                    };
                    match bytes.and_then(|v| pics.take(v)) {
                        Some(v) => b.image(picture(v, &alt, None)),
                        // A picture we can't (or won't) open keeps its words.
                        None => b.text(&alt),
                    }
                }
                _ => {}
            },
            MdEvent::Text(t) => match (code.as_mut(), image.as_mut()) {
                (Some(c), _) => c.push_str(&t),
                (None, Some((_, alt))) => alt.push_str(&t),
                (None, None) => b.text(&t),
            },
            MdEvent::Code(t) => {
                b.push_style(|s| s.code = true);
                b.text(&t);
                b.pop_style();
            }
            MdEvent::SoftBreak => b.text(" "),
            MdEvent::HardBreak => b.text("\n"),
            MdEvent::Rule => b.block(Block::Rule),
            MdEvent::Html(h) | MdEvent::InlineHtml(h) if h.trim_start().starts_with("<br") => {
                b.text("\n")
            }
            _ => {}
        }
    }
    Doc {
        title: None,
        blocks: b.finish(),
    }
}

/// Index of the `}` closing the group whose content starts at `from` (or the end).
fn group_end(bytes: &[u8], from: usize) -> usize {
    let mut depth = 1usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1, // skip the escaped character (`\{`, `\}`, `\\`)
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// A `\pict` group's picture: PNG, JPEG, EMF or WMF as hex (or `\bin`) data, with its
/// width from `\picwgoal` (twips) and `\picscalex` (percent).
fn rtf_picture(g: &[u8], pics: &mut Pics) -> Option<Picture> {
    let (mut wgoal, mut scale, mut known) = (None::<f32>, 100.0f32, false);
    let mut data = Vec::new();
    let mut half: Option<u8> = None;
    let (mut i, mut depth) = (0usize, 0usize);
    while i < g.len() {
        match g[i] {
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' => {
                let mut j = i + 1;
                while j < g.len() && g[j].is_ascii_alphabetic() {
                    j += 1;
                }
                let word = std::str::from_utf8(&g[i + 1..j]).unwrap_or("");
                let mut k = j;
                if k < g.len() && (g[k] == b'-' || g[k].is_ascii_digit()) {
                    k += 1;
                    while k < g.len() && g[k].is_ascii_digit() {
                        k += 1;
                    }
                }
                let param: Option<i64> = std::str::from_utf8(&g[j..k])
                    .ok()
                    .and_then(|p| p.parse().ok());
                if k < g.len() && g[k] == b' ' {
                    k += 1;
                }
                if depth == 0 {
                    match word {
                        "pngblip" | "jpegblip" | "emfblip" | "wmetafile" => known = true,
                        "picwgoal" => wgoal = param.map(|t| t as f32 / 20.0),
                        "picscalex" => scale = param.map_or(100.0, |p| p as f32),
                        "bin" => {
                            let n = param.unwrap_or(0).max(0) as usize;
                            let end = (k + n).min(g.len());
                            data.extend_from_slice(&g[k..end]);
                            k = end;
                        }
                        _ => {}
                    }
                }
                i = k.max(i + 1);
            }
            c if depth == 0 && c.is_ascii_hexdigit() => {
                let v = (c as char).to_digit(16).unwrap_or(0) as u8;
                match half.take() {
                    Some(h) => data.push(h << 4 | v),
                    None => half = Some(v),
                }
                if data.len() as u64 > PICTURE_LIMIT {
                    return None;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    if !known {
        return None;
    }
    // Old-style WMF data has no placeable header; the metafile reader handles both.
    let bytes = pics.take(data)?;
    Some(picture(bytes, "", wgoal.map(|w| w * scale / 100.0)))
}

// ---------------------------------------------------------------- HTML

/// Finds the file a picture's `src` names inside the document (an e-book's package), or
/// None. Plain web pages get one that finds nothing: their linked files are never loaded.
type Resolve<'a> = dyn FnMut(&str) -> Option<Vec<u8>> + 'a;

struct HtmlWalk<'a> {
    b: Builder,
    /// Last emitted character was whitespace (collapses runs across elements).
    space: bool,
    pics: &'a mut Pics,
    resolve: &'a mut Resolve<'a>,
}

impl HtmlWalk<'_> {
    /// A picture's bytes: embedded in the page (`data:`), or from the document's package.
    fn load(&mut self, src: &str) -> Option<Vec<u8>> {
        let src = src.trim();
        let bytes = match src.get(..5) {
            Some(p) if p.eq_ignore_ascii_case("data:") => data_uri(&src[5..])?,
            _ => (self.resolve)(src)?,
        };
        self.pics.take(bytes)
    }

    fn text(&mut self, t: &str) {
        let mut out = String::with_capacity(t.len());
        for c in t.chars() {
            if c.is_whitespace() {
                if !self.space {
                    out.push(' ');
                    self.space = true;
                }
            } else {
                out.push(c);
                self.space = false;
            }
        }
        self.b.text(&out);
    }

    fn brk(&mut self) {
        self.space = true;
    }

    fn walk(&mut self, node: ego_tree::NodeRef<'_, scraper::Node>) {
        for child in node.children() {
            match child.value() {
                scraper::Node::Text(t) => self.text(t),
                scraper::Node::Element(e) => self.element(child, e.name(), e),
                _ => {}
            }
        }
    }

    fn element(
        &mut self,
        node: ego_tree::NodeRef<'_, scraper::Node>,
        name: &str,
        e: &scraper::node::Element,
    ) {
        match name {
            "script" | "style" | "head" | "noscript" | "template" | "iframe" | "button"
            | "select" | "math" => {}
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.b.para();
                self.brk();
                self.walk(node);
                self.b.heading(name.as_bytes()[1] - b'0');
                self.brk();
            }
            "br" => {
                self.b.text("\n");
                self.brk();
            }
            "hr" => self.b.block(Block::Rule),
            "ul" | "ol" | "menu" => {
                self.b.open_list(name == "ol");
                self.walk(node);
                self.b.close_list();
            }
            "li" => {
                self.b.open_item();
                self.brk();
                self.walk(node);
                self.b.close_item();
            }
            "blockquote" => {
                self.b.open_quote();
                self.walk(node);
                self.b.close_quote();
            }
            "pre" => {
                let text: String = node
                    .descendants()
                    .filter_map(|n| n.value().as_text().map(|t| t.to_string()))
                    .collect();
                self.b
                    .block(Block::Code(text.trim_end_matches('\n').to_string()));
                self.brk();
            }
            "table" => {
                self.b.open_table();
                self.walk(node);
                self.b.close_table();
            }
            "tr" => {
                self.walk(node);
                self.b.close_row();
            }
            "th" | "td" => {
                if name == "th" {
                    self.b.set_table_header();
                }
                self.b.open_cell();
                self.brk();
                self.walk(node);
                self.b.close_cell();
            }
            "b" | "strong" => self.styled(node, |s| s.bold = true),
            "i" | "em" | "cite" | "var" => self.styled(node, |s| s.italic = true),
            "code" | "kbd" | "samp" | "tt" => self.styled(node, |s| s.code = true),
            "a" => match e.attr("href").filter(|h| {
                !h.starts_with('#') && !h.to_ascii_lowercase().starts_with("javascript:")
            }) {
                Some(h) => {
                    let h = h.to_string();
                    self.styled(node, move |s| s.link = Some(h));
                }
                None => self.walk(node),
            },
            "img" => {
                let alt = e.attr("alt").unwrap_or_default().trim().to_string();
                // The width attribute, or a `width:` in its style (what our writer puts).
                let width = e.attr("width").and_then(length_pt).or_else(|| {
                    e.attr("style")?.split(';').find_map(|d| {
                        let (k, v) = d.split_once(':')?;
                        (k.trim().eq_ignore_ascii_case("width")).then(|| length_pt(v))?
                    })
                });
                match e.attr("src").and_then(|s| self.load(s)) {
                    Some(bytes) => {
                        self.b.image(picture(bytes, &alt, width));
                        self.brk();
                    }
                    None if !alt.is_empty() => self.text(&format!("[{alt}]")),
                    None => {}
                }
            }
            // E-book covers and full-page pictures: `<svg><image href="cover.jpg"/></svg>`.
            "svg" => {
                let href = node.descendants().find_map(|n| {
                    n.value()
                        .as_element()
                        .filter(|e| e.name() == "image")
                        .and_then(|e| e.attr("href"))
                });
                if let Some(bytes) = href.and_then(|h| self.load(h)) {
                    self.b.image(picture(bytes, "", None));
                    self.brk();
                }
            }
            "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "aside"
            | "figure" | "figcaption" | "body" | "html" | "dl" | "dt" | "dd" | "address"
            | "center" | "nav" | "form" | "fieldset" | "details" | "summary" => {
                self.b.para();
                self.brk();
                self.walk(node);
                self.b.para();
                self.brk();
            }
            _ => self.walk(node),
        }
    }

    fn styled(
        &mut self,
        node: ego_tree::NodeRef<'_, scraper::Node>,
        f: impl FnOnce(&mut super::Style),
    ) {
        self.b.push_style(f);
        self.walk(node);
        self.b.pop_style();
    }
}

fn html_into<'a>(
    src: &str,
    b: Builder,
    pics: &'a mut Pics,
    resolve: &'a mut Resolve<'a>,
) -> (Builder, Option<String>) {
    let dom = scraper::Html::parse_document(src);
    let title = scraper::Selector::parse("title")
        .ok()
        .and_then(|sel| {
            dom.select(&sel)
                .next()
                .map(|t| t.text().collect::<String>().trim().to_string())
        })
        .filter(|t| !t.is_empty());
    let mut w = HtmlWalk {
        b,
        space: true,
        pics,
        resolve,
    };
    w.walk(*dom.root_element());
    w.b.para();
    (w.b, title)
}

pub fn html(src: &str) -> Doc {
    let (mut pics, mut none) = (Pics::new(), |_: &str| None);
    let (b, title) = html_into(src, Builder::new(), &mut pics, &mut none);
    Doc {
        title,
        blocks: b.finish(),
    }
}

// ---------------------------------------------------------------- XML helper

/// A simplified XML event: local names only, attributes unescaped, entities resolved.
enum X {
    Start(String, Vec<(String, String)>),
    End(String),
    Text(String),
}

fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_string()
}

fn xml_events(src: &str, mut f: impl FnMut(X)) -> Result<(), EngineError> {
    let mut r = quick_xml::Reader::from_str(src);
    r.config_mut().trim_text(false);
    loop {
        match r.read_event().map_err(EngineError::decode)? {
            XmlEvent::Start(e) => f(X::Start(local(e.name().as_ref()), attrs(&e))),
            XmlEvent::Empty(e) => {
                let name = local(e.name().as_ref());
                f(X::Start(name.clone(), attrs(&e)));
                f(X::End(name));
            }
            XmlEvent::End(e) => f(X::End(local(e.name().as_ref()))),
            XmlEvent::Text(t) => f(X::Text(t.xml10_content().into_owned())),
            XmlEvent::CData(t) => f(X::Text(t.to_string())),
            XmlEvent::GeneralRef(r) => {
                let c = if r.is_char_ref() {
                    r.resolve_char_ref().ok().flatten()
                } else {
                    match &*r {
                        "amp" => Some('&'),
                        "lt" => Some('<'),
                        "gt" => Some('>'),
                        "quot" => Some('"'),
                        "apos" => Some('\''),
                        "nbsp" => Some('\u{a0}'),
                        _ => None,
                    }
                };
                if let Some(c) = c {
                    f(X::Text(c.to_string()));
                }
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

fn attrs(e: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|a| {
            (
                local(a.key.as_ref()),
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(|v| v.into_owned())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

fn attr<'a>(a: &'a [(String, String)], key: &str) -> Option<&'a str> {
    a.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

fn open_zip(bytes: &[u8]) -> Result<Zip<'_>, EngineError> {
    zip::ZipArchive::new(Cursor::new(bytes)).map_err(EngineError::decode)
}

fn part(z: &mut Zip<'_>, name: &str) -> Result<Option<String>, EngineError> {
    let Ok(f) = z.by_name(name) else {
        return Ok(None);
    };
    let mut s = String::new();
    f.take(PART_LIMIT).read_to_string(&mut s)?;
    Ok(Some(s))
}

fn on(v: Option<&str>) -> bool {
    !matches!(v, Some("0" | "false" | "off" | "none"))
}

// ---------------------------------------------------------------- DOCX

pub fn docx(bytes: &[u8]) -> Result<Doc, EngineError> {
    let mut z = open_zip(bytes)?;
    let body = part(&mut z, "word/document.xml")?
        .ok_or_else(|| EngineError::Decode("not a Word document (no word/document.xml)".into()))?;

    // styleId → heading level (Title = 1, "heading N" = N, or an outline level).
    let mut heading_of: HashMap<String, u8> = HashMap::new();
    if let Some(styles) = part(&mut z, "word/styles.xml")? {
        let mut id = String::new();
        xml_events(&styles, |x| {
            if let X::Start(n, a) = x {
                match n.as_str() {
                    "style" => id = attr(&a, "styleId").unwrap_or_default().to_string(),
                    "name" if !id.is_empty() => {
                        let name = attr(&a, "val").unwrap_or_default().to_ascii_lowercase();
                        let level = match name.as_str() {
                            "title" => Some(1),
                            "subtitle" => Some(2),
                            n => n
                                .strip_prefix("heading ")
                                .and_then(|d| d.trim().parse::<u8>().ok()),
                        };
                        if let Some(l) = level {
                            heading_of.insert(id.clone(), l);
                        }
                    }
                    "outlineLvl" if !id.is_empty() => {
                        if let Some(l) = attr(&a, "val")
                            .and_then(|v| v.parse::<u8>().ok())
                            .filter(|l| *l < 9)
                        {
                            heading_of.entry(id.clone()).or_insert(l + 1);
                        }
                    }
                    _ => {}
                }
            }
        })?;
    }

    // (numId, level) → ordered?
    let mut ordered_of: HashMap<(String, usize), bool> = HashMap::new();
    if let Some(num) = part(&mut z, "word/numbering.xml")? {
        let mut abs: HashMap<String, HashMap<usize, bool>> = HashMap::new();
        let mut num_to_abs: Vec<(String, String)> = Vec::new();
        let (mut cur_abs, mut cur_lvl, mut cur_num) = (String::new(), 0usize, String::new());
        xml_events(&num, |x| {
            if let X::Start(n, a) = x {
                match n.as_str() {
                    "abstractNum" => {
                        cur_abs = attr(&a, "abstractNumId").unwrap_or_default().to_string()
                    }
                    "lvl" => cur_lvl = attr(&a, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
                    "numFmt" => {
                        let fmt = attr(&a, "val").unwrap_or("bullet");
                        abs.entry(cur_abs.clone())
                            .or_default()
                            .insert(cur_lvl, !matches!(fmt, "bullet" | "none"));
                    }
                    "num" => cur_num = attr(&a, "numId").unwrap_or_default().to_string(),
                    "abstractNumId" => num_to_abs.push((
                        cur_num.clone(),
                        attr(&a, "val").unwrap_or_default().to_string(),
                    )),
                    _ => {}
                }
            }
        })?;
        for (n, a) in num_to_abs {
            if let Some(levels) = abs.get(&a) {
                for (&l, &o) in levels {
                    ordered_of.insert((n.clone(), l), o);
                }
            }
        }
    }

    let mut rels: HashMap<String, String> = HashMap::new();
    // Relationship id → a part inside the package (linked files outside it are left out).
    let mut inside: HashMap<String, String> = HashMap::new();
    if let Some(r) = part(&mut z, "word/_rels/document.xml.rels")? {
        xml_events(&r, |x| {
            if let X::Start(n, a) = x
                && n == "Relationship"
                && let (Some(id), Some(t)) = (attr(&a, "Id"), attr(&a, "Target"))
            {
                rels.insert(id.to_string(), t.to_string());
                if attr(&a, "TargetMode") != Some("External") && !t.contains("://") {
                    let path = match t.strip_prefix('/') {
                        Some(abs) => normalize(abs),
                        None => normalize(&format!("word/{}", percent_decode(t))),
                    };
                    inside.insert(id.to_string(), path);
                }
            }
        })?;
    }
    let mut pics = Pics::new();
    // The picture being read: its relationship, alternative text and width (points).
    #[derive(Default)]
    struct Pic {
        rid: Option<String>,
        alt: String,
        width: Option<f32>,
    }
    let mut pic: Option<Pic> = None;

    let mut b = Builder::new();
    #[derive(Default)]
    struct P {
        style: String,
        num: Option<(String, usize)>,
        page_break: bool,
    }
    let mut p = P::default();
    let (mut bold, mut italic, mut in_t, mut in_rpr) = (false, false, false, false);
    let mut link: Option<String> = None;
    let mut table_depth = 0usize;
    let mut skip = 0usize; // inside deleted text, field codes, drawings, footnote refs…
    let mut cell_has_text = false;

    xml_events(&body, |x| match x {
        X::Start(n, a) => match n.as_str() {
            // A picture in the text (not inside a text box or deleted text).
            "drawing" | "pict" | "object" if skip == 0 => {
                pic = Some(Pic::default());
                skip += 1;
            }
            "extent" if skip == 1 => {
                if let Some(pc) = pic.as_mut()
                    && pc.width.is_none()
                {
                    pc.width = attr(&a, "cx")
                        .and_then(|v| v.parse::<f32>().ok())
                        .map(|emu| emu / 12700.0);
                }
            }
            "docPr" => {
                if let Some(pc) = pic.as_mut() {
                    pc.alt = attr(&a, "descr")
                        .filter(|d| !d.trim().is_empty())
                        .or(attr(&a, "title"))
                        .unwrap_or_default()
                        .to_string();
                }
            }
            "blip" | "imagedata" => {
                if let Some(pc) = pic.as_mut()
                    && pc.rid.is_none()
                {
                    pc.rid = attr(&a, "embed").or(attr(&a, "id")).map(str::to_string);
                    if pc.alt.is_empty()
                        && let Some(t) = attr(&a, "title")
                    {
                        pc.alt = t.to_string();
                    }
                }
            }
            "shape" => {
                // VML pictures give their size in the style: `width:120pt;height:…`.
                if let Some(pc) = pic.as_mut()
                    && pc.width.is_none()
                {
                    pc.width = attr(&a, "style").and_then(|st| {
                        st.split(';').find_map(|d| {
                            let (k, v) = d.split_once(':')?;
                            (k.trim() == "width").then(|| length_pt(v)).flatten()
                        })
                    });
                }
            }
            "del" | "instrText" | "drawing" | "pict" | "object" | "txbxContent"
            | "footnoteReference" => skip += 1,
            _ if skip > 0 => {}
            "p" => p = P::default(),
            "pStyle" => p.style = attr(&a, "val").unwrap_or_default().to_string(),
            "numId" => {
                let id = attr(&a, "val").unwrap_or_default().to_string();
                if id != "0" {
                    p.num = Some((id, p.num.as_ref().map_or(0, |n| n.1)));
                }
            }
            "ilvl" => {
                let l = attr(&a, "val").and_then(|v| v.parse().ok()).unwrap_or(0);
                p.num = Some((p.num.take().map(|n| n.0).unwrap_or_default(), l));
            }
            "r" => {
                bold = false;
                italic = false;
            }
            "rPr" => in_rpr = true,
            "b" if in_rpr => bold = on(attr(&a, "val")),
            "i" if in_rpr => italic = on(attr(&a, "val")),
            "t" => in_t = true,
            "tab" if !in_rpr => b.text("\t"),
            "br" | "cr" => {
                if attr(&a, "type") == Some("page") {
                    p.page_break = true;
                } else {
                    b.text("\n");
                }
            }
            "hyperlink" => link = attr(&a, "id").and_then(|id| rels.get(id).cloned()),
            "tbl" => {
                table_depth += 1;
                if table_depth == 1 {
                    b.open_table();
                }
            }
            "tc" if table_depth == 1 => {
                b.open_cell();
                cell_has_text = false;
            }
            _ => {}
        },
        X::End(n) => match n.as_str() {
            "drawing" | "pict" | "object" if skip == 1 && pic.is_some() => {
                skip = 0;
                if let Some(pc) = pic.take()
                    && let Some(path) = pc.rid.as_ref().and_then(|r| inside.get(r))
                    && let Some(bytes) = pics.read_zip(&mut z, path)
                {
                    b.image(picture(bytes, &pc.alt, pc.width));
                }
            }
            "del" | "instrText" | "drawing" | "pict" | "object" | "txbxContent"
            | "footnoteReference" => skip = skip.saturating_sub(1),
            _ if skip > 0 => {}
            "rPr" => in_rpr = false,
            "t" => in_t = false,
            "hyperlink" => link = None,
            "p" => {
                if b.in_table_cell() {
                    if cell_has_text {
                        b.text(" ");
                    }
                } else if let Some(level) = heading_of.get(&p.style).copied().or_else(|| {
                    let s = p.style.to_ascii_lowercase();
                    s.strip_prefix("heading")
                        .and_then(|d| d.parse().ok())
                        .or((s == "title").then_some(1))
                }) {
                    b.heading(level);
                } else if let Some((id, lvl)) = p.num.as_ref() {
                    b.list_para(
                        *lvl,
                        ordered_of
                            .get(&(id.clone(), *lvl))
                            .copied()
                            .unwrap_or(false),
                    );
                } else {
                    b.para();
                }
                if p.page_break {
                    b.block(Block::PageBreak);
                }
            }
            "tc" if table_depth == 1 => b.close_cell(),
            "tr" if table_depth == 1 => b.close_row(),
            "tbl" => {
                if table_depth == 1 {
                    b.close_table();
                }
                table_depth = table_depth.saturating_sub(1);
            }
            _ => {}
        },
        X::Text(t) if in_t && skip == 0 => {
            cell_has_text = true;
            let l = link.clone();
            b.push_style(|s| {
                s.bold = bold;
                s.italic = italic;
                s.link = l;
            });
            b.text(&t);
            b.pop_style();
        }
        X::Text(_) => {}
    })?;
    Ok(Doc {
        title: None,
        blocks: b.finish(),
    })
}

// ---------------------------------------------------------------- ODT

pub fn odt(bytes: &[u8]) -> Result<Doc, EngineError> {
    let mut z = open_zip(bytes)?;
    let content = part(&mut z, "content.xml")?
        .ok_or_else(|| EngineError::Decode("not an OpenDocument text (no content.xml)".into()))?;
    let styles_xml = part(&mut z, "styles.xml")?.unwrap_or_default();

    // Text styles (bold/italic) and list styles (numbered?) from both style sheets.
    let mut text_style: HashMap<String, (bool, bool)> = HashMap::new();
    let mut numbered: HashMap<String, bool> = HashMap::new();
    for src in [&styles_xml, &content] {
        let (mut cur, mut cur_list, mut first_level) = (String::new(), String::new(), false);
        xml_events(src, |x| {
            if let X::Start(n, a) = x {
                match n.as_str() {
                    "style" => cur = attr(&a, "name").unwrap_or_default().to_string(),
                    "text-properties" if !cur.is_empty() => {
                        let e = text_style.entry(cur.clone()).or_default();
                        e.0 |= attr(&a, "font-weight").is_some_and(|w| {
                            w == "bold" || w.parse::<u32>().is_ok_and(|n| n >= 600)
                        });
                        e.1 |=
                            attr(&a, "font-style").is_some_and(|s| s == "italic" || s == "oblique");
                    }
                    "list-style" => {
                        cur_list = attr(&a, "name").unwrap_or_default().to_string();
                        first_level = true;
                    }
                    "list-level-style-number" | "list-level-style-bullet" if first_level => {
                        numbered.insert(cur_list.clone(), n == "list-level-style-number");
                        first_level = false;
                    }
                    _ => {}
                }
            }
        })?;
    }

    let mut b = Builder::new();
    let mut in_body = false;
    let mut skip = 0usize;
    let mut heading: Vec<u8> = Vec::new();
    let mut list_styles: Vec<Option<String>> = Vec::new();
    let mut spans: Vec<bool> = Vec::new(); // whether each open span/link pushed a style
    let mut table_depth = 0usize;
    let mut pics = Pics::new();
    // The frame being read: its picture files (first usable wins), title and width.
    #[derive(Default)]
    struct Frame {
        hrefs: Vec<String>,
        alt: String,
        width: Option<f32>,
        in_title: bool,
    }
    let mut frame: Option<Frame> = None;
    xml_events(&content, |x| match x {
        X::Start(n, a) => match n.as_str() {
            "text" if !in_body => in_body = true,
            _ if !in_body => {}
            "frame" if skip == 0 => {
                frame = Some(Frame {
                    width: attr(&a, "width").and_then(length_pt),
                    ..Frame::default()
                });
                skip += 1;
            }
            "image" => {
                if let Some(f) = frame.as_mut()
                    && let Some(h) = attr(&a, "href")
                    && !h.contains("://")
                    && !h.starts_with('/')
                {
                    f.hrefs.push(normalize(&percent_decode(h)));
                }
            }
            "title" | "desc" => {
                if let Some(f) = frame.as_mut() {
                    f.in_title = f.alt.is_empty() || n == "title";
                }
            }
            "note" | "annotation" | "frame" | "tracked-changes" | "sequence-decls" => skip += 1,
            _ if skip > 0 => {}
            "h" => {
                b.para();
                heading.push(
                    attr(&a, "outline-level")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1),
                );
            }
            "p" => {
                if !b.in_table_cell() {
                    b.para();
                }
            }
            "list" => {
                let style = attr(&a, "style-name")
                    .map(str::to_string)
                    .or_else(|| list_styles.last().cloned().flatten());
                let ordered = style
                    .as_ref()
                    .and_then(|s| numbered.get(s))
                    .copied()
                    .unwrap_or(false);
                list_styles.push(style);
                b.open_list(ordered);
            }
            "list-item" => b.open_item(),
            "span" => {
                let st = attr(&a, "style-name")
                    .and_then(|s| text_style.get(s))
                    .copied();
                spans.push(st.is_some());
                if let Some((bo, it)) = st {
                    b.push_style(|s| {
                        s.bold |= bo;
                        s.italic |= it;
                    });
                }
            }
            "a" => {
                let href = attr(&a, "href").map(str::to_string);
                spans.push(href.is_some());
                if let Some(h) = href {
                    b.push_style(|s| s.link = Some(h));
                }
            }
            "line-break" => b.text("\n"),
            "tab" => b.text("\t"),
            "s" => b.text(
                &" ".repeat(
                    attr(&a, "c")
                        .and_then(|c| c.parse().ok())
                        .unwrap_or(1usize)
                        .min(64),
                ),
            ),
            "table" => {
                table_depth += 1;
                if table_depth == 1 {
                    b.open_table();
                }
            }
            "table-header-rows" if table_depth == 1 => b.set_table_header(),
            "table-cell" if table_depth == 1 => b.open_cell(),
            _ => {}
        },
        X::End(n) => match n.as_str() {
            _ if !in_body => {}
            "frame" if skip == 1 && frame.is_some() => {
                skip = 0;
                if let Some(f) = frame.take()
                    && let Some(bytes) = f.hrefs.iter().find_map(|h| pics.read_zip(&mut z, h))
                {
                    b.image(picture(bytes, &f.alt, f.width));
                }
            }
            "title" | "desc" => {
                if let Some(f) = frame.as_mut() {
                    f.in_title = false;
                }
            }
            "note" | "annotation" | "frame" | "tracked-changes" | "sequence-decls" => {
                skip = skip.saturating_sub(1)
            }
            _ if skip > 0 => {}
            "text" => in_body = false,
            "h" => b.heading(heading.pop().unwrap_or(1)),
            "p" => {
                if b.in_table_cell() {
                    b.text(" ");
                } else {
                    b.para();
                }
            }
            "list" => {
                list_styles.pop();
                b.close_list();
            }
            "list-item" => b.close_item(),
            "span" | "a" => {
                if spans.pop() == Some(true) {
                    b.pop_style();
                }
            }
            "table-cell" if table_depth == 1 => b.close_cell(),
            "table-row" if table_depth == 1 => b.close_row(),
            "table" => {
                if table_depth == 1 {
                    b.close_table();
                }
                table_depth = table_depth.saturating_sub(1);
            }
            _ => {}
        },
        X::Text(t) if in_body && skip == 0 => b.text(&t),
        X::Text(t) => {
            if let Some(f) = frame.as_mut()
                && f.in_title
            {
                f.alt.push_str(&t);
            }
        }
    })?;
    Ok(Doc {
        title: None,
        blocks: b.finish(),
    })
}

// ---------------------------------------------------------------- EPUB

pub fn epub(bytes: &[u8]) -> Result<Doc, EngineError> {
    let mut z = open_zip(bytes)?;
    let container = part(&mut z, "META-INF/container.xml")?
        .ok_or_else(|| EngineError::Decode("not an EPUB (no container.xml)".into()))?;
    let mut opf_path = None;
    xml_events(&container, |x| {
        if let X::Start(n, a) = x
            && n == "rootfile"
            && opf_path.is_none()
        {
            opf_path = attr(&a, "full-path").map(str::to_string);
        }
    })?;
    let opf_path =
        opf_path.ok_or_else(|| EngineError::Decode("EPUB has no package file".into()))?;
    let opf = part(&mut z, &opf_path)?
        .ok_or_else(|| EngineError::Decode("EPUB package file is missing".into()))?;
    let base = opf_path.rsplit_once('/').map_or("", |(d, _)| d).to_string();

    let (mut manifest, mut spine, mut title, mut in_title) =
        (HashMap::new(), Vec::new(), String::new(), false);
    xml_events(&opf, |x| match x {
        X::Start(n, a) => match n.as_str() {
            "item" => {
                if let (Some(id), Some(href)) = (attr(&a, "id"), attr(&a, "href")) {
                    manifest.insert(id.to_string(), href.to_string());
                }
            }
            "itemref" => {
                if let Some(id) = attr(&a, "idref") {
                    spine.push(id.to_string());
                }
            }
            "title" => in_title = title.is_empty(),
            _ => {}
        },
        X::End(n) if n == "title" => in_title = false,
        X::Text(t) if in_title => title.push_str(&t),
        _ => {}
    })?;

    let mut b = Builder::new();
    let mut pics = Pics::new();
    let mut first = true;
    for id in spine {
        let Some(href) = manifest.get(&id) else {
            continue;
        };
        let href = percent_decode(href.split('#').next().unwrap_or(href));
        let path = if base.is_empty() {
            href
        } else {
            format!("{base}/{href}")
        };
        let Some(xhtml) = part(&mut z, &normalize(&path))? else {
            continue;
        };
        if !first {
            b.block(Block::PageBreak);
        }
        first = false;
        // Pictures are resolved against the chapter's folder, inside the book only.
        let dir = normalize(&path)
            .rsplit_once('/')
            .map_or(String::new(), |(d, _)| d.to_string());
        let mut inside = |src: &str| -> Option<Vec<u8>> {
            if src.contains("://") || src.starts_with('/') {
                return None;
            }
            let rel = percent_decode(src.split(['#', '?']).next().unwrap_or(src));
            let name = if dir.is_empty() {
                normalize(&rel)
            } else {
                normalize(&format!("{dir}/{rel}"))
            };
            let f = z.by_name(&name).ok()?;
            if f.size() > PICTURE_LIMIT {
                return None;
            }
            let mut v = Vec::new();
            f.take(PICTURE_LIMIT).read_to_end(&mut v).ok()?;
            Some(v)
        };
        b = html_into(&xhtml, b, &mut pics, &mut inside).0;
    }
    let title = Some(title.trim().to_string()).filter(|t| !t.is_empty());
    Ok(Doc {
        title,
        blocks: b.finish(),
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Some(v) = std::str::from_utf8(&bytes[i + 1..(i + 3).min(bytes.len())])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Resolve `a/b/../c` inside the archive.
fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            c => parts.push(c),
        }
    }
    parts.join("/")
}

// ---------------------------------------------------------------- RTF

pub fn rtf(bytes: &[u8]) -> Doc {
    #[derive(Clone, Copy)]
    struct St {
        skip: bool,
        bold: bool,
        italic: bool,
        uc: usize,
    }
    const SKIP: &[&str] = &[
        "fonttbl",
        "colortbl",
        "stylesheet",
        "info",
        "pict",
        "object",
        "header",
        "headerl",
        "headerr",
        "headerf",
        "footer",
        "footerl",
        "footerr",
        "footerf",
        "footnote",
        "fldinst",
        "listtext",
        "pntext",
        "pntxta",
        "pntxtb",
        "themedata",
        "colorschememapping",
        "latentstyles",
        "datastore",
        "xmlnstbl",
        "rsidtbl",
        "generator",
        "mmathPr",
        "listtable",
        "listoverridetable",
        "pgdsctbl",
        "revtbl",
        "filetbl",
        "bkmkstart",
        "bkmkend",
        "xmlopen",
        "wgrffmtfilter",
        "shpinst",
        "nonshppict",
        "annotation",
        "atnid",
        "atnauthor",
    ];
    let mut b = Builder::new();
    let mut pics = Pics::new();
    let mut st = St {
        skip: false,
        bold: false,
        italic: false,
        uc: 1,
    };
    let mut stack: Vec<St> = Vec::new();
    let mut buf = String::new();
    let mut skip_chars = 0usize;
    let mut group_start = false;
    let flush = |b: &mut Builder, buf: &mut String, st: &St| {
        if !buf.is_empty() {
            let (bo, it) = (st.bold, st.italic);
            b.push_style(|s| {
                s.bold = bo;
                s.italic = it;
            });
            b.text(buf);
            b.pop_style();
            buf.clear();
        }
    };
    let emit = |c: char, b: &mut Builder, buf: &mut String, st: &St, skip_chars: &mut usize| {
        if *skip_chars > 0 {
            *skip_chars -= 1;
        } else if !st.skip {
            let _ = b;
            buf.push(c);
        }
    };
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'{' => {
                flush(&mut b, &mut buf, &st);
                stack.push(st);
                group_start = true;
                i += 1;
                continue;
            }
            b'}' => {
                flush(&mut b, &mut buf, &st);
                if let Some(s) = stack.pop() {
                    st = s;
                }
                i += 1;
            }
            b'\\' => {
                let Some(&n) = bytes.get(i + 1) else { break };
                if n.is_ascii_alphabetic() {
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                    let word = std::str::from_utf8(&bytes[i + 1..j]).unwrap_or("");
                    let mut k = j;
                    if k < bytes.len() && (bytes[k] == b'-' || bytes[k].is_ascii_digit()) {
                        k += 1;
                        while k < bytes.len() && bytes[k].is_ascii_digit() {
                            k += 1;
                        }
                    }
                    let param: Option<i32> = std::str::from_utf8(&bytes[j..k])
                        .ok()
                        .and_then(|p| p.parse().ok());
                    if k < bytes.len() && bytes[k] == b' ' {
                        k += 1;
                    }
                    i = k;
                    if group_start && word == "pict" && !st.skip {
                        // The rest of the group, up to its closing brace.
                        let end = group_end(bytes, k);
                        flush(&mut b, &mut buf, &st);
                        if let Some(p) = rtf_picture(&bytes[k..end], &mut pics) {
                            b.image(p);
                        }
                        // The closing brace is handled by the main loop (it pops the state).
                        i = end;
                        group_start = false;
                        continue;
                    }
                    if group_start && SKIP.contains(&word) {
                        st.skip = true;
                    }
                    group_start = false;
                    match word {
                        "par" | "sect" | "row" => {
                            flush(&mut b, &mut buf, &st);
                            if !st.skip {
                                b.para();
                            }
                        }
                        "line" => emit('\n', &mut b, &mut buf, &st, &mut skip_chars),
                        "tab" | "cell" => emit('\t', &mut b, &mut buf, &st, &mut skip_chars),
                        "page" => {
                            flush(&mut b, &mut buf, &st);
                            if !st.skip {
                                b.block(Block::PageBreak);
                            }
                        }
                        "b" => {
                            flush(&mut b, &mut buf, &st);
                            st.bold = param != Some(0);
                        }
                        "i" => {
                            flush(&mut b, &mut buf, &st);
                            st.italic = param != Some(0);
                        }
                        "plain" => {
                            flush(&mut b, &mut buf, &st);
                            st.bold = false;
                            st.italic = false;
                        }
                        "uc" => st.uc = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            if let Some(p) = param {
                                let cp = if p < 0 { (p + 65536) as u32 } else { p as u32 };
                                if let Some(ch) = char::from_u32(cp) {
                                    emit(ch, &mut b, &mut buf, &st, &mut 0);
                                }
                                skip_chars = st.uc;
                            }
                        }
                        "emdash" => emit('—', &mut b, &mut buf, &st, &mut skip_chars),
                        "endash" => emit('–', &mut b, &mut buf, &st, &mut skip_chars),
                        "bullet" => emit('•', &mut b, &mut buf, &st, &mut skip_chars),
                        "lquote" => emit('‘', &mut b, &mut buf, &st, &mut skip_chars),
                        "rquote" => emit('’', &mut b, &mut buf, &st, &mut skip_chars),
                        "ldblquote" => emit('“', &mut b, &mut buf, &st, &mut skip_chars),
                        "rdblquote" => emit('”', &mut b, &mut buf, &st, &mut skip_chars),
                        _ => {}
                    }
                    continue;
                }
                group_start = false;
                match n {
                    b'\'' => {
                        let hex = bytes
                            .get(i + 2..i + 4)
                            .and_then(|h| std::str::from_utf8(h).ok())
                            .and_then(|h| u8::from_str_radix(h, 16).ok());
                        if let Some(v) = hex {
                            let byte = [v];
                            let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&byte);
                            for ch in s.chars() {
                                emit(ch, &mut b, &mut buf, &st, &mut skip_chars);
                            }
                        }
                        i += 4;
                    }
                    b'*' => {
                        // `{\*\shppict{\pict …}}` is Word's picture; every other optional
                        // destination (including the `\nonshppict` copy) is skipped.
                        if !bytes[i + 2..].starts_with(b"\\shppict") {
                            st.skip = true;
                        }
                        i += 2;
                    }
                    b'~' => {
                        emit('\u{a0}', &mut b, &mut buf, &st, &mut skip_chars);
                        i += 2;
                    }
                    b'_' => {
                        emit('-', &mut b, &mut buf, &st, &mut skip_chars);
                        i += 2;
                    }
                    b'\n' | b'\r' => {
                        flush(&mut b, &mut buf, &st);
                        if !st.skip {
                            b.para();
                        }
                        i += 2;
                    }
                    b'-' => i += 2,
                    other => {
                        emit(other as char, &mut b, &mut buf, &st, &mut skip_chars);
                        i += 2;
                    }
                }
                continue;
            }
            b'\r' | b'\n' => i += 1,
            _ => {
                // Plain bytes: ASCII, or Windows-1252 for the high half.
                let ch = if c < 0x80 {
                    c as char
                } else {
                    encoding_rs::WINDOWS_1252
                        .decode(&[c])
                        .0
                        .chars()
                        .next()
                        .unwrap_or('?')
                };
                emit(ch, &mut b, &mut buf, &st, &mut skip_chars);
                i += 1;
            }
        }
        group_start = false;
    }
    flush(&mut b, &mut buf, &st);
    Doc {
        title: None,
        blocks: b.finish(),
    }
}

// ---------------------------------------------------------------- PDF (text)

pub fn pdf(bytes: &[u8]) -> Result<Doc, EngineError> {
    // The layout engine finds headings, lists and tables in reading order (docs/13).
    let l = crate::pdflayout::analyze(bytes, &swirvo_core::CancelToken::new())?;
    if l.has_text {
        return Ok(crate::pdflayout::to_doc(&l));
    }
    if crate::ocr::available() {
        return ocr_pdf(bytes);
    }
    Err(EngineError::Decode(
        "this PDF has no text layer (it's probably scanned), and this PC has no OCR engine".into(),
    ))
}

/// Scanned PDF: render each page and recognize its text (docs/12 §8). Lines are joined into
/// paragraphs; a short line ending a sentence closes its paragraph.
fn ocr_pdf(bytes: &[u8]) -> Result<Doc, EngineError> {
    const MAX_PAGES: usize = 300;
    let r = crate::pdf_tools::PageRenderer::from_bytes(bytes.to_vec())?;
    let n = r.count();
    if n > MAX_PAGES {
        return Err(EngineError::Invalid(format!(
            "this scanned PDF has {n} pages; OCR handles up to {MAX_PAGES} at a time"
        )));
    }
    let mut b = Builder::new();
    let mut any = false;
    r.session(|s| -> Result<(), EngineError> {
        for i in 0..n {
            if i > 0 {
                b.block(Block::PageBreak);
            }
            let img = s.render_dpi(i, 200.0)?;
            let text = crate::ocr::recognize(&img)?;
            let lines: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect();
            let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
            let mut para = String::new();
            for l in &lines {
                any = true;
                if !para.is_empty() {
                    para.push(' ');
                }
                para.push_str(l);
                let ends = l.ends_with(['.', '!', '?', ':']);
                if ends && l.chars().count() * 10 < longest * 7 {
                    b.text(&para);
                    b.para();
                    para.clear();
                }
            }
            if !para.is_empty() {
                b.text(&para);
                b.para();
            }
        }
        Ok(())
    })?;
    if !any {
        return Err(EngineError::Decode(
            "no text found in this PDF, even with OCR".into(),
        ));
    }
    Ok(Doc {
        title: None,
        blocks: b.finish(),
    })
}

// ---------------------------------------------------------------- tables

pub fn delimited(src: &str, from: &str) -> Result<Doc, EngineError> {
    let delim = if from == "tsv" {
        b'\t'
    } else {
        crate::data::sniff_delimiter(src)
    };
    let mut r = csv::ReaderBuilder::new()
        .delimiter(delim)
        .has_headers(false)
        .flexible(true)
        .from_reader(src.as_bytes());
    let rows: Vec<Vec<String>> = r
        .records()
        .map(|rec| rec.map(|r| r.iter().map(str::to_string).collect()))
        .collect::<Result<_, _>>()
        .map_err(EngineError::decode)?;
    Ok(table_doc(rows))
}

fn sheet(path: &Path) -> Result<Doc, EngineError> {
    let rows = crate::sheet::read_sheet(path)?;
    Ok(table_doc(
        rows.iter()
            .map(|r| r.iter().map(crate::sheet::cell_text).collect())
            .collect(),
    ))
}

fn table_doc(mut rows: Vec<Vec<String>>) -> Doc {
    while rows
        .last()
        .is_some_and(|r| r.iter().all(|c| c.trim().is_empty()))
    {
        rows.pop();
    }
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    for r in &mut rows {
        r.resize(width, String::new());
    }
    let blocks = if rows.is_empty() {
        Vec::new()
    } else {
        vec![Block::Table {
            rows,
            header: true,
            pictures: Vec::new(),
        }]
    };
    Doc {
        title: None,
        blocks,
    }
}
