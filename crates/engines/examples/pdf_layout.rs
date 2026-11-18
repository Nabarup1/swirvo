//! Inspect the PDF layout analysis (docs/13 §6).
//!
//! ```text
//! cargo run -p swirvo-engines --example pdf_layout -- dump in.pdf [out.docx]
//! cargo run -p swirvo-engines --example pdf_layout -- compare a.pdf b.pdf out.png [dpi]
//! ```
//! `dump` prints pages, bands, columns and blocks with their geometry (and writes the DOCX);
//! `compare` renders two PDFs page by page side by side, for visual round-trip checks.

use std::error::Error;
use std::path::Path;
use swirvo_core::CancelToken;
use swirvo_engines::pdf_tools::PageRenderer;
use swirvo_engines::pdflayout::{self, model::*};

fn text(p: &Para) -> String {
    let t = p.text();
    if t.chars().count() > 70 {
        format!("{}…", t.chars().take(70).collect::<String>())
    } else {
        t
    }
}

fn dump_block(b: &Block, indent: &str) {
    match b {
        Block::Para(p) => println!(
            "{indent}P {:?} L{:.0} F{:.0} R{:.0} base {:.1}..{:.1} pitch {:.1} size {} {}{}{}{}  {:?}",
            p.align,
            p.left,
            p.first,
            p.right,
            p.first_base,
            p.last_base,
            p.pitch,
            p.size,
            if p.bold { "bold " } else { "" },
            p.heading.map(|h| format!("H{h} ")).unwrap_or_default(),
            p.bullet
                .as_ref()
                .map(|b| format!("bullet {} ", b.glyph))
                .unwrap_or_default(),
            if p.tabs.is_empty() {
                String::new()
            } else {
                format!("tabs {} ", p.tabs.len())
            },
            text(p)
        ),
        Block::Table(t) => {
            println!(
                "{indent}TABLE {}×{} at {:.0},{:.0} {:.0}×{:.0}",
                t.rows.len(),
                t.cols.len(),
                t.bbox.x0,
                t.bbox.y0,
                t.bbox.w(),
                t.bbox.h()
            );
            for (r, row) in t.rows.iter().enumerate().take(6) {
                let cells: Vec<String> = row
                    .cells
                    .iter()
                    .map(|c| {
                        let s: String = c
                            .paras
                            .iter()
                            .map(Para::text)
                            .collect::<Vec<_>>()
                            .join(" / ");
                        format!(
                            "[{}{}]",
                            s.chars().take(18).collect::<String>(),
                            if c.span > 1 {
                                format!(" ×{}", c.span)
                            } else {
                                String::new()
                            }
                        )
                    })
                    .collect();
                println!("{indent}  r{r} h{:.0} {}", row.height, cells.join(" "));
            }
        }
        Block::Picture(p) => println!(
            "{indent}PICTURE #{} at {:.0},{:.0} {:.0}×{:.0}",
            p.media,
            p.bbox.x0,
            p.bbox.y0,
            p.bbox.w(),
            p.bbox.h()
        ),
    }
}

fn dump(input: &Path, out: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let bytes = std::fs::read(input)?;
    let t = std::time::Instant::now();
    let l = pdflayout::analyze(&bytes, &CancelToken::new())?;
    println!(
        "{} pages in {:?}; body {:?} {} pt; {} fonts; {} media; title {:?}",
        l.pages.len(),
        t.elapsed(),
        l.fonts.get(l.body_font as usize).map(|f| &f.family),
        l.body_size,
        l.fonts.len(),
        l.media.len(),
        l.title
    );
    for f in &l.fonts {
        println!(
            "  font {:?} bold={} italic={} serif={} mono={}",
            f.family, f.bold, f.italic, f.serif, f.mono
        );
    }
    if let Some(h) = &l.header {
        println!(
            "HEADER at {:.0}: {:?}",
            h.offset,
            h.paras.iter().map(Para::text).collect::<Vec<_>>()
        );
    }
    if let Some(f) = &l.footer {
        println!(
            "FOOTER at {:.0}: {:?}",
            f.offset,
            f.paras.iter().map(Para::text).collect::<Vec<_>>()
        );
    }
    for (i, p) in l.pages.iter().enumerate() {
        let m = p.margins;
        println!(
            "\n== page {} {:.0}×{:.0} margins t{:.0} r{:.0} b{:.0} l{:.0}; {} floats; header {} footer {}",
            i + 1,
            p.w,
            p.h,
            m.top,
            m.right,
            m.bottom,
            m.left,
            p.floats.len(),
            p.header,
            p.footer
        );
        for f in p.floats.iter().take(12) {
            match f {
                Float::Picture {
                    media,
                    bbox,
                    behind,
                } => println!(
                    "  float picture #{media} {:.0},{:.0} {:.0}×{:.0} behind={behind}",
                    bbox.x0,
                    bbox.y0,
                    bbox.w(),
                    bbox.h()
                ),
                Float::Rect { bbox, fill, stroke } => println!(
                    "  float rect {:.0},{:.0} {:.0}×{:.0} fill {:?} stroke {:?}",
                    bbox.x0,
                    bbox.y0,
                    bbox.w(),
                    bbox.h(),
                    fill,
                    stroke.map(|s| (s.width, s.color))
                ),
            }
        }
        for (bi, b) in p.bands.iter().enumerate() {
            println!("  band {bi}: {} col(s)", b.cols.len());
            for (ci, c) in b.cols.iter().enumerate() {
                println!("   col {ci} x {:.0}..{:.0}", c.x0, c.x1);
                for blk in &c.blocks {
                    dump_block(blk, "    ");
                }
            }
        }
    }
    if let Some(out) = out {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        pdflayout::write_docx(&l, &stem, out)?;
        println!("\nwrote {}", out.display());
    }
    Ok(())
}

fn compare(a: &Path, b: &Path, out: &Path, dpi: f32) -> Result<(), Box<dyn Error>> {
    // PAGES=first-last (1-based) limits the comparison.
    let range = std::env::var("PAGES").ok().and_then(|r| {
        let (x, y) = r.split_once('-')?;
        Some((x.parse::<usize>().ok()? - 1, y.parse::<usize>().ok()?))
    });
    let ra = PageRenderer::open(a)?;
    let rb = PageRenderer::open(b)?;
    let n = ra.count().max(rb.count());
    let mut pages: Vec<(Option<image::RgbaImage>, Option<image::RgbaImage>)> = Vec::new();
    let mut ia = Vec::new();
    ra.session(|s| {
        for i in 0..ra.count() {
            ia.push(s.render_dpi(i, dpi).ok());
        }
    });
    let mut ib = Vec::new();
    rb.session(|s| {
        for i in 0..rb.count() {
            ib.push(s.render_dpi(i, dpi).ok());
        }
    });
    let (from, to) = range.unwrap_or((0, n));
    for i in from..to.min(n) {
        pages.push((ia.get(i).cloned().flatten(), ib.get(i).cloned().flatten()));
    }
    let gap = 12;
    let cw = pages
        .iter()
        .map(|(x, y)| x.as_ref().map_or(0, |i| i.width()) + y.as_ref().map_or(0, |i| i.width()))
        .max()
        .unwrap_or(1)
        + gap * 3;
    let ch: u32 = pages
        .iter()
        .map(|(x, y)| {
            x.as_ref()
                .map_or(0, |i| i.height())
                .max(y.as_ref().map_or(0, |i| i.height()))
                + gap
        })
        .sum::<u32>()
        + gap;
    let mut canvas = image::RgbaImage::from_pixel(cw, ch, image::Rgba([90, 90, 100, 255]));
    let mut y = gap;
    for (x, z) in &pages {
        let mut xpos = gap;
        let mut h = 0;
        for img in [x, z].into_iter().flatten() {
            image::imageops::overlay(&mut canvas, img, i64::from(xpos), i64::from(y));
            xpos += img.width() + gap;
            h = h.max(img.height());
        }
        y += h + gap;
    }
    canvas.save(out)?;
    println!("{n} page(s) → {}", out.display());
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("raw") if args.len() >= 3 => {
            let bytes = std::fs::read(&args[1])?;
            print!("{}", pdflayout::debug_page(&bytes, args[2].parse()?)?);
            Ok(())
        }
        Some("dump") if args.len() >= 2 => dump(Path::new(&args[1]), args.get(2).map(Path::new)),
        Some("compare") if args.len() >= 4 => compare(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            args.get(4).and_then(|d| d.parse().ok()).unwrap_or(60.0),
        ),
        _ => {
            eprintln!(
                "usage: pdf_layout dump in.pdf [out.docx] | compare a.pdf b.pdf out.png [dpi]"
            );
            std::process::exit(2);
        }
    }
}
