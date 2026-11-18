//! Render the Board and the page picker to PNGs for design review:
//! `cargo run -p swirvo-board --example snapshot -- <out-dir>`

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swirvo_board::{Board, PagePicker};
use swirvo_jobs::Toolkit;
use swirvo_jobs::tools::PageOp;
use swirvo_ui::{Fonts, Theme, Ui, snapshot};

fn photo(path: &Path, w: u32, h: u32, seed: u32) {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        let fx = x as f32 / w as f32;
        let fy = y as f32 / h as f32;
        let r = (120.0 + 120.0 * (fx * 3.0 + seed as f32).sin()) as u8;
        let g = (110.0 + 90.0 * (fy * 2.0 + seed as f32 * 0.7).cos()) as u8;
        let b = (150.0 + 100.0 * ((fx + fy) * 2.5 + seed as f32 * 1.3).sin()) as u8;
        image::Rgb([r, g, b])
    });
    img.save(path).expect("write sample image");
}

fn main() -> anyhow::Result<()> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&out)?;
    let dir = tempfile::tempdir()?;
    let d = dir.path();
    let mut files = Vec::new();
    for (i, (name, w, h)) in [
        ("IMG_2041.png", 1600, 1200),
        ("Beach sunset.jpg", 1200, 1600),
        ("Logo final v3.png", 800, 800),
        ("Screenshot 2026-11-08 at 10.42.17.png", 1920, 1080),
        ("Wedding photos (edited).jpg", 1500, 1000),
    ]
    .into_iter()
    .enumerate()
    {
        let p = d.join(name);
        photo(&p, w, h, i as u32);
        files.push(p);
    }
    let pages: Vec<PathBuf> = (0..7)
        .map(|i| {
            let p = d.join(format!("page{i}.png"));
            photo(&p, 600, 820, 10 + i);
            p
        })
        .collect();
    let pdf = d.join("Quarterly report.pdf");
    swirvo_engines::pdf_tools::images_to_pdf(&pages, &pdf, &swirvo_core::CancelToken::new())?;
    files.push(pdf.clone());
    std::fs::write(d.join("notes.md"), "# Notes\n\nHello *world*.\n")?;
    files.push(d.join("notes.md"));
    std::fs::write(d.join("data.json"), "[{\"a\":1}]")?;
    files.push(d.join("data.json"));

    let tk = Arc::new(Toolkit::with_builtin_engines()?);
    // A Word document, made from Markdown, for its page-one thumbnail.
    let md = d.join("Project brief.md");
    std::fs::write(
        &md,
        "# Project brief\n\nThe new site launches in **March**. This brief covers scope, \
         timeline and budget.\n\n## Scope\n\n- Home and product pages\n- Checkout\n\n\
         ## Timeline\n\nDesign in January, build in February.\n"
            .repeat(2),
    )?;
    if let Some(id) = tk.registry.id("docx") {
        swirvo_jobs::convert(
            &tk,
            std::slice::from_ref(&md),
            id,
            &swirvo_jobs::Options::default(),
            &swirvo_core::CancelToken::new(),
            &|_| {},
        );
        files.insert(1, d.join("Project brief.docx"));
    }
    let waker: swirvo_ui::Waker = Arc::new(|| {});
    let fonts = || Fonts::system().ok_or_else(|| anyhow::anyhow!("no UI font"));
    for (dark, tag) in [(true, "dark"), (false, "light")] {
        let mut ui = Ui::new(Theme::for_dark(dark), fonts()?);
        let mut b = Board::new(tk.clone(), waker.clone(), files.clone());
        b.settle(&ui);
        // Let thumbnails arrive.
        for _ in 0..60 {
            snapshot(&mut b, &mut ui, 1280.0, 800.0, 1.0, 2);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        b.select(&[1, 3]);
        let pm = snapshot(&mut b, &mut ui, 1280.0, 800.0, 1.0, 8);
        pm.save_png(out.join(format!("board-{tag}.png")))?;

        // The Tools tab for the PDF: the list, then a tool's options.
        let mut pb = Board::new(tk.clone(), waker.clone(), vec![pdf.clone()]);
        let mut uip = Ui::new(Theme::for_dark(dark), fonts()?);
        pb.settle(&uip);
        pb.open_tool("");
        let pm = snapshot(&mut pb, &mut uip, 1180.0, 760.0, 1.0, 8);
        pm.save_png(out.join(format!("board-tools-{tag}.png")))?;
        pb.open_tool("pdf-protect");
        let pm = snapshot(&mut pb, &mut uip, 1180.0, 760.0, 1.0, 8);
        pm.save_png(out.join(format!("board-form-{tag}.png")))?;

        let mut empty = Board::new(tk.clone(), waker.clone(), Vec::new());
        let mut ui2 = Ui::new(Theme::for_dark(dark), fonts()?);
        let pm = snapshot(&mut empty, &mut ui2, 1100.0, 700.0, 1.0, 8);
        pm.save_png(out.join(format!("board-empty-{tag}.png")))?;

        let mut ui3 = Ui::new(Theme::for_dark(dark), fonts()?);
        let mut p = PagePicker::new(
            tk.clone(),
            waker.clone(),
            pdf.clone(),
            PageOp::Images {
                png: false,
                dpi: 150,
            },
        );
        p.settle();
        let _ = p.set_range("1-3, 6");
        for _ in 0..40 {
            snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 2);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let pm = snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 8);
        pm.save_png(out.join(format!("pages-{tag}.png")))?;

        // Organise mode: a turned page, a blank page and marks, then the page editor.
        p.organise();
        p.org_select(&[1]);
        p.org_rotate_selected();
        p.org_add_blank();
        p.org_redact(0, [60.0, 500.0, 300.0, 540.0]);
        p.org_comment(
            0,
            swirvo_engines::pdfedit::annotate::Comment::Highlight {
                rect: [60.0, 600.0, 400.0, 630.0],
                color: [255, 214, 0],
            },
        );
        p.org_select(&[1, 2]);
        for _ in 0..40 {
            snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 2);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let pm = snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 8);
        pm.save_png(out.join(format!("organise-{tag}.png")))?;
        p.org_crop(0, [40.0, 60.0, 560.0, 800.0]);
        p.org_edit(Some(0));
        for _ in 0..40 {
            snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 2);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let pm = snapshot(&mut p, &mut ui3, 1040.0, 720.0, 1.0, 8);
        pm.save_png(out.join(format!("organise-editor-{tag}.png")))?;
    }
    println!("wrote snapshots to {}", out.display());
    Ok(())
}
