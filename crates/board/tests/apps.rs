//! Headless tests of the Board and the page picker: real conversions, no window.

#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use swirvo_board::{Board, PagePicker};
use swirvo_jobs::Toolkit;
use swirvo_jobs::tools::PageOp;
use swirvo_ui::{App, Fonts, Theme, Ui, snapshot};

fn tk() -> Arc<Toolkit> {
    Arc::new(Toolkit::with_builtin_engines().unwrap())
}

fn png(p: &Path, w: u32, h: u32) {
    image::RgbImage::from_pixel(w, h, image::Rgb([200, 120, 40]))
        .save(p)
        .unwrap();
}

fn ui() -> Option<Ui> {
    Fonts::system().map(|f| Ui::new(Theme::dark(), f))
}

#[test]
fn board_converts_a_mixed_batch() {
    let Some(ui) = ui() else { return };
    let d = tempfile::tempdir().unwrap();
    let (a, b, m) = (
        d.path().join("a.png"),
        d.path().join("b.png"),
        d.path().join("n.md"),
    );
    png(&a, 40, 30);
    png(&b, 30, 40);
    std::fs::write(&m, "# Hi\n\ntext\n").unwrap();
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    // A folder is expanded; duplicates are ignored.
    let mut board = Board::new(tk(), noop, vec![d.path().to_path_buf(), a.clone()]);
    board.settle(&ui);
    assert_eq!(board.paths().len(), 3);
    assert!(board.set_target("png", "jpg"));
    assert!(board.set_target("md", "html"));
    board.convert_and_wait(&ui);
    let st = board.statuses();
    assert!(
        st.contains(&("a.png".into(), "done:a.jpg".into())),
        "{st:?}"
    );
    assert!(
        st.contains(&("b.png".into(), "done:b.jpg".into())),
        "{st:?}"
    );
    assert!(
        st.contains(&("n.md".into(), "done:n.html".into())),
        "{st:?}"
    );
    assert!(d.path().join("a.jpg").exists() && d.path().join("n.html").exists());
    // Converting again only touches what isn't done: nothing left, nothing overwritten.
    board.convert_and_wait(&ui);
    assert!(!d.path().join("a (2).jpg").exists());
}

#[test]
fn page_picker_exports_a_subset() {
    let d = tempfile::tempdir().unwrap();
    let imgs: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = d.path().join(format!("{i}.png"));
            png(&p, 60 + i * 10, 80);
            p
        })
        .collect();
    let pdf = d.path().join("doc.pdf");
    swirvo_engines::pdf_tools::images_to_pdf(&imgs, &pdf, &swirvo_core::CancelToken::new())
        .unwrap();
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut p = PagePicker::new(tk(), noop, pdf.clone(), PageOp::Extract);
    p.settle();
    assert_eq!(p.range_text(), "all");
    p.set_range("3, 1").unwrap();
    assert_eq!(p.range_text(), "1, 3");
    assert!(p.set_range("9").is_err());
    let r = p.run_and_wait().unwrap().as_ref().unwrap().clone();
    let out = &r.outputs[0];
    assert!(out.ends_with("doc (pages 1, 3).pdf"), "{out:?}");
    assert_eq!(swirvo_engines::pdf_tools::page_count(out).unwrap(), 2);
}

#[test]
fn page_picker_splits_into_typed_groups() {
    let d = tempfile::tempdir().unwrap();
    let imgs: Vec<_> = (0..6)
        .map(|i| {
            let p = d.path().join(format!("{i}.png"));
            image::RgbImage::from_pixel(40, 60, image::Rgb([i * 40, 90, 200]))
                .save(&p)
                .unwrap();
            p
        })
        .collect();
    let pdf = d.path().join("doc.pdf");
    swirvo_engines::pdf_tools::images_to_pdf(&imgs, &pdf, &swirvo_core::CancelToken::new())
        .unwrap();
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut p = PagePicker::new(tk(), noop, pdf.clone(), PageOp::Split);
    p.settle();
    assert!(p.set_groups("1-2, 4; 9").unwrap_err().starts_with("PDF 2"));
    p.set_groups("1-2, 4; 5-6; 3").unwrap();
    if let (Some(mut ui), Ok(dir)) = (ui(), std::env::var("SWIRVO_SNAPSHOTS")) {
        let pm = snapshot(&mut p, &mut ui, 1000.0, 640.0, 1.0, 3);
        pm.save_png(std::path::Path::new(&dir).join("split-groups.png"))
            .unwrap();
    }
    let r = p.run_and_wait().unwrap().as_ref().unwrap().clone();
    let mut parts: Vec<_> = std::fs::read_dir(&r.outputs[0])
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    parts.sort();
    let counts: Vec<usize> = parts
        .iter()
        .map(|f| swirvo_engines::pdf_tools::page_count(f).unwrap())
        .collect();
    assert_eq!(counts, [3, 2, 1], "{parts:?}");
    assert!(parts[0].ends_with("doc 1 (p1-2, 4).pdf"), "{parts:?}");
}

#[test]
fn both_apps_render_frames() {
    let Some(mut ui) = ui() else { return };
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut board = Board::new(tk(), noop.clone(), Vec::new());
    let pm = snapshot(&mut board, &mut ui, 900.0, 600.0, 1.25, 3);
    assert_eq!((pm.width(), pm.height()), (1125, 750));
    assert!(board.title().contains("Board"));
    // The title bar is a caption zone; the close button is a Close zone.
    assert_eq!(
        ui.zone_at(swirvo_ui::Point::new(450.0, 20.0)),
        Some(swirvo_ui::Zone::Caption)
    );
    assert_eq!(
        ui.zone_at(swirvo_ui::Point::new(890.0, 20.0)),
        Some(swirvo_ui::Zone::Close)
    );
    assert_eq!(
        ui.zone_at(swirvo_ui::Point::new(830.0, 20.0)),
        Some(swirvo_ui::Zone::Maximize)
    );
    assert_eq!(
        ui.zone_at(swirvo_ui::Point::new(785.0, 20.0)),
        Some(swirvo_ui::Zone::Minimize)
    );
    // The "Add files" button sits in the title bar but stays clickable.
    assert_eq!(ui.zone_at(swirvo_ui::Point::new(225.0, 24.0)), None);
}

#[test]
fn caption_buttons_with_files_loaded() {
    let Some(mut ui) = ui() else { return };
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.png");
    png(&a, 20, 20);
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut board = Board::new(tk(), noop, vec![a]);
    board.settle(&ui);
    snapshot(&mut board, &mut ui, 980.0, 617.6, 1.25, 3);
    for x in [850.0, 865.0, 880.0] {
        assert_eq!(
            ui.zone_at(swirvo_ui::Point::new(x, 16.0)),
            Some(swirvo_ui::Zone::Minimize),
            "x={x}"
        );
    }
}

/// Organise mode end to end: reorder, turn, blank page, a page from another PDF, a
/// black-out box and a note, then one new PDF in that order. The original is unchanged.
#[test]
fn organise_mode_builds_the_new_pdf() {
    use swirvo_engines::pdfedit::annotate::Comment;
    let d = tempfile::tempdir().unwrap();
    let imgs: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = d.path().join(format!("{i}.png"));
            png(&p, 60 + i * 10, 80);
            p
        })
        .collect();
    let pdf = d.path().join("doc.pdf");
    let other = d.path().join("other.pdf");
    let cancel = swirvo_core::CancelToken::new();
    swirvo_engines::pdf_tools::images_to_pdf(&imgs, &pdf, &cancel).unwrap();
    swirvo_engines::pdf_tools::images_to_pdf(&imgs[..2], &other, &cancel).unwrap();
    let before = std::fs::read(&pdf).unwrap();

    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut p = PagePicker::new(tk(), noop, pdf.clone(), PageOp::Extract);
    p.settle();
    p.organise();
    assert_eq!(p.org_len(), 3);
    // 3 1 2, then turn the first, a blank page after it, the other PDF at the end.
    p.org_move(&[2], 0);
    p.org_select(&[0]);
    p.org_rotate_selected();
    p.org_add_blank();
    p.org_insert_pdf(other, 4);
    p.org_settle();
    assert_eq!(
        p.org_order(),
        [
            Some((0, 2)),
            None,
            Some((0, 0)),
            Some((0, 1)),
            Some((1, 0)),
            Some((1, 1))
        ]
    );
    // Duplicate and undo leave it as it was.
    p.org_select(&[5]);
    p.org_duplicate_selected();
    assert_eq!(p.org_len(), 7);
    p.org_undo();
    assert_eq!(p.org_len(), 6);
    p.org_redact(2, [0.0, 0.0, 30.0, 30.0]);
    p.org_comment(
        3,
        Comment::Note {
            at: [10.0, 70.0],
            text: "check".into(),
            color: [255, 210, 0],
        },
    );
    let r = p.run_and_wait().unwrap().as_ref().unwrap().clone();
    let out = &r.outputs[0];
    assert!(out.ends_with("doc (organised).pdf"), "{out:?}");
    assert!(r.message.contains("6 pages"), "{}", r.message);
    assert_eq!(swirvo_engines::pdf_tools::page_count(out).unwrap(), 6);
    assert_eq!(
        std::fs::read(&pdf).unwrap(),
        before,
        "the original is untouched"
    );
}

#[test]
fn organise_mode_renders_the_grid_and_the_editor() {
    let Some(mut ui) = ui() else { return };
    let d = tempfile::tempdir().unwrap();
    let imgs: Vec<PathBuf> = (0..2)
        .map(|i| {
            let p = d.path().join(format!("{i}.png"));
            png(&p, 60, 80);
            p
        })
        .collect();
    let pdf = d.path().join("doc.pdf");
    swirvo_engines::pdf_tools::images_to_pdf(&imgs, &pdf, &swirvo_core::CancelToken::new())
        .unwrap();
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut p = PagePicker::new(tk(), noop, pdf, PageOp::Extract);
    p.settle();
    p.organise();
    p.org_select(&[1]);
    let pm = snapshot(&mut p, &mut ui, 1060.0, 740.0, 1.0, 3);
    assert_eq!((pm.width(), pm.height()), (1060, 740));
    p.org_crop(0, [5.0, 5.0, 40.0, 50.0]);
    p.org_edit(Some(0));
    let pm = snapshot(&mut p, &mut ui, 1060.0, 740.0, 1.0, 3);
    assert_eq!((pm.width(), pm.height()), (1060, 740));
}

/// Word, PowerPoint, Excel and OpenDocument files get a picture of their first page,
/// laid out by our own engines off the UI thread.
#[test]
fn documents_get_page_one_thumbnails() {
    if ui().is_none() {
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let md = d.path().join("notes.md");
    let text = "# Quarterly notes\n\nRevenue grew **12 %** this quarter.\n\n- one\n- two\n";
    std::fs::write(&md, text.repeat(3)).unwrap();
    let tk = tk();
    for to in ["docx", "odt"] {
        let id = tk.registry.id(to).unwrap();
        let s = swirvo_jobs::convert(
            &tk,
            std::slice::from_ref(&md),
            id,
            &swirvo_jobs::Options::default(),
            &swirvo_core::CancelToken::new(),
            &|_| {},
        );
        assert_eq!(s.failed, 0, "{to}");
    }
    let decode = swirvo_board::decoder(tk);
    for name in ["notes.docx", "notes.odt"] {
        let img = decode(&d.path().join(name), 160).unwrap_or_else(|| panic!("{name}"));
        let (w, h) = img.dimensions();
        assert!(h == 160 && w < h, "{name}: a portrait page, {w}×{h}");
        // At this size body text is ~2 px tall and anti-aliased to gray.
        let ink = img
            .pixels()
            .filter(|p| p[0].min(p[1]).min(p[2]) < 200)
            .count();
        assert!(ink > 30, "{name}: the heading and text are drawn ({ink})");
    }
    // Not a document: no thumbnail, and no error.
    assert!(decode(&md, 160).is_none());
}

#[test]
fn board_previews_pdf_tools_on_a_page() {
    let Some(mut ui) = ui() else { return };
    let d = tempfile::tempdir().unwrap();
    let imgs: Vec<PathBuf> = (0..3)
        .map(|i| {
            let p = d.path().join(format!("{i}.png"));
            png(&p, 300, 420);
            p
        })
        .collect();
    let pdf = d.path().join("doc.pdf");
    swirvo_engines::pdf_tools::images_to_pdf(&imgs, &pdf, &swirvo_core::CancelToken::new())
        .unwrap();
    for p in imgs {
        std::fs::remove_file(p).unwrap();
    }
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut board = Board::new(tk(), noop, vec![pdf.clone()]);
    board.settle(&ui);
    board.open_tool("pdf-watermark");
    board.form_set("text", "DRAFT");
    assert_eq!(board.preview_wait(), Some((0, 1)));
    let pm = snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 3);
    if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
        pm.save_png(std::path::Path::new(&dir).join("preview-watermark.png"))
            .unwrap();
    }
    board.open_tool("pdf-nup");
    board.form_set("per_sheet", "4");
    assert_eq!(board.preview_wait(), Some((0, 1)));
    board.open_tool("pdf-crop");
    board.form_set("custom", "2: 0, 0, 30, 0");
    assert!(board.preview_wait().is_some());
    let pm = snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 3);
    if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
        pm.save_png(std::path::Path::new(&dir).join("preview-crop.png"))
            .unwrap();
    }
    // The signature box is placed on the page (nothing runs: signing needs a certificate).
    board.open_tool("pdf-sign");
    board.form_set("place", "2: 20, 30");
    for _ in 0..40 {
        snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 1);
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let pm = snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 2);
    if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
        pm.save_png(std::path::Path::new(&dir).join("preview-sign.png"))
            .unwrap();
    }
    // Split ranges: a row per PDF, checked against the page count as it's typed.
    board.open_tool("pdf-split-ranges");
    board.form_set("ranges", "1-2, 3; 3; 2-9");
    for _ in 0..20 {
        snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 1);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let pm = snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 2);
    if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
        pm.save_png(std::path::Path::new(&dir).join("split-ranges-form.png"))
            .unwrap();
    }
    // Previews write nothing next to the PDF.
    let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().collect();
    assert_eq!(names.len(), 1);
}

#[test]
fn search_narrows_files_and_tools() {
    let Some(mut ui) = ui() else { return };
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (
        d.path().join("holiday.png"),
        d.path().join("Report 2026.png"),
    );
    png(&a, 40, 30);
    png(&b, 40, 30);
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut board = Board::new(tk(), noop, vec![a, b]);
    board.settle(&ui);
    board.set_search("report", false);
    assert_eq!(board.shown_files(), ["Report 2026.png"]);
    board.set_search("RE 2026", false);
    assert_eq!(board.shown_files(), ["Report 2026.png"]);
    board.set_search("split", true);
    assert_eq!(
        board.shown_files().len(),
        2,
        "a tool search keeps the files"
    );
    let pm = snapshot(&mut board, &mut ui, 1100.0, 700.0, 1.0, 3);
    if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
        pm.save_png(std::path::Path::new(&dir).join("search-tools.png"))
            .unwrap();
    }
    board.set_search("", false);
    assert_eq!(board.shown_files().len(), 2);
}

#[test]
fn settings_window_renders_every_section() {
    let Some(mut ui) = ui() else { return };
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut s =
        swirvo_board::SettingsApp::new(noop, None, swirvo_board::InstallState::NotInstalled);
    assert_eq!(s.title(), "Swirvo Settings");
    for (k, name) in ["general", "output", "wheel", "engines", "privacy", "about"]
        .iter()
        .enumerate()
    {
        s.show(k);
        let pm = snapshot(&mut s, &mut ui, 900.0, 640.0, 1.0, 3);
        assert_eq!(pm.width(), 900);
        if let Ok(dir) = std::env::var("SWIRVO_SNAPSHOTS") {
            pm.save_png(std::path::Path::new(&dir).join(format!("settings-{name}.png")))
                .unwrap();
        }
    }
    // Nothing was changed, so nothing was saved or asked of the host.
    assert!(s.take_requests().is_empty());
}
