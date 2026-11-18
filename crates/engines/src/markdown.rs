//! Markdown → standalone HTML and plain text (pulldown-cmark). Pandoc covers the rest.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use swirvo_core::{Capability, Engine, EngineError, StepRequest};

use crate::text::read_text;

pub struct MarkdownEngine;

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
}

impl Engine for MarkdownEngine {
    fn id(&self) -> &'static str {
        "markdown"
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability::new("markdown", "md", "html")
                .time(5, 10)
                .note("CommonMark + GFM tables, standalone page"),
            Capability::new("markdown", "md", "txt")
                .loss(10)
                .time(5, 10)
                .note("markup removed"),
        ]
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let src = read_text(req.input)?;
        let out = match req.to {
            "html" => to_html(&src),
            "txt" => to_text(&src),
            other => {
                return Err(EngineError::Invalid(format!(
                    "markdown engine can't write {other}"
                )));
            }
        };
        std::fs::write(req.output, out)?;
        Ok(())
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn to_html(src: &str) -> String {
    let mut title = String::new();
    let mut in_h1 = false;
    let mut have_title = false;
    for ev in Parser::new_ext(src, options()) {
        match ev {
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            }) if !have_title => in_h1 = true,
            Event::Text(t) | Event::Code(t) if in_h1 => title.push_str(&t),
            Event::End(TagEnd::Heading(_)) if in_h1 => {
                in_h1 = false;
                have_title = true;
            }
            _ => {}
        }
    }
    if title.is_empty() {
        title.push_str("Document");
    }
    let mut body = String::with_capacity(src.len() * 2);
    html::push_html(&mut body, Parser::new_ext(src, options()));
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n<style>body{{font:16px/1.6 system-ui,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem}}pre,code{{font-family:ui-monospace,monospace}}pre{{background:#f4f4f5;padding:1rem;overflow:auto}}table{{border-collapse:collapse}}td,th{{border:1px solid #ccc;padding:.3rem .6rem}}@media (prefers-color-scheme:dark){{body{{background:#18181b;color:#e4e4e7}}pre{{background:#27272a}}}}</style>\n</head>\n<body>\n{body}</body>\n</html>\n",
        escape(&title)
    )
}

pub fn to_text(src: &str) -> String {
    let mut out = String::new();
    let mut list_depth = 0usize;
    for ev in Parser::new_ext(src, options()) {
        match ev {
            Event::Text(t) | Event::Code(t) => out.push_str(&t),
            Event::SoftBreak | Event::HardBreak => out.push('\n'),
            Event::Start(Tag::List(_)) => list_depth += 1,
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    out.push('\n');
                }
            }
            Event::Start(Tag::Item) => {
                out.push_str(&"  ".repeat(list_depth.saturating_sub(1)));
                out.push_str("- ");
            }
            Event::End(TagEnd::Item) => out.push('\n'),
            Event::End(
                TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::BlockQuote(_),
            ) => {
                if list_depth == 0 {
                    out.push_str("\n\n");
                }
            }
            Event::End(TagEnd::TableCell) => out.push('\t'),
            Event::End(TagEnd::TableRow | TagEnd::TableHead) => out.push('\n'),
            _ => {}
        }
    }
    let mut s = out.trim_end().to_string();
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_has_title_and_table() {
        let h = to_html("# Release <1.0>\n\n| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(h.contains("<title>Release &lt;1.0&gt;</title>"), "{h}");
        assert!(h.contains("<table>"));
    }

    #[test]
    fn text_strips_markup() {
        assert_eq!(
            to_text("# Hi\n\nSome **bold** `code`.\n\n- one\n- two\n"),
            "Hi\n\nSome bold code.\n\n- one\n- two\n"
        );
    }
}
