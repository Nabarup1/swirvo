//! Writes the Swirvo icon assets from the one geometry in `swirvo_render::icon`:
//! `packaging/icon/` (SVGs, PNGs, the multi-size `.ico`) and `packaging/msix/Assets/`
//! (the Store's tile set).
//!
//! `cargo run -p swirvo-render --example icon_assets [repo-root]`

use std::path::{Path, PathBuf};

use swirvo_render::{Pixmap, app_icon, icon};

fn png(px: u32) -> Vec<u8> {
    app_icon(px).expect("icon size").encode_png().expect("png")
}

/// The icon centred on a transparent `w` × `h` canvas at `scale` of the shorter side.
fn tile(w: u32, h: u32, scale: f32) -> Vec<u8> {
    let side = ((w.min(h) as f32) * scale).round() as u32;
    let mark = app_icon(side).expect("icon size");
    let mut pm = Pixmap::new(w, h).expect("tile size");
    pm.draw_pixmap(
        ((w - side) / 2) as i32,
        ((h - side) / 2) as i32,
        mark.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    pm.encode_png().expect("png")
}

/// A Windows icon file holding PNG images (Windows Vista and later read these at every
/// size).
fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        out.extend_from_slice(&[dim, dim, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(img.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend_from_slice(&img);
    }
    out
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, bytes).expect("write");
}

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let dir = root.join("packaging/icon");
    write(&dir.join("swirvo.svg"), icon::svg(false).as_bytes());
    write(&dir.join("swirvo-small.svg"), icon::svg(true).as_bytes());
    for px in [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512] {
        write(&dir.join(format!("png/swirvo-{px}.png")), &png(px));
    }
    write(
        &dir.join("swirvo.ico"),
        &ico(&[16, 20, 24, 32, 40, 48, 64, 256]),
    );

    // MSIX visual assets (names match AppxManifest.xml; scale and target-size
    // qualifiers let Windows pick the right one).
    let a = root.join("packaging/msix/Assets");
    for t in [16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256] {
        write(
            &a.join(format!("Square44x44Logo.targetsize-{t}.png")),
            &png(t),
        );
        write(
            &a.join(format!(
                "Square44x44Logo.targetsize-{t}_altform-unplated.png"
            )),
            &png(t),
        );
        write(
            &a.join(format!(
                "Square44x44Logo.targetsize-{t}_altform-lightunplated.png"
            )),
            &png(t),
        );
    }
    for (scale, k) in [(100, 1.0), (125, 1.25), (150, 1.5), (200, 2.0), (400, 4.0)] {
        let s = |n: f32| (n * k).round() as u32;
        write(
            &a.join(format!("Square44x44Logo.scale-{scale}.png")),
            &png(s(44.0)),
        );
        write(
            &a.join(format!("Square150x150Logo.scale-{scale}.png")),
            &tile(s(150.0), s(150.0), 0.66),
        );
        write(
            &a.join(format!("Wide310x150Logo.scale-{scale}.png")),
            &tile(s(310.0), s(150.0), 0.66),
        );
        write(
            &a.join(format!("SmallTile.scale-{scale}.png")),
            &tile(s(71.0), s(71.0), 0.7),
        );
        write(
            &a.join(format!("LargeTile.scale-{scale}.png")),
            &tile(s(310.0), s(310.0), 0.6),
        );
        write(
            &a.join(format!("StoreLogo.scale-{scale}.png")),
            &png(s(50.0)),
        );
        write(
            &a.join(format!("SplashScreen.scale-{scale}.png")),
            &tile(s(620.0), s(300.0), 0.5),
        );
    }
    // A zoomed contact sheet for checking the small sizes by eye (not an asset).
    if let Some(sheet) = std::env::var_os("SWIRVO_ICON_PREVIEW") {
        let sizes = [16u32, 20, 24, 32, 48, 64];
        let zoom = 6;
        let cell = 64 * zoom + 24;
        let mut pm = Pixmap::new(cell * sizes.len() as u32, cell * 2).expect("sheet");
        for (row, bg) in [(0u32, [245u8, 245, 245]), (1, [32, 32, 36])] {
            let mut paint = tiny_skia::Paint::default();
            paint.set_color_rgba8(bg[0], bg[1], bg[2], 255);
            if let Some(r) =
                tiny_skia::Rect::from_xywh(0.0, (row * cell) as f32, pm.width() as f32, cell as f32)
            {
                pm.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
            }
            for (i, &s) in sizes.iter().enumerate() {
                let ic = app_icon(s).expect("icon");
                let k = (zoom * 64 / s.max(1)) as f32 / 1.0;
                let k = k.min(zoom as f32 * 64.0 / s as f32);
                let ppaint = tiny_skia::PixmapPaint {
                    quality: tiny_skia::FilterQuality::Nearest,
                    ..Default::default()
                };
                pm.draw_pixmap(
                    0,
                    0,
                    ic.as_ref(),
                    &ppaint,
                    tiny_skia::Transform::from_row(
                        k,
                        0.0,
                        0.0,
                        k,
                        (i as u32 * cell + 12) as f32,
                        (row * cell + 12) as f32,
                    ),
                    None,
                );
            }
        }
        pm.save_png(sheet).expect("sheet png");
    }
    println!(
        "icon assets written under {}",
        root.join("packaging").display()
    );
}
