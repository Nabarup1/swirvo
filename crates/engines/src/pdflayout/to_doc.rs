//! Layout → [`Doc`], for PDF → Markdown, HTML, TXT, ODT and EPUB: headings from sizes, real
//! lists, tables and emphasis, in reading order.

use std::sync::Arc;

use super::model::{BBox, Block as LBlock, Float, FontInfo, Layout, Para, Piece};
use crate::doc::{Block, Doc, Inline, Picture, Style};

/// Smaller than this (points, either side) is a bullet, a rule or a speck, not a picture.
const MIN_PICTURE: f32 = 12.0;

fn inlines(p: &Para, fonts: &[FontInfo], skip_number: bool) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::new();
    let mut push = |text: &str, style: Style| {
        if text.is_empty() {
            return;
        }
        match out.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => out.push(Inline {
                text: text.to_string(),
                style,
            }),
        }
    };
    for (i, line) in p.lines.iter().enumerate() {
        let mut pieces: &[Piece] = line;
        if i == 0 && skip_number {
            // "12.\tText": the list carries the number.
            if let Some(t) = pieces.iter().position(|x| matches!(x, Piece::Tab)) {
                pieces = &pieces[t + 1..];
            }
        }
        if i > 0 {
            push(" ", Style::default());
        }
        for piece in pieces {
            match piece {
                Piece::Run(r) | Piece::PageNumber(r) | Piece::PageCount(r) => {
                    let style = Style {
                        bold: r.bold,
                        italic: r.italic,
                        code: fonts.get(r.font as usize).is_some_and(|f| f.mono),
                        link: None,
                    };
                    push(&r.text, style);
                }
                Piece::Tab => push(" ", Style::default()),
            }
        }
    }
    // Undo hyphenation at line ends, then tidy spaces.
    for inl in &mut out {
        let mut s = String::with_capacity(inl.text.len());
        let chars: Vec<char> = inl.text.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            if chars[k] == '-'
                && k > 0
                && chars[k - 1].is_alphabetic()
                && chars.get(k + 1) == Some(&' ')
                && chars.get(k + 2).is_some_and(|c| c.is_lowercase())
            {
                k += 2;
                continue;
            }
            s.push(chars[k]);
            k += 1;
        }
        inl.text = s;
    }
    if let Some(first) = out.first_mut() {
        first.text = first.text.trim_start().to_string();
    }
    if let Some(last) = out.last_mut() {
        last.text = last.text.trim_end().to_string();
    }
    out.retain(|i| !i.text.is_empty());
    out
}

fn links(p: &Para, l: &Layout, inl: &mut [Inline]) {
    // One link per paragraph is the common case (a URL, an email); attach it to the text.
    let link = p.lines.iter().flatten().find_map(|x| match x {
        Piece::Run(r) => r.link,
        _ => None,
    });
    if let Some(k) = link
        && let Some(url) = l.links.get(k as usize)
        && inl.len() == 1
    {
        inl[0].style.link = Some(url.clone());
    }
}

pub fn to_doc(l: &Layout) -> Doc {
    let mut blocks: Vec<Block> = Vec::new();
    let flush_list = |blocks: &mut Vec<Block>, list: &mut Option<(bool, Vec<Vec<Block>>)>| {
        if let Some((ordered, items)) = list.take() {
            blocks.push(Block::List { ordered, items });
        }
    };
    // One shared copy of each picture, however often it's drawn (logos on every page).
    let mut shared: Vec<Option<Arc<Vec<u8>>>> = vec![None; l.media.len()];
    let mut picture = |media: u32, bbox: &BBox| -> Option<Block> {
        let (w, h) = (bbox.x1 - bbox.x0, bbox.y1 - bbox.y0);
        if w < MIN_PICTURE || h < MIN_PICTURE {
            return None;
        }
        let m = l.media.get(media as usize)?;
        let bytes = shared
            .get_mut(media as usize)?
            .get_or_insert_with(|| Arc::new(m.bytes.clone()))
            .clone();
        Some(Block::Image(Picture {
            bytes,
            alt: String::new(),
            width_pt: Some(w.round() as u32),
        }))
    };
    for (pi, page) in l.pages.iter().enumerate() {
        if pi > 0 {
            blocks.push(Block::PageBreak);
        }
        let mut list: Option<(bool, Vec<Vec<Block>>)> = None;
        // Pictures placed on the page (not behind the text), in the reading order of their
        // tops: each goes before the first block that starts below it.
        let mut floats: Vec<(f32, u32, BBox)> = page
            .floats
            .iter()
            .filter_map(|f| match f {
                Float::Picture {
                    media,
                    bbox,
                    behind: false,
                } => Some((bbox.y0, *media, *bbox)),
                _ => None,
            })
            .collect();
        floats.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut floats = floats.into_iter().peekable();
        for band in &page.bands {
            for col in &band.cols {
                for b in &col.blocks {
                    while let Some(&(top, media, bbox)) = floats.peek() {
                        if top > b.top() {
                            break;
                        }
                        floats.next();
                        if let Some(img) = picture(media, &bbox) {
                            flush_list(&mut blocks, &mut list);
                            blocks.push(img);
                        }
                    }
                    match b {
                        LBlock::Para(p) => {
                            let item = p.bullet.is_some() || p.numbered;
                            let mut inl = inlines(p, &l.fonts, p.numbered);
                            links(p, l, &mut inl);
                            if inl.is_empty() {
                                continue;
                            }
                            if item {
                                let ordered = p.numbered;
                                if list.as_ref().is_some_and(|(o, _)| *o != ordered) {
                                    flush_list(&mut blocks, &mut list);
                                }
                                list.get_or_insert((ordered, Vec::new()))
                                    .1
                                    .push(vec![Block::Para(inl)]);
                                continue;
                            }
                            flush_list(&mut blocks, &mut list);
                            blocks.push(match p.heading {
                                Some(h) => Block::Heading(h, inl),
                                None => Block::Para(inl),
                            });
                        }
                        LBlock::Table(t) => {
                            flush_list(&mut blocks, &mut list);
                            let rows = t
                                .rows
                                .iter()
                                .map(|r| {
                                    r.cells
                                        .iter()
                                        .map(|c| {
                                            c.paras
                                                .iter()
                                                .map(|p| {
                                                    inlines(p, &l.fonts, false)
                                                        .iter()
                                                        .map(|i| i.text.as_str())
                                                        .collect::<String>()
                                                })
                                                .collect::<Vec<_>>()
                                                .join(" ")
                                        })
                                        .collect()
                                })
                                .collect();
                            let header = t.rows.first().is_some_and(|r| {
                                r.cells.iter().all(|c| c.paras.iter().all(|p| p.bold))
                            });
                            blocks.push(Block::Table {
                                rows,
                                header,
                                pictures: Vec::new(),
                            });
                        }
                        LBlock::Picture(p) => {
                            if let Some(img) = picture(p.media, &p.bbox) {
                                flush_list(&mut blocks, &mut list);
                                blocks.push(img);
                            }
                        }
                    }
                }
            }
        }
        flush_list(&mut blocks, &mut list);
        for (_, media, bbox) in floats {
            if let Some(img) = picture(media, &bbox) {
                blocks.push(img);
            }
        }
    }
    Doc {
        title: l.title.clone(),
        blocks,
    }
}
