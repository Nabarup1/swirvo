//! Writers: [`Doc`] → Markdown, HTML, TXT, DOCX, ODT, EPUB (PDF lives in `pdf.rs`).

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use swirvo_core::EngineError;
use zip::write::SimpleFileOptions;

use super::picture::{Media, place};
use super::{Block, Doc, Inline, Picture, cell_pictures, plain};

pub fn write_path(doc: &Doc, to: &str, out: &Path) -> Result<(), EngineError> {
    let fallback = out
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match to {
        "md" => Ok(std::fs::write(out, markdown(doc))?),
        "html" => Ok(std::fs::write(out, html(doc, &fallback))?),
        "txt" => Ok(std::fs::write(out, text(doc))?),
        "docx" => docx(doc, &fallback, out),
        "odt" => odt(doc, &fallback, out),
        "epub" => epub(doc, &fallback, out),
        "pdf" => super::pdf::write(doc, &fallback, out),
        other => Err(EngineError::Invalid(format!(
            "document engine can't write {other}"
        ))),
    }
}

// ---------------------------------------------------------------- Markdown

fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn md_inlines(inl: &[Inline]) -> String {
    let mut out = String::new();
    for i in inl {
        let body = if i.style.code {
            format!("`{}`", i.text.replace('`', "'"))
        } else {
            md_escape(&i.text)
        };
        // Keep surrounding spaces outside the markers (`** bold**` isn't bold).
        let lead = &body[..body.len() - body.trim_start().len()];
        let trail = &body[body.trim_end().len()..];
        let core = body.trim();
        if core.is_empty() {
            out.push_str(&body);
            continue;
        }
        let mut core = core.to_string();
        if i.style.italic && !i.style.code {
            core = format!("*{core}*");
        }
        if i.style.bold && !i.style.code {
            core = format!("**{core}**");
        }
        if let Some(l) = &i.style.link {
            core = format!("[{core}]({})", l.replace(' ', "%20").replace(')', "%29"));
        }
        out.push_str(lead);
        out.push_str(&core);
        out.push_str(trail);
    }
    out.replace('\n', "  \n")
}

/// Pictures up to this size (as written) go inside a Markdown file as data URIs; bigger
/// ones are left out with a note. The writer makes one file: it can't put pictures in a
/// folder beside the Markdown, because it only sees a temporary output that's renamed
/// into place afterwards.
pub(crate) const MD_PICTURE: usize = 256 << 10;

/// `![alt](data:…)`, or a note when the picture is too big or can't be read.
fn md_picture(p: &Picture) -> String {
    let alt: String = p
        .alt
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect::<String>()
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    match web_picture(p) {
        Some((mime, _, bytes)) if bytes.len() <= MD_PICTURE => {
            format!(
                "![{alt}](data:{mime};base64,{})",
                crate::emf::base64(&bytes)
            )
        }
        Some(_) if alt.is_empty() => "*(picture left out: too large for Markdown)*".into(),
        Some(_) => format!("*(picture left out: too large for Markdown: {alt})*"),
        None => String::new(),
    }
}

fn md_blocks(blocks: &[Block], indent: &str, out: &mut String) {
    for b in blocks {
        match b {
            Block::Heading(l, i) => {
                let _ = writeln!(
                    out,
                    "{indent}{} {}\n",
                    "#".repeat(*l as usize),
                    md_inlines(i).replace("  \n", " ")
                );
            }
            Block::Para(i) => {
                let _ = writeln!(
                    out,
                    "{indent}{}\n",
                    md_inlines(i).replace('\n', &format!("\n{indent}"))
                );
            }
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", n + 1)
                    } else {
                        "- ".to_string()
                    };
                    let mut inner = String::new();
                    md_blocks(item, "", &mut inner);
                    let pad = " ".repeat(marker.len());
                    let body = inner.trim_end().replace("\n\n", "\n");
                    for (k, line) in body.lines().enumerate() {
                        if k == 0 {
                            let _ = writeln!(out, "{indent}{marker}{line}");
                        } else {
                            let _ = writeln!(out, "{indent}{pad}{line}");
                        }
                    }
                }
                out.push('\n');
            }
            Block::Quote(inner) => {
                let mut q = String::new();
                md_blocks(inner, "", &mut q);
                for line in q.trim_end().lines() {
                    let _ = writeln!(out, "{indent}> {line}");
                }
                out.push('\n');
            }
            Block::Code(c) => {
                let fence = if c.contains("```") { "~~~~" } else { "```" };
                let _ = writeln!(out, "{indent}{fence}\n{c}\n{fence}\n");
            }
            Block::Table {
                rows,
                header,
                pictures,
            } => {
                let rows: Vec<Vec<String>> = rows
                    .iter()
                    .enumerate()
                    .map(|(r, row)| {
                        row.iter()
                            .enumerate()
                            .map(|(c, text)| {
                                let mut t = text.replace('|', "\\|").replace('\n', " ");
                                for p in cell_pictures(pictures, r, c) {
                                    let m = md_picture(p);
                                    if !m.is_empty() {
                                        if !t.is_empty() {
                                            t.push(' ');
                                        }
                                        t.push_str(&m);
                                    }
                                }
                                t
                            })
                            .collect()
                    })
                    .collect();
                let cell = |s: &str| s.to_string();
                let width = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
                let (head, body): (Vec<String>, &[Vec<String>]) = if *header {
                    (rows[0].clone(), &rows[1..])
                } else {
                    (vec![String::new(); width], &rows[..])
                };
                let line = |r: &[String]| {
                    let mut s = String::from("|");
                    for c in 0..width {
                        let _ = write!(s, " {} |", cell(r.get(c).map_or("", String::as_str)));
                    }
                    s
                };
                let _ = writeln!(out, "{indent}{}", line(&head));
                let _ = writeln!(out, "{indent}|{}", " --- |".repeat(width));
                for r in body {
                    let _ = writeln!(out, "{indent}{}", line(r));
                }
                out.push('\n');
            }
            Block::Rule => {
                let _ = writeln!(out, "{indent}---\n");
            }
            Block::Image(p) => {
                let m = md_picture(p);
                if !m.is_empty() {
                    let _ = writeln!(out, "{indent}{m}\n");
                }
            }
            Block::PageBreak => {}
        }
    }
}

pub fn markdown(doc: &Doc) -> String {
    let mut out = String::new();
    md_blocks(&doc.blocks, "", &mut out);
    out.trim_end().to_string() + "\n"
}

// ---------------------------------------------------------------- plain text

fn text_blocks(blocks: &[Block], indent: &str, out: &mut String) {
    for b in blocks {
        match b {
            Block::Heading(_, i) | Block::Para(i) => {
                let _ = writeln!(
                    out,
                    "{indent}{}\n",
                    plain(i).replace('\n', &format!("\n{indent}"))
                );
            }
            Block::List { ordered, items } => {
                for (n, item) in items.iter().enumerate() {
                    let marker = if *ordered {
                        format!("{}. ", n + 1)
                    } else {
                        "• ".to_string()
                    };
                    let mut inner = String::new();
                    text_blocks(item, "", &mut inner);
                    let pad = " ".repeat(marker.chars().count());
                    for (k, line) in inner.trim_end().replace("\n\n", "\n").lines().enumerate() {
                        let _ =
                            writeln!(out, "{indent}{}{line}", if k == 0 { &marker } else { &pad });
                    }
                }
                out.push('\n');
            }
            Block::Quote(inner) => {
                let mut q = String::new();
                text_blocks(inner, "", &mut q);
                for line in q.trim_end().lines() {
                    let _ = writeln!(out, "{indent}  {line}");
                }
                out.push('\n');
            }
            Block::Code(c) => {
                for line in c.lines() {
                    let _ = writeln!(out, "{indent}    {line}");
                }
                out.push('\n');
            }
            Block::Table { rows, .. } => {
                // Aligned columns, capped so one huge cell doesn't push everything off-screen.
                let width = rows.iter().map(Vec::len).max().unwrap_or(0);
                let w: Vec<usize> = (0..width)
                    .map(|c| {
                        rows.iter()
                            .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                            .max()
                            .unwrap_or(0)
                            .min(40)
                    })
                    .collect();
                for r in rows {
                    let mut line = String::from(indent);
                    for (c, cw) in w.iter().enumerate() {
                        let s = r.get(c).map_or("", String::as_str).replace('\n', " ");
                        let _ = write!(line, "{s:<cw$}  ");
                    }
                    let _ = writeln!(out, "{}", line.trim_end());
                }
                out.push('\n');
            }
            Block::Rule => out.push_str("----------------------------------------\n\n"),
            Block::PageBreak | Block::Image(_) => {}
        }
    }
}

pub fn text(doc: &Doc) -> String {
    let mut out = String::new();
    text_blocks(&doc.blocks, "", &mut out);
    out.trim_end().to_string() + "\n"
}

// ---------------------------------------------------------------- HTML / XHTML

/// Escape for XML/HTML text and attributes; drops characters XML 1.0 forbids.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}

fn html_inlines(inl: &[Inline]) -> String {
    let mut out = String::new();
    for i in inl {
        let mut s = esc(&i.text).replace('\n', "<br/>");
        if i.style.code {
            s = format!("<code>{s}</code>");
        }
        if i.style.italic {
            s = format!("<em>{s}</em>");
        }
        if i.style.bold {
            s = format!("<strong>{s}</strong>");
        }
        if let Some(l) = &i.style.link {
            s = format!("<a href=\"{}\">{s}</a>", esc(l));
        }
        out.push_str(&s);
    }
    out
}

/// Body markup. XHTML-compatible (self-closed void elements) so EPUB can reuse it.
/// Where a picture goes in a page: a `src` for it, or None to leave it out.
type PicSink<'a> = dyn FnMut(&Picture) -> Option<String> + 'a;

/// A picture as browsers and e-book readers show it: MIME type, file extension and
/// bytes. PNG, JPEG, GIF, WebP and SVG are kept as they are; other bitmaps become PNG and
/// EMF/WMF become SVG (vectors stay vectors). None when it can't be read.
pub(crate) fn web_picture(p: &Picture) -> Option<(&'static str, &'static str, Vec<u8>)> {
    use image::ImageFormat as F;
    let b = p.bytes.as_slice();
    match image::guess_format(b) {
        Ok(F::Png) => return Some(("image/png", "png", b.to_vec())),
        Ok(F::Jpeg) => return Some(("image/jpeg", "jpg", b.to_vec())),
        Ok(F::Gif) => return Some(("image/gif", "gif", b.to_vec())),
        Ok(F::WebP) => return Some(("image/webp", "webp", b.to_vec())),
        Ok(_) => {
            let img = image::load_from_memory(b).ok()?;
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), F::Png)
                .ok()?;
            return Some(("image/png", "png", png));
        }
        Err(_) => {}
    }
    if is_svg(b) {
        return Some(("image/svg+xml", "svg", b.to_vec()));
    }
    if crate::emf::is_metafile(b) {
        let svg = crate::emf::to_svg_string(b)?;
        return Some(("image/svg+xml", "svg", svg.into_bytes()));
    }
    None
}

pub(crate) fn is_svg(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(4096)]);
    let t = head.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<svg") || t.starts_with("<?xml") || t.starts_with("<!--"))
        && head.contains("<svg")
}

/// `<img …/>`, sized as placed in the source (the style sheet keeps it within the page).
fn html_img(p: &Picture, src: &str) -> String {
    let style = p
        .width_pt
        .map(|w| format!(" style=\"width:{w}pt\""))
        .unwrap_or_default();
    format!("<img src=\"{}\" alt=\"{}\"{style}/>", esc(src), esc(&p.alt))
}

/// `<figure><img …/></figure>`, sized as placed in the source but never wider than the page.
fn html_picture(p: &Picture, src: &str, out: &mut String) {
    let _ = writeln!(out, "<figure>{}</figure>", html_img(p, src));
}

fn html_blocks(blocks: &[Block], out: &mut String, ids: &mut usize, pics: &mut PicSink<'_>) {
    for b in blocks {
        match b {
            Block::Heading(l, i) => {
                *ids += 1;
                let _ = writeln!(out, "<h{l} id=\"h{ids}\">{}</h{l}>", html_inlines(i));
            }
            Block::Para(i) => {
                let _ = writeln!(out, "<p>{}</p>", html_inlines(i));
            }
            Block::List { ordered, items } => {
                let tag = if *ordered { "ol" } else { "ul" };
                let _ = writeln!(out, "<{tag}>");
                for item in items {
                    out.push_str("<li>");
                    // A single paragraph renders inline (tight list).
                    match item.as_slice() {
                        [Block::Para(i)] => out.push_str(&html_inlines(i)),
                        [Block::Para(i), rest @ ..] => {
                            out.push_str(&html_inlines(i));
                            out.push('\n');
                            html_blocks(rest, out, ids, pics);
                        }
                        other => html_blocks(other, out, ids, pics),
                    }
                    out.push_str("</li>\n");
                }
                let _ = writeln!(out, "</{tag}>");
            }
            Block::Quote(inner) => {
                out.push_str("<blockquote>\n");
                html_blocks(inner, out, ids, pics);
                out.push_str("</blockquote>\n");
            }
            Block::Code(c) => {
                let _ = writeln!(out, "<pre><code>{}</code></pre>", esc(c));
            }
            Block::Table {
                rows,
                header,
                pictures,
            } => {
                out.push_str("<table>\n");
                for (r, row) in rows.iter().enumerate() {
                    let cell = if *header && r == 0 { "th" } else { "td" };
                    out.push_str("<tr>");
                    for (c, text) in row.iter().enumerate() {
                        let mut body = esc(text).replace('\n', "<br/>");
                        for p in cell_pictures(pictures, r, c) {
                            if let Some(src) = pics(p) {
                                if !body.is_empty() {
                                    body.push_str("<br/>");
                                }
                                body.push_str(&html_img(p, &src));
                            }
                        }
                        let _ = write!(out, "<{cell}>{body}</{cell}>");
                    }
                    out.push_str("</tr>\n");
                }
                out.push_str("</table>\n");
            }
            Block::Rule => out.push_str("<hr/>\n"),
            Block::PageBreak => out.push_str("<div class=\"page-break\"></div>\n"),
            Block::Image(p) => {
                if let Some(src) = pics(p) {
                    html_picture(p, &src, out);
                }
            }
        }
    }
}

const CSS: &str = "body{font:16px/1.6 system-ui,-apple-system,'Segoe UI',Roboto,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem;color:#1a1c21;background:#fff}\
h1,h2,h3{line-height:1.25}pre{background:#f4f4f6;padding:.75rem 1rem;overflow:auto;border-radius:6px}code{font-family:ui-monospace,Consolas,monospace;font-size:.92em}\
blockquote{border-left:3px solid #d0d3da;margin:1rem 0;padding:.1rem 1rem;color:#4a4f59}table{border-collapse:collapse;margin:1rem 0}\
th,td{border:1px solid #d0d3da;padding:.35rem .6rem;text-align:left;vertical-align:top}th{background:#f4f4f6}\
.page-break{break-after:page}figure{margin:1rem 0}img{max-width:100%;height:auto}@media(prefers-color-scheme:dark){body{background:#17181c;color:#e8eaee}pre,th{background:#24262c}th,td{border-color:#3a3d45}blockquote{color:#a3a9b4;border-color:#3a3d45}a{color:#8ab4ff}}";

pub fn html(doc: &Doc, fallback: &str) -> String {
    let mut body = String::new();
    // Pictures go inside the page as data URIs, so the page is one self-contained file.
    let mut pics = |p: &Picture| {
        let (mime, _, bytes) = web_picture(p)?;
        Some(format!("data:{mime};base64,{}", crate::emf::base64(&bytes)))
    };
    html_blocks(&doc.blocks, &mut body, &mut 0, &mut pics);
    format!(
        "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>{CSS}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        esc(&doc.title_or(fallback))
    )
}

// ---------------------------------------------------------------- packages

fn zip_writer(out: &Path) -> Result<zip::ZipWriter<BufWriter<File>>, EngineError> {
    Ok(zip::ZipWriter::new(BufWriter::new(File::create(out)?)))
}

fn put(
    z: &mut zip::ZipWriter<BufWriter<File>>,
    name: &str,
    body: &str,
    stored: bool,
) -> Result<(), EngineError> {
    let o = SimpleFileOptions::default().compression_method(if stored {
        zip::CompressionMethod::Stored
    } else {
        zip::CompressionMethod::Deflated
    });
    z.start_file(name, o).map_err(EngineError::encode)?;
    z.write_all(body.as_bytes())?;
    Ok(())
}

fn put_bytes(
    z: &mut zip::ZipWriter<BufWriter<File>>,
    name: &str,
    body: &[u8],
) -> Result<(), EngineError> {
    // Pictures are compressed already.
    let o = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    z.start_file(name, o).map_err(EngineError::encode)?;
    z.write_all(body)?;
    Ok(())
}

fn finish(z: zip::ZipWriter<BufWriter<File>>) -> Result<(), EngineError> {
    z.finish().map_err(EngineError::encode)?.flush()?;
    Ok(())
}

// ---------------------------------------------------------------- DOCX

struct Docx {
    body: String,
    links: Vec<String>,
    /// One numbering instance per list (ordered?), so each numbered list restarts at 1.
    lists: Vec<bool>,
    media: Media,
    /// Drawings so far (each needs its own id).
    drawings: usize,
}

/// The text area of the pages the DOCX and ODT writers make (A4, 2.3 cm margins), in
/// points; pictures are scaled down to fit it.
const TEXT_W: f32 = 464.0;
const TEXT_H: f32 = 690.0;
/// English Metric Units per point.
const EMU: f32 = 12_700.0;

impl Docx {
    fn run(&mut self, i: &Inline) -> String {
        let mut rpr = String::new();
        if i.style.bold {
            rpr.push_str("<w:b/>");
        }
        if i.style.italic {
            rpr.push_str("<w:i/>");
        }
        if i.style.code {
            rpr.push_str("<w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/>");
        }
        if i.style.link.is_some() {
            rpr.push_str("<w:rStyle w:val=\"Hyperlink\"/>");
        }
        let rpr = if rpr.is_empty() {
            String::new()
        } else {
            format!("<w:rPr>{rpr}</w:rPr>")
        };
        let mut runs = String::new();
        for (k, line) in i.text.split('\n').enumerate() {
            if k > 0 {
                let _ = write!(runs, "<w:r>{rpr}<w:br/></w:r>");
            }
            for (t, part) in line.split('\t').enumerate() {
                if t > 0 {
                    let _ = write!(runs, "<w:r>{rpr}<w:tab/></w:r>");
                }
                if !part.is_empty() {
                    let _ = write!(
                        runs,
                        "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                        esc(part)
                    );
                }
            }
        }
        match &i.style.link {
            Some(l) => {
                let id = match self.links.iter().position(|x| x == l) {
                    Some(p) => p,
                    None => {
                        self.links.push(l.clone());
                        self.links.len() - 1
                    }
                };
                format!("<w:hyperlink r:id=\"rIdL{id}\">{runs}</w:hyperlink>")
            }
            None => runs,
        }
    }

    fn para(&mut self, style: Option<&str>, extra_ppr: &str, inl: &[Inline]) {
        let mut ppr = String::new();
        if let Some(s) = style {
            let _ = write!(ppr, "<w:pStyle w:val=\"{s}\"/>");
        }
        ppr.push_str(extra_ppr);
        let runs: String = inl.iter().map(|i| self.run(i)).collect();
        let _ = write!(
            self.body,
            "<w:p>{}{runs}</w:p>",
            if ppr.is_empty() {
                String::new()
            } else {
                format!("<w:pPr>{ppr}</w:pPr>")
            }
        );
    }

    fn blocks(&mut self, blocks: &[Block], quote: bool, list: Option<(usize, usize)>) {
        for b in blocks {
            match b {
                Block::Heading(l, i) => self.para(Some(&format!("Heading{}", l.min(&6))), "", i),
                Block::Para(i) => match list {
                    Some((num, lvl)) => self.para(Some("ListParagraph"), &format!("<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{num}\"/></w:numPr>"), i),
                    None if quote => self.para(Some("Quote"), "", i),
                    None => self.para(None, "", i),
                },
                Block::List { ordered, items } => {
                    self.lists.push(*ordered);
                    let num = self.lists.len();
                    let lvl = list.map_or(0, |(_, l)| l + 1).min(8);
                    for item in items {
                        for (k, inner) in item.iter().enumerate() {
                            match inner {
                                // First paragraph carries the bullet; later ones are continuation.
                                Block::Para(i) if k == 0 => self.blocks(std::slice::from_ref(&Block::Para(i.clone())), quote, Some((num, lvl))),
                                Block::Para(i) => {
                                    let ind = 720 * (lvl + 1);
                                    self.para(None, &format!("<w:ind w:left=\"{ind}\"/>"), i)
                                }
                                other => self.blocks(std::slice::from_ref(other), quote, Some((num, lvl))),
                            }
                        }
                    }
                }
                Block::Quote(inner) => self.blocks(inner, true, list),
                Block::Code(c) => {
                    for line in c.split('\n') {
                        let i = Inline { text: line.to_string(), style: Default::default() };
                        self.para(Some("Code"), "", std::slice::from_ref(&i));
                    }
                }
                Block::Table { rows, header, pictures } => {
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                    let w = 9000 / cols;
                    self.body.push_str("<w:tbl><w:tblPr><w:tblStyle w:val=\"TableGrid\"/><w:tblW w:w=\"0\" w:type=\"auto\"/></w:tblPr><w:tblGrid>");
                    for _ in 0..cols {
                        let _ = write!(self.body, "<w:gridCol w:w=\"{w}\"/>");
                    }
                    self.body.push_str("</w:tblGrid>");
                    for (r, row) in rows.iter().enumerate() {
                        let head = *header && r == 0;
                        self.body.push_str(if head { "<w:tr><w:trPr><w:tblHeader/></w:trPr>" } else { "<w:tr>" });
                        for c in 0..cols {
                            let text = row.get(c).map_or("", String::as_str);
                            let i = Inline { text: text.to_string(), style: super::Style { bold: head, ..Default::default() } };
                            let _ = write!(self.body, "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/></w:tcPr>");
                            self.para(None, "<w:spacing w:after=\"0\"/>", std::slice::from_ref(&i));
                            // Pictures fit the cell: its width less the cell margins (108 twips a side).
                            let max_w = (w as f32 - 216.0) / 20.0;
                            for p in cell_pictures(pictures, r, c) {
                                self.picture(p, max_w.max(12.0), TEXT_H / 2.0, false);
                            }
                            self.body.push_str("</w:tc>");
                        }
                        self.body.push_str("</w:tr>");
                    }
                    self.body.push_str("</w:tbl><w:p/>");
                }
                Block::Rule => self.body.push_str(
                    "<w:p><w:pPr><w:pBdr><w:bottom w:val=\"single\" w:sz=\"6\" w:space=\"1\" w:color=\"BBBBBB\"/></w:pBdr></w:pPr></w:p>",
                ),
                Block::PageBreak => self.body.push_str("<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>"),
                Block::Image(p) => self.picture(p, TEXT_W, TEXT_H, true),
            }
        }
    }

    /// A picture in a paragraph of its own (centred when `centre`), inline (moves with the
    /// text), no bigger than `max_w` × `max_h` points.
    fn picture(&mut self, p: &Picture, max_w: f32, max_h: f32, centre: bool) {
        let Some(mut placed) = place(p, max_w, max_h, true) else {
            return;
        };
        let ppr = if centre {
            r#"<w:pPr><w:jc w:val="center"/></w:pPr>"#
        } else {
            r#"<w:pPr><w:spacing w:before="60" w:after="0"/></w:pPr>"#
        };
        let (k, _) = self.media.add(p, &mut placed, "media/");
        let rid = format!("rIdP{k}");
        self.drawings += 1;
        let n = self.drawings;
        let (cx, cy) = (
            (placed.w * EMU).round() as u64,
            (placed.h * EMU).round() as u64,
        );
        let alt = esc(&p.alt);
        let _ = write!(
            self.body,
            r#"<w:p>{ppr}<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}" descr="{alt}"/><wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="Picture {n}" descr="{alt}"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="{rid}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
        );
    }
}

const DOCX_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:sz w:val="22"/><w:szCs w:val="22"/><w:lang w:val="en-US"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="276" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/><w:szCs w:val="30"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:i/><w:sz w:val="24"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val="4"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading6"><w:name w:val="heading 6"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:outlineLvl w:val="5"/></w:pPr><w:rPr><w:i/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="60"/><w:contextualSpacing/></w:pPr></w:style>
<w:style w:type="paragraph" w:styleId="Quote"><w:name w:val="Quote"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:ind w:left="720"/><w:pBdr><w:left w:val="single" w:sz="12" w:space="8" w:color="CCCCCC"/></w:pBdr></w:pPr><w:rPr><w:i/><w:color w:val="555555"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Code"><w:name w:val="Code"/><w:basedOn w:val="Normal"/><w:pPr><w:spacing w:after="0" w:line="240" w:lineRule="auto"/><w:shd w:val="clear" w:color="auto" w:fill="F2F2F2"/></w:pPr><w:rPr><w:rFonts w:ascii="Consolas" w:hAnsi="Consolas" w:cs="Consolas"/><w:sz w:val="19"/></w:rPr></w:style>
<w:style w:type="character" w:styleId="Hyperlink"><w:name w:val="Hyperlink"/><w:rPr><w:color w:val="0563C1"/><w:u w:val="single"/></w:rPr></w:style>
<w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:color="BFBFBF"/><w:left w:val="single" w:sz="4" w:color="BFBFBF"/><w:bottom w:val="single" w:sz="4" w:color="BFBFBF"/><w:right w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideH w:val="single" w:sz="4" w:color="BFBFBF"/><w:insideV w:val="single" w:sz="4" w:color="BFBFBF"/></w:tblBorders><w:tblCellMar><w:left w:w="108" w:type="dxa"/><w:right w:w="108" w:type="dxa"/></w:tblCellMar></w:tblPr></w:style>
</w:styles>"#;

fn docx_numbering(lists: &[bool]) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    );
    for (abs, ordered) in [(0, false), (1, true)] {
        let _ = write!(
            s,
            "<w:abstractNum w:abstractNumId=\"{abs}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
        );
        for lvl in 0..9 {
            let ind = 720 * (lvl + 1);
            let (fmt, text) = if ordered {
                (
                    ["decimal", "lowerLetter", "lowerRoman"][lvl % 3],
                    format!("%{}.", lvl + 1),
                )
            } else {
                ("bullet", ["•", "◦", "▪"][lvl % 3].to_string())
            };
            let _ = write!(
                s,
                "<w:lvl w:ilvl=\"{lvl}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{ind}\" w:hanging=\"360\"/></w:pPr></w:lvl>"
            );
        }
        s.push_str("</w:abstractNum>");
    }
    for (n, ordered) in lists.iter().enumerate() {
        let _ = write!(
            s,
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{}\"/><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride></w:num>",
            n + 1,
            u8::from(*ordered)
        );
    }
    s.push_str("</w:numbering>");
    s
}

fn core_props(title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>Swirvo</dc:creator></cp:coreProperties>"#,
        esc(title)
    )
}

pub fn docx(doc: &Doc, fallback: &str, out: &Path) -> Result<(), EngineError> {
    let mut d = Docx {
        body: String::new(),
        links: Vec::new(),
        lists: Vec::new(),
        media: Media::default(),
        drawings: 0,
    };
    d.blocks(&doc.blocks, false, None);
    if d.body.is_empty() {
        d.body.push_str("<w:p/>");
    }
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1304" w:bottom="1440" w:left="1304" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#,
        d.body
    );
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdS" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rIdN" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>"#,
    );
    for (k, l) in d.links.iter().enumerate() {
        let _ = write!(
            rels,
            r#"<Relationship Id="rIdL{k}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="{}" TargetMode="External"/>"#,
            esc(l)
        );
    }
    for (k, (name, _, _)) in d.media.files.iter().enumerate() {
        let _ = write!(
            rels,
            r#"<Relationship Id="rIdP{k}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="{name}"/>"#
        );
    }
    rels.push_str("</Relationships>");
    let mut types = String::new();
    for (name, mime, _) in &d.media.files {
        let ext = name.rsplit('.').next().unwrap_or("png");
        let entry = format!(r#"<Default Extension="{ext}" ContentType="{mime}"/>"#);
        if !types.contains(&entry) {
            types.push_str(&entry);
        }
    }

    let mut z = zip_writer(out)?;
    put(
        &mut z,
        "[Content_Types].xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/>{types}<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/></Types>"#
        ),
        false,
    )?;
    put(
        &mut z,
        "_rels/.rels",
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#,
        false,
    )?;
    put(
        &mut z,
        "docProps/core.xml",
        &core_props(&doc.title_or(fallback)),
        false,
    )?;
    put(&mut z, "word/document.xml", &document, false)?;
    put(&mut z, "word/styles.xml", DOCX_STYLES, false)?;
    put(
        &mut z,
        "word/numbering.xml",
        &docx_numbering(&d.lists),
        false,
    )?;
    put(&mut z, "word/_rels/document.xml.rels", &rels, false)?;
    for (name, _, bytes) in &d.media.files {
        put_bytes(&mut z, &format!("word/{name}"), bytes)?;
    }
    finish(z)
}

// ---------------------------------------------------------------- ODT

fn odt_inlines(inl: &[Inline]) -> String {
    let mut out = String::new();
    for i in inl {
        let mut s = String::new();
        for (k, line) in i.text.split('\n').enumerate() {
            if k > 0 {
                s.push_str("<text:line-break/>");
            }
            for (t, part) in line.split('\t').enumerate() {
                if t > 0 {
                    s.push_str("<text:tab/>");
                }
                // Runs of spaces must be spelled out in ODF.
                let mut spaces = 0;
                for c in part.chars() {
                    if c == ' ' {
                        spaces += 1;
                        continue;
                    }
                    flush_spaces(&mut s, &mut spaces);
                    s.push_str(&esc(&c.to_string()));
                }
                flush_spaces(&mut s, &mut spaces);
            }
        }
        let style = match (i.style.bold, i.style.italic, i.style.code) {
            (_, _, true) => Some("TCode"),
            (true, true, _) => Some("TBoldItalic"),
            (true, false, _) => Some("TBold"),
            (false, true, _) => Some("TItalic"),
            _ => None,
        };
        if let Some(st) = style {
            s = format!("<text:span text:style-name=\"{st}\">{s}</text:span>");
        }
        if let Some(l) = &i.style.link {
            s = format!(
                "<text:a xlink:type=\"simple\" xlink:href=\"{}\">{s}</text:a>",
                esc(l)
            );
        }
        out.push_str(&s);
    }
    out
}

fn flush_spaces(s: &mut String, n: &mut usize) {
    match *n {
        0 => {}
        1 => s.push(' '),
        k => {
            let _ = write!(s, " <text:s text:c=\"{}\"/>", k - 1);
        }
    }
    *n = 0;
}

/// What the ODT writer carries through the blocks.
#[derive(Default)]
struct OdtState {
    tables: usize,
    page_break: bool,
    media: Media,
    frames: usize,
}

fn odt_blocks(blocks: &[Block], out: &mut String, quote: bool, st: &mut OdtState) {
    for b in blocks {
        let pstyle = |default: &str, page_break: &mut bool| {
            if std::mem::take(page_break) {
                "PBreak".to_string()
            } else {
                default.to_string()
            }
        };
        match b {
            Block::Heading(l, i) => {
                let _ = write!(
                    out,
                    "<text:h text:style-name=\"Heading_20_{l}\" text:outline-level=\"{l}\">{}</text:h>",
                    odt_inlines(i)
                );
            }
            Block::Para(i) => {
                let st = pstyle(
                    if quote { "Quotations" } else { "Text_20_body" },
                    &mut st.page_break,
                );
                let _ = write!(
                    out,
                    "<text:p text:style-name=\"{st}\">{}</text:p>",
                    odt_inlines(i)
                );
            }
            Block::List { ordered, items } => {
                let _ = write!(
                    out,
                    "<text:list text:style-name=\"{}\">",
                    if *ordered { "LNum" } else { "LBullet" }
                );
                for item in items {
                    out.push_str("<text:list-item>");
                    odt_blocks(item, out, quote, st);
                    out.push_str("</text:list-item>");
                }
                out.push_str("</text:list>");
            }
            Block::Quote(inner) => odt_blocks(inner, out, true, st),
            Block::Code(c) => {
                for line in c.split('\n') {
                    let i = Inline {
                        text: line.to_string(),
                        style: Default::default(),
                    };
                    let _ = write!(
                        out,
                        "<text:p text:style-name=\"Preformatted_20_Text\">{}</text:p>",
                        odt_inlines(std::slice::from_ref(&i))
                    );
                }
            }
            Block::Table {
                rows,
                header,
                pictures,
            } => {
                st.tables += 1;
                let tables = st.tables;
                let cols = rows.iter().map(Vec::len).max().unwrap_or(1).max(1);
                let _ = write!(
                    out,
                    "<table:table table:name=\"Table{tables}\" table:style-name=\"Tbl\"><table:table-column table:number-columns-repeated=\"{cols}\"/>"
                );
                for (r, row) in rows.iter().enumerate() {
                    let head = *header && r == 0;
                    if head {
                        out.push_str("<table:table-header-rows>");
                    }
                    out.push_str("<table:table-row>");
                    for c in 0..cols {
                        let text = row.get(c).map_or("", String::as_str);
                        let i = Inline {
                            text: text.to_string(),
                            style: super::Style {
                                bold: head,
                                ..Default::default()
                            },
                        };
                        let _ = write!(
                            out,
                            "<table:table-cell table:style-name=\"Cell\" office:value-type=\"string\"><text:p text:style-name=\"Table_20_Contents\">{}</text:p>",
                            odt_inlines(std::slice::from_ref(&i))
                        );
                        // Pictures fit the cell: the table's 17 cm shared out, less padding.
                        let max_w = (ODT_TABLE_W / cols as f32 - 2.0 * ODT_CELL_PAD).max(12.0);
                        for p in cell_pictures(pictures, r, c) {
                            odt_picture(p, max_w, TEXT_H / 2.0, "Table_20_Contents", out, st);
                        }
                        out.push_str("</table:table-cell>");
                    }
                    out.push_str("</table:table-row>");
                    if head {
                        out.push_str("</table:table-header-rows>");
                    }
                }
                out.push_str("</table:table>");
            }
            Block::Rule => out.push_str("<text:p text:style-name=\"Rule\"/>"),
            Block::PageBreak => st.page_break = true,
            Block::Image(p) => {
                let style = pstyle("PFigure", &mut st.page_break);
                odt_picture(p, TEXT_W, TEXT_H, &style, out, st);
            }
        }
    }
}

/// The width of tables the ODT writer makes (17 cm) and their cells' padding, in points.
const ODT_TABLE_W: f32 = 481.9;
const ODT_CELL_PAD: f32 = 2.83;

/// A picture in a paragraph of its own (style `style`), anchored as a character, no
/// bigger than `max_w` × `max_h` points.
fn odt_picture(
    p: &Picture,
    max_w: f32,
    max_h: f32,
    style: &str,
    out: &mut String,
    st: &mut OdtState,
) {
    let Some(mut placed) = place(p, max_w, max_h, true) else {
        return;
    };
    let (_, href) = st.media.add(p, &mut placed, "Pictures/");
    st.frames += 1;
    let title = if p.alt.is_empty() {
        String::new()
    } else {
        format!("<svg:title>{}</svg:title>", esc(&p.alt))
    };
    let _ = write!(
        out,
        "<text:p text:style-name=\"{style}\"><draw:frame draw:name=\"Picture{n}\" text:anchor-type=\"as-char\" svg:width=\"{w:.2}pt\" svg:height=\"{h:.2}pt\" draw:z-index=\"0\"><draw:image xlink:href=\"{href}\" xlink:type=\"simple\" xlink:show=\"embed\" xlink:actuate=\"onLoad\" draw:mime-type=\"{mime}\"/>{title}</draw:frame></text:p>",
        n = st.frames,
        w = placed.w,
        h = placed.h,
        mime = placed.mime,
    );
}

const ODF_NS: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0" xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0" xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0" xmlns:xlink="http://www.w3.org/1999/xlink" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0" office:version="1.3""#;

pub fn odt(doc: &Doc, fallback: &str, out: &Path) -> Result<(), EngineError> {
    let mut body = String::new();
    let mut st = OdtState::default();
    odt_blocks(&doc.blocks, &mut body, false, &mut st);
    let content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content {ODF_NS}><office:automatic-styles>
<style:style style:name="TBold" style:family="text"><style:text-properties fo:font-weight="bold"/></style:style>
<style:style style:name="TItalic" style:family="text"><style:text-properties fo:font-style="italic"/></style:style>
<style:style style:name="TBoldItalic" style:family="text"><style:text-properties fo:font-weight="bold" fo:font-style="italic"/></style:style>
<style:style style:name="TCode" style:family="text"><style:text-properties style:font-name="Consolas" fo:font-family="Consolas"/></style:style>
<style:style style:name="PBreak" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:break-before="page"/></style:style>
<style:style style:name="PFigure" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:text-align="center" fo:margin-top="0.2cm" fo:margin-bottom="0.2cm"/></style:style>
<style:style style:name="Rule" style:family="paragraph"><style:paragraph-properties fo:border-bottom="0.5pt solid #bbbbbb" fo:margin-bottom="0.2cm"/></style:style>
<style:style style:name="Tbl" style:family="table"><style:table-properties style:width="17cm" table:align="left"/></style:style>
<style:style style:name="Cell" style:family="table-cell"><style:table-cell-properties fo:border="0.5pt solid #bfbfbf" fo:padding="0.1cm"/></style:style>
<text:list-style style:name="LBullet">{}</text:list-style>
<text:list-style style:name="LNum">{}</text:list-style>
</office:automatic-styles><office:body><office:text>{body}</office:text></office:body></office:document-content>"#,
        (1..=10).map(|l| format!(r#"<text:list-level-style-bullet text:level="{l}" text:bullet-char="{}"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.6cm" fo:margin-left="{:.1}cm"/></style:list-level-properties></text:list-level-style-bullet>"#, ["•", "◦", "▪"][(l - 1) % 3], 0.6 * l as f32 + 0.1)).collect::<String>(),
        (1..=10).map(|l| format!(r#"<text:list-level-style-number text:level="{l}" style:num-suffix="." style:num-format="1"><style:list-level-properties text:list-level-position-and-space-mode="label-alignment"><style:list-level-label-alignment text:label-followed-by="listtab" fo:text-indent="-0.6cm" fo:margin-left="{:.1}cm"/></style:list-level-properties></text:list-level-style-number>"#, 0.6 * l as f32 + 0.1)).collect::<String>(),
    );
    let styles = format!(
        r##"<?xml version="1.0" encoding="UTF-8"?><office:document-styles {ODF_NS}><office:styles>
<style:default-style style:family="paragraph"><style:paragraph-properties fo:margin-bottom="0.21cm"/><style:text-properties fo:font-size="11pt" style:font-name="Calibri" fo:font-family="Calibri, 'Segoe UI', sans-serif"/></style:default-style>
<style:style style:name="Standard" style:family="paragraph" style:class="text"/>
<style:style style:name="Text_20_body" style:display-name="Text body" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:line-height="130%"/></style:style>
{}
<style:style style:name="Quotations" style:family="paragraph" style:parent-style-name="Text_20_body"><style:paragraph-properties fo:margin-left="1cm" fo:border-left="1.5pt solid #cccccc" fo:padding-left="0.3cm"/><style:text-properties fo:font-style="italic" fo:color="#555555"/></style:style>
<style:style style:name="Preformatted_20_Text" style:display-name="Preformatted Text" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-bottom="0cm" fo:background-color="#f2f2f2"/><style:text-properties style:font-name="Consolas" fo:font-family="Consolas, monospace" fo:font-size="9.5pt"/></style:style>
<style:style style:name="Table_20_Contents" style:display-name="Table Contents" style:family="paragraph" style:parent-style-name="Standard"><style:paragraph-properties fo:margin-bottom="0cm"/></style:style>
</office:styles><office:automatic-styles><style:page-layout style:name="pm1"><style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm" fo:margin-top="2.5cm" fo:margin-bottom="2.5cm" fo:margin-left="2.3cm" fo:margin-right="2.3cm"/></style:page-layout></office:automatic-styles><office:master-styles><style:master-page style:name="Standard" style:page-layout-name="pm1"/></office:master-styles></office:document-styles>"##,
        (1..=6)
            .map(|l| format!(r#"<style:style style:name="Heading_20_{l}" style:display-name="Heading {l}" style:family="paragraph" style:parent-style-name="Standard" style:next-style-name="Text_20_body" style:default-outline-level="{l}"><style:paragraph-properties fo:margin-top="{:.2}cm" fo:margin-bottom="0.2cm" fo:keep-with-next="always"/><style:text-properties fo:font-size="{}pt" fo:font-weight="bold"/></style:style>"#, 0.5 - 0.05 * l as f32, [18, 15, 13, 12, 11, 11][l - 1]))
            .collect::<String>()
    );
    let meta = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><office:document-meta {ODF_NS}><office:meta><dc:title>{}</dc:title><meta:generator>Swirvo</meta:generator></office:meta></office:document-meta>"#,
        esc(&doc.title_or(fallback))
    );
    let mut z = zip_writer(out)?;
    // The mimetype must be the first entry, stored uncompressed (ODF 1.3 §3.3).
    put(
        &mut z,
        "mimetype",
        "application/vnd.oasis.opendocument.text",
        true,
    )?;
    let pictures: String = st
        .media
        .files
        .iter()
        .map(|(name, mime, _)| {
            format!(
                r#"<manifest:file-entry manifest:full-path="{name}" manifest:media-type="{mime}"/>"#
            )
        })
        .collect();
    put(
        &mut z,
        "META-INF/manifest.xml",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.3"><manifest:file-entry manifest:full-path="/" manifest:version="1.3" manifest:media-type="application/vnd.oasis.opendocument.text"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="styles.xml" manifest:media-type="text/xml"/><manifest:file-entry manifest:full-path="meta.xml" manifest:media-type="text/xml"/>{pictures}</manifest:manifest>"#
        ),
        false,
    )?;
    put(&mut z, "content.xml", &content, false)?;
    put(&mut z, "styles.xml", &styles, false)?;
    put(&mut z, "meta.xml", &meta, false)?;
    for (name, _, bytes) in &st.media.files {
        put_bytes(&mut z, name, bytes)?;
    }
    finish(z)
}

// ---------------------------------------------------------------- EPUB 3

pub fn epub(doc: &Doc, fallback: &str, out: &Path) -> Result<(), EngineError> {
    let title = doc.title_or(fallback);
    // Chapters: split before each level-1 heading and at page breaks; never empty.
    let mut chapters: Vec<Vec<Block>> = vec![Vec::new()];
    for b in &doc.blocks {
        let split = matches!(b, Block::Heading(1, _) | Block::PageBreak);
        if split && chapters.last().is_some_and(|c| !c.is_empty()) {
            chapters.push(Vec::new());
        }
        if !matches!(b, Block::PageBreak)
            && let Some(c) = chapters.last_mut()
        {
            c.push(b.clone());
        }
    }
    chapters.retain(|c| !c.is_empty());
    if chapters.is_empty() {
        chapters.push(vec![Block::Para(vec![Inline {
            text: title.clone(),
            style: Default::default(),
        }])]);
    }

    // A stable identifier derived from the content (no clock, no randomness).
    let id = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        format!("{:?}", doc.blocks).hash(&mut h);
        format!("urn:swirvo:{:016x}", h.finish())
    };

    let mut z = zip_writer(out)?;
    put(&mut z, "mimetype", "application/epub+zip", true)?;
    put(
        &mut z,
        "META-INF/container.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        false,
    )?;
    put(&mut z, "OEBPS/style.css", CSS, false)?;

    let mut nav = String::new();
    let mut manifest = String::new();
    let mut spine = String::new();
    // Pictures become files in the book, each written once however often it appears.
    let mut images: Vec<(String, &'static str, Vec<u8>)> = Vec::new();
    let mut seen: std::collections::HashMap<*const Vec<u8>, Option<String>> =
        std::collections::HashMap::new();
    for (n, ch) in chapters.iter().enumerate() {
        let file = format!("chapter-{:03}.xhtml", n + 1);
        let mut body = String::new();
        let mut pics = |p: &Picture| {
            seen.entry(std::sync::Arc::as_ptr(&p.bytes))
                .or_insert_with(|| {
                    let (mime, ext, bytes) = web_picture(p)?;
                    let name = format!("images/image-{:03}.{ext}", images.len() + 1);
                    images.push((name.clone(), mime, bytes));
                    Some(name)
                })
                .clone()
        };
        html_blocks(ch, &mut body, &mut 0, &mut pics);
        let ch_title = ch
            .iter()
            .find_map(|b| match b {
                Block::Heading(_, i) => Some(plain(i)),
                _ => None,
            })
            .unwrap_or_else(|| {
                if chapters.len() == 1 {
                    title.clone()
                } else {
                    format!("Part {}", n + 1)
                }
            });
        let xhtml = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\">\n<head><title>{}</title><link rel=\"stylesheet\" href=\"style.css\"/></head>\n<body>\n{body}</body>\n</html>\n",
            esc(&ch_title)
        );
        put(&mut z, &format!("OEBPS/{file}"), &xhtml, false)?;
        let _ = write!(nav, "<li><a href=\"{file}\">{}</a></li>", esc(&ch_title));
        let _ = write!(
            manifest,
            "<item id=\"c{n}\" href=\"{file}\" media-type=\"application/xhtml+xml\"/>"
        );
        let _ = write!(spine, "<itemref idref=\"c{n}\"/>");
    }
    for (k, (name, mime, bytes)) in images.iter().enumerate() {
        put_bytes(&mut z, &format!("OEBPS/{name}"), bytes)?;
        let _ = write!(
            manifest,
            "<item id=\"img{k}\" href=\"{name}\" media-type=\"{mime}\"/>"
        );
    }
    put(
        &mut z,
        "OEBPS/nav.xhtml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\"><head><title>{t}</title></head><body><nav epub:type=\"toc\" id=\"toc\"><h1>{t}</h1><ol>{nav}</ol></nav></body></html>",
            t = esc(&title)
        ),
        false,
    )?;
    put(
        &mut z,
        "OEBPS/content.opf",
        &format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="uid">{id}</dc:identifier><dc:title>{}</dc:title><dc:language>en</dc:language><meta property="dcterms:modified">2000-01-01T00:00:00Z</meta></metadata><manifest><item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/><item id="css" href="style.css" media-type="text/css"/>{manifest}</manifest><spine>{spine}</spine></package>"#,
            esc(&title)
        ),
        false,
    )?;
    finish(z)
}
