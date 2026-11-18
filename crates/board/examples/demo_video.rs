//! The launch demo video: the real Board, page picker, previews and wheel, driven by a
//! script, composed with titles, captions and a pointer, and piped to FFmpeg.
//!
//! `cargo run --release -p swirvo-board --example demo_video -- <samples dir> <ffmpeg> <out.mp4>`
//!
//! The samples dir holds the made-up files the demo uses (no real documents or people).
//! Nothing here is faked: every screen is the app drawing itself; only the pointer, the
//! key cap and the folder window of the first scene are drawn on top.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use swirvo_board::{Board, PagePicker};
use swirvo_jobs::Toolkit;
use swirvo_jobs::tools::{PageOp, tools_for};
use swirvo_ui::theme::size;
use swirvo_ui::{
    Align, App, Fonts, Icon, Key, Kind, Mods, Pixmap, Point, Rect, Rgba, Theme, Ui, Weight,
};

const W: u32 = 1920;
const H: u32 = 1080;
const FPS: f32 = 30.0;
/// Apps are drawn this much larger than 1:1, so their text reads on a phone.
const APP_SCALE: f32 = 1.25;
/// The wheel's size on the canvas relative to its natural size.
const WHEEL_ZOOM: f32 = 1.6;

/// A point given in an app's logical px, on the canvas.
fn app_pt(x: f32, y: f32) -> Point {
    Point::new(APP.x + x * APP_SCALE, APP.y + y * APP_SCALE)
}
/// Where app windows sit on the canvas (logical px, 1:1).
const APP: Rect = Rect {
    x: 160.0,
    y: 36.0,
    w: 1600.0,
    h: 900.0,
};

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_pt(a: Point, b: Point, t: f32) -> Point {
    Point::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

/// The output: frames go straight into FFmpeg's stdin as raw RGBA.
struct Out {
    child: std::process::Child,
    frames: usize,
}

impl Out {
    fn push(&mut self, pm: &Pixmap) {
        let stdin = self.child.stdin.as_mut().expect("ffmpeg stdin");
        stdin.write_all(pm.data()).expect("ffmpeg closed");
        self.frames += 1;
    }
}

/// One frame of an app at the app size, with the glass backdrop the snapshots use.
fn render_app(app: &mut dyn App, ui: &mut Ui, input: impl FnOnce(&mut Ui)) -> Pixmap {
    let (pw, ph) = (APP.w as u32, APP.h as u32);
    ui.backdrop = true;
    ui.input.dt = 1.0 / FPS;
    ui.input.time += 1.0 / FPS as f64;
    input(ui);
    ui.begin_frame(pw, ph, APP_SCALE);
    app.frame(ui);
    ui.end_frame();
    let mut out = Pixmap::new(pw, ph).expect("pixmap");
    swirvo_ui::fake_backdrop(&mut out, ui.theme.dark);
    out.draw_pixmap(
        0,
        0,
        ui.pixmap().as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::identity(),
        None,
    );
    out
}

/// The canvas everything is composed on.
struct Canvas {
    ui: Ui,
    icon: Option<Pixmap>,
}

impl Canvas {
    fn begin(&mut self) {
        self.ui.input.dt = 1.0 / FPS;
        self.ui.input.time += 1.0 / FPS as f64;
        self.ui.begin_frame(W, H, 1.0);
        // Background: deep blue-violet with a warm glow, like a desktop.
        let ui = &mut self.ui;
        ui.gradient_rrect(
            Rect::new(0.0, 0.0, W as f32, H as f32),
            0.0,
            Rgba::rgb(0x161A2B),
            Rgba::rgb(0x1E1330),
        );
        ui.fill_circle(1500.0, 180.0, 520.0, Rgba::hexa(0xE8772E, 26));
        ui.fill_circle(300.0, 980.0, 460.0, Rgba::hexa(0x5B5FD6, 22));
    }

    fn end(&mut self) -> Pixmap {
        self.ui.end_frame();
        self.ui.pixmap().clone()
    }

    fn window(&mut self, pm: &Pixmap, alpha: f32) {
        let ui = &mut self.ui;
        ui.shadow(APP, 12.0, 40.0, 14.0, Rgba::BLACK.fade(0.55 * alpha));
        ui.image(APP, pm, 12.0, false);
        if alpha < 1.0 {
            ui.fill_rrect(
                APP,
                12.0,
                Rgba::hexa(0x161A2B, ((1.0 - alpha) * 255.0) as u8),
            );
        }
    }

    /// A caption pill at the bottom.
    fn caption(&mut self, text: &str, alpha: f32) {
        if alpha <= 0.01 || text.is_empty() {
            return;
        }
        let ui = &mut self.ui;
        let w = ui.measure(text, 30.0, Weight::Semibold) + 64.0;
        let r = Rect::new((W as f32 - w) / 2.0, 966.0, w, 70.0);
        ui.fill_rrect(r, 35.0, Rgba::hexa(0x0D0F18, (215.0 * alpha) as u8));
        ui.stroke_rrect(r, 35.0, Rgba::WHITE.fade(0.12 * alpha));
        ui.text_in(
            r,
            text,
            30.0,
            Weight::Semibold,
            Rgba::WHITE.fade(alpha),
            Align::Center,
        );
    }

    /// A key cap ("Shift") at the top, pressed.
    fn key(&mut self, label: &str, what: &str, alpha: f32) {
        if alpha <= 0.01 {
            return;
        }
        let ui = &mut self.ui;
        let r = Rect::new(W as f32 / 2.0 - 70.0, 70.0, 140.0, 64.0);
        ui.shadow(r, 12.0, 14.0, 4.0, Rgba::BLACK.fade(0.4 * alpha));
        ui.fill_rrect(r, 12.0, Rgba::hexa(0xF2F2F5, (255.0 * alpha) as u8));
        ui.stroke_rrect_w(r, 12.0, Rgba::hexa(0xE8772E, (255.0 * alpha) as u8), 3.0);
        ui.text_in(
            r,
            label,
            26.0,
            Weight::Semibold,
            Rgba::hexa(0x1B1B22, (255.0 * alpha) as u8),
            Align::Center,
        );
        let t = Rect::new(r.x - 200.0, r.bottom() + 10.0, r.w + 400.0, 30.0);
        ui.text_in(
            t,
            what,
            20.0,
            Weight::Regular,
            Rgba::WHITE.fade(0.8 * alpha),
            Align::Center,
        );
    }

    /// The pointer arrow, tip at `p`.
    fn pointer(&mut self, p: Point, pressed: bool) {
        let pts = [
            (0.0, 0.0),
            (0.0, 26.0),
            (6.5, 20.0),
            (11.0, 30.0),
            (15.0, 28.0),
            (10.5, 18.5),
            (19.0, 18.5),
        ];
        let k = if pressed { 0.92 } else { 1.0 };
        let path = |pb: &mut swirvo_ui::PathBuilder, s: f32, grow: f32| {
            for (i, (x, y)) in pts.iter().enumerate() {
                let (x, y) = ((p.x + x * k * grow) * s, (p.y + y * k * grow) * s);
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
            pb.close();
        };
        self.ui
            .fill_logical(|pb, s| path(pb, s, 1.12), Rgba::BLACK.fade(0.85));
        self.ui.fill_logical(|pb, s| path(pb, s, 1.0), Rgba::WHITE);
    }

    fn title(&mut self, big: &str, small: &str, alpha: f32, y: f32) {
        let ui = &mut self.ui;
        if let Some(icon) = &self.icon {
            let r = Rect::new(W as f32 / 2.0 - 80.0, y - 220.0, 160.0, 160.0);
            ui.shadow(r, 36.0, 30.0, 10.0, Rgba::BLACK.fade(0.4 * alpha));
            let icon = icon.clone();
            ui.image(r, &icon, 0.0, false);
            if alpha < 1.0 {
                // Fade by covering with the background tone.
                ui.fill_rect(
                    r.inset(-6.0),
                    Rgba::hexa(0x191630, ((1.0 - alpha) * 255.0) as u8),
                );
            }
        }
        let r = Rect::new(0.0, y - 40.0, W as f32, 100.0);
        ui.text_in(
            r,
            big,
            84.0,
            Weight::Semibold,
            Rgba::WHITE.fade(alpha),
            Align::Center,
        );
        let r2 = Rect::new(0.0, y + 70.0, W as f32, 60.0);
        ui.text_in(
            r2,
            small,
            32.0,
            Weight::Regular,
            Rgba::WHITE.fade(0.75 * alpha),
            Align::Center,
        );
    }
}

/// The made-up Downloads folder of the first scene, drawn as a plain window.
struct Folder {
    rows: Vec<(String, Kind, String)>,
}

impl Folder {
    const R: Rect = Rect {
        x: 330.0,
        y: 170.0,
        w: 1260.0,
        h: 700.0,
    };

    fn row_rect(i: usize) -> Rect {
        Rect::new(
            Self::R.x + 24.0,
            Self::R.y + 110.0 + i as f32 * 62.0,
            Self::R.w - 48.0,
            54.0,
        )
    }

    fn draw(
        &self,
        ui: &mut Ui,
        hot: Option<usize>,
        fresh: Option<&(String, Kind, String)>,
        fresh_a: f32,
    ) {
        let t = Theme::dark();
        ui.shadow(Self::R, 14.0, 40.0, 14.0, Rgba::BLACK.fade(0.5));
        ui.fill_rrect(Self::R, 14.0, Rgba::hexa(0x202433, 250));
        ui.stroke_rrect(Self::R, 14.0, Rgba::WHITE.fade(0.08));
        let head = Rect::new(Self::R.x + 28.0, Self::R.y + 22.0, 600.0, 40.0);
        ui.icon(
            Rect::new(head.x, head.y + 8.0, 24.0, 24.0),
            Icon::Folder,
            t.accent,
        );
        ui.text_in(
            head.translate(36.0, 0.0),
            "Downloads",
            24.0,
            Weight::Semibold,
            t.text,
            Align::Left,
        );
        ui.hline(
            Self::R.x + 24.0,
            Self::R.right() - 24.0,
            Self::R.y + 84.0,
            Rgba::WHITE.fade(0.08),
        );
        let mut rows: Vec<&(String, Kind, String)> = self.rows.iter().collect();
        if let Some(f) = fresh {
            rows.insert(1, f);
        }
        for (i, (name, kind, meta)) in rows.iter().enumerate() {
            let r = Self::row_rect(i);
            let is_fresh = fresh.is_some() && i == 1;
            if hot == Some(i) && !is_fresh {
                ui.fill_rrect(r, 8.0, t.accent.fade(0.18));
            }
            if is_fresh {
                ui.fill_rrect(r, 8.0, t.success.fade(0.16 * fresh_a));
            }
            let a = if is_fresh { fresh_a } else { 1.0 };
            ui.kind_badge(Rect::new(r.x + 12.0, r.y + 9.0, 36.0, 36.0), *kind);
            ui.text_in(
                Rect::new(r.x + 64.0, r.y, 700.0, r.h),
                name,
                21.0,
                Weight::Regular,
                t.text.fade(a),
                Align::Left,
            );
            ui.text_in(
                Rect::new(r.right() - 340.0, r.y, 320.0, r.h),
                meta,
                18.0,
                Weight::Regular,
                t.text2.fade(a),
                Align::Right,
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, samples, ffmpeg, out] = &args[..] else {
        eprintln!("usage: demo_video <samples dir> <ffmpeg> <out.mp4>");
        std::process::exit(2);
    };
    let samples = PathBuf::from(samples);
    let tk = Arc::new(Toolkit::with_builtin_engines().expect("toolkit"));
    let fonts = || Fonts::system().expect("fonts");
    let mut canvas = Canvas {
        ui: Ui::new(Theme::dark(), fonts()),
        icon: swirvo_render::app_icon(320),
    };
    let wheel_font = swirvo_render::load_ui_font();
    let child = Command::new(ffmpeg)
        .args([
            "-y",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            "1920x1080",
            "-r",
            "30",
            "-i",
            "-",
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            "17",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            out,
        ])
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut video = Out { child, frames: 0 };
    let secs = |s: f32| (s * FPS).round() as usize;

    // A fresh Downloads folder with copies of the samples, so outputs land in it.
    let work = samples.join("Downloads");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("work dir");
    let pick = [
        "Quarterly report.docx",
        "Team handbook.pdf",
        "Resume - Jane Doe.pdf",
        "Sunset.png",
        "Project plan.odt",
        "Meeting notes.md",
        "Budget.csv",
    ];
    for f in pick {
        std::fs::copy(samples.join(f), work.join(f)).expect(f);
    }
    let in_work = |f: &str| work.join(f);

    // ── 1. Title ─────────────────────────────────────────────────────────────────────
    for i in 0..secs(3.2) {
        let t = i as f32 / FPS;
        let a = ease(t / 0.8) * (1.0 - ease((t - 2.6) / 0.6));
        canvas.begin();
        canvas.title(
            "Swirvo",
            "Drag. Tap. Convert. Everything stays on your PC.",
            a,
            560.0,
        );
        video.push(&canvas.end());
    }

    // ── 2. The gesture: drag, tap Shift, drop on PDF ─────────────────────────────────
    let folder = Folder {
        rows: vec![
            (
                "Quarterly report.docx".into(),
                Kind::Document,
                "Word document · 6 KB".into(),
            ),
            (
                "Team handbook.pdf".into(),
                Kind::Document,
                "PDF · 13 pages".into(),
            ),
            (
                "Resume - Jane Doe.pdf".into(),
                Kind::Document,
                "PDF · 1 page".into(),
            ),
            (
                "Sunset.png".into(),
                Kind::Image,
                "PNG image · 1600 × 1066".into(),
            ),
            (
                "Project plan.odt".into(),
                Kind::Document,
                "OpenDocument · 53 KB".into(),
            ),
            (
                "Meeting notes.md".into(),
                Kind::Document,
                "Markdown · 1 KB".into(),
            ),
            ("Budget.csv".into(), Kind::Data, "CSV · 1 KB".into()),
        ],
    };
    let docx_fmt = tk.registry.id("docx").expect("docx");
    let (items, more) = swirvo_jobs::wheel_items(&tk, &[docx_fmt], &Default::default(), 0);
    let mut wedges: Vec<swirvo_render::Wedge> = items
        .iter()
        .map(|i| swirvo_render::Wedge {
            label: i.label.clone(),
            caption: i.caption.clone(),
        })
        .collect();
    if more > 0 {
        wedges.push(swirvo_render::Wedge {
            label: "More…".into(),
            caption: format!("{more} more"),
        });
    }
    let pdf_slot = items.iter().position(|i| i.label == "PDF").unwrap_or(0);
    let n = wedges.len().max(1);
    let wheel_at = |hover: Option<usize>,
                    which: usize,
                    wedges: &Vec<swirvo_render::Wedge>,
                    title: &str,
                    sub: &str| {
        swirvo_render::Scene {
            wedges: wedges.clone(),
            hover,
            wheel: which,
            hub_title: title.into(),
            hub_subtitle: sub.into(),
            theme: swirvo_render::Theme::Dark,
            scale: 1.25 * WHEEL_ZOOM,
            sub: None,
            hit_radius: 0.0,
        }
    };
    let start = Folder::row_rect(0).center();
    let drag_to = Point::new(960.0, 560.0);
    // The wedge centre: slot 0 at 12 o'clock, clockwise.
    let ang =
        |slot: usize| -std::f32::consts::FRAC_PI_2 + slot as f32 * std::f32::consts::TAU / n as f32;
    let wedge_pt = |slot: usize| {
        Point::new(
            drag_to.x + 140.0 * WHEEL_ZOOM * ang(slot).cos(),
            drag_to.y + 140.0 * WHEEL_ZOOM * ang(slot).sin(),
        )
    };
    let plain = swirvo_render::render(
        &wheel_at(None, 0, &wedges, "Quarterly…", "6 KB"),
        wheel_font.as_ref(),
    );
    let lit = swirvo_render::render(
        &wheel_at(Some(pdf_slot), 0, &wedges, "Quarterly…", "6 KB"),
        wheel_font.as_ref(),
    );
    let total = secs(9.0);
    for i in 0..total {
        let t = i as f32 / FPS;
        canvas.begin();
        let fresh = (
            "Quarterly report.pdf".to_string(),
            Kind::Document,
            "PDF · just now".to_string(),
        );
        let done = t > 6.2;
        folder.draw(
            &mut canvas.ui,
            if t < 6.2 { Some(0) } else { None },
            done.then_some(&fresh),
            ease((t - 6.2) / 0.5),
        );
        let (p, pressed) = if t < 0.8 {
            (
                lerp_pt(Point::new(1200.0, 820.0), start, ease(t / 0.8)),
                false,
            )
        } else if t < 2.4 {
            (lerp_pt(start, drag_to, ease((t - 1.0) / 1.2)), true)
        } else if t < 5.6 {
            let to = wedge_pt(pdf_slot);
            (lerp_pt(drag_to, to, ease((t - 3.8) / 0.9)), true)
        } else {
            (
                lerp_pt(
                    wedge_pt(pdf_slot),
                    Point::new(1250.0, 700.0),
                    ease((t - 6.0) / 1.5),
                ),
                t < 6.0,
            )
        };
        // The file being dragged.
        if (1.0..2.8).contains(&t) {
            let chip = Rect::new(p.x + 18.0, p.y + 22.0, 260.0, 44.0);
            canvas.ui.fill_rrect(chip, 10.0, Rgba::hexa(0x2A2F42, 230));
            canvas.ui.kind_badge(
                Rect::new(chip.x + 8.0, chip.y + 7.0, 30.0, 30.0),
                Kind::Document,
            );
            canvas.ui.text_in(
                chip.translate(48.0, 0.0),
                "Quarterly report.docx",
                17.0,
                Weight::Regular,
                Rgba::WHITE,
                Align::Left,
            );
        }
        // Shift, then the wheel.
        let key_a = ease((t - 2.5) / 0.25) * (1.0 - ease((t - 5.9) / 0.3));
        canvas.key("Shift", "Convert", key_a);
        if (2.7..6.1).contains(&t) {
            let grow = ease((t - 2.7) / 0.25) * (1.0 - ease((t - 5.85) / 0.25));
            let pm = if t > 4.5 { &lit } else { &plain };
            if let Some(pm) = pm {
                let (w, h) = (
                    pm.width() as f32 / 1.25 * grow,
                    pm.height() as f32 / 1.25 * grow,
                );
                let r = Rect::new(drag_to.x - w / 2.0, drag_to.y - h / 2.0, w, h);
                canvas.ui.image(r, pm, 0.0, false);
            }
        }
        canvas.pointer(p, pressed);
        if done {
            let a = ease((t - 6.3) / 0.4);
            let toast = Rect::new(1260.0, 900.0, 520.0, 64.0);
            canvas
                .ui
                .fill_rrect(toast, 14.0, Rgba::hexa(0x23283A, (240.0 * a) as u8));
            canvas.ui.icon(
                Rect::new(toast.x + 18.0, toast.y + 20.0, 24.0, 24.0),
                Icon::Check,
                Rgba::hexa(0x5FD08A, (255.0 * a) as u8),
            );
            canvas.ui.text_in(
                toast.translate(54.0, 0.0),
                "Quarterly report.pdf · saved next to the original",
                18.0,
                Weight::Regular,
                Rgba::WHITE.fade(a),
                Align::Left,
            );
        }
        let cap = if t < 2.4 {
            "Start dragging any file…"
        } else if t < 6.0 {
            "…tap Shift, drop it on a format"
        } else {
            "Done. The original is never touched."
        };
        canvas.caption(cap, 1.0);
        video.push(&canvas.end());
    }

    // ── 3. Ctrl: the Tools wheel on a PDF ────────────────────────────────────────────
    let pdf_tools: Vec<swirvo_render::Wedge> = tools_for(&tk, &[in_work("Resume - Jane Doe.pdf")])
        .into_iter()
        .take(7)
        .map(|t| {
            let (l, c) = t.label();
            swirvo_render::Wedge {
                label: l.into(),
                caption: c.into(),
            }
        })
        .collect();
    let tools_lit = swirvo_render::render(
        &wheel_at(Some(1), 1, &pdf_tools, "Resume - J…", "1 page"),
        wheel_font.as_ref(),
    );
    for i in 0..secs(3.4) {
        let t = i as f32 / FPS;
        canvas.begin();
        folder.draw(&mut canvas.ui, Some(2), None, 0.0);
        canvas.key(
            "Ctrl",
            "Tools",
            ease(t / 0.2) * (1.0 - ease((t - 3.0) / 0.3)),
        );
        if let Some(pm) = &tools_lit {
            let grow = ease(t / 0.25) * (1.0 - ease((t - 3.0) / 0.3));
            let (w, h) = (
                pm.width() as f32 / 1.25 * grow,
                pm.height() as f32 / 1.25 * grow,
            );
            canvas.ui.image(
                Rect::new(drag_to.x - w / 2.0, drag_to.y - h / 2.0, w, h),
                pm,
                0.0,
                false,
            );
        }
        canvas.pointer(
            lerp_pt(
                drag_to,
                Point::new(
                    drag_to.x + 120.0 * WHEEL_ZOOM,
                    drag_to.y - 70.0 * WHEEL_ZOOM,
                ),
                ease(t / 1.0),
            ),
            true,
        );
        canvas.caption("Ctrl opens Tools: compress, split, sign, redact…", 1.0);
        video.push(&canvas.end());
    }

    // ── 4. The Board: a whole Downloads folder at once ──────────────────────────────
    let noop: swirvo_ui::Waker = Arc::new(|| {});
    let mut ui = Ui::new(Theme::dark(), fonts());
    let files: Vec<PathBuf> = pick.iter().map(|f| in_work(f)).collect();
    let mut board = Board::new(tk.clone(), noop.clone(), files.clone());
    board.settle(&ui);
    board.set_target("pdf", "docx");
    board.set_target("png", "jpg");
    board.set_target("md", "html");
    board.set_target("csv", "xlsx");
    // Let the thumbnails arrive before recording.
    for _ in 0..90 {
        render_app(&mut board, &mut ui, |_| {});
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let button = app_pt(175.0, APP.h / APP_SCALE - 70.0);
    for i in 0..secs(8.0) {
        let t = i as f32 / FPS;
        let press = i == secs(1.6);
        let pm = render_app(&mut board, &mut ui, |ui| {
            if press {
                ui.input.keys.push((Key::Enter, Mods::default()));
            }
        });
        if t > 1.6 {
            // Conversions run for real in the background; give them real time.
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        canvas.begin();
        canvas.window(&pm, ease(t / 0.4));
        canvas.pointer(
            lerp_pt(Point::new(900.0, 600.0), button, ease(t / 1.4)),
            (1.5..1.75).contains(&t),
        );
        canvas.caption(
            if t < 1.6 {
                "Or tidy a whole folder: every file, its own format"
            } else {
                "All your cores at work. Nothing leaves your PC."
            },
            1.0,
        );
        video.push(&canvas.end());
    }

    // ── 5. Search for a tool ─────────────────────────────────────────────────────────
    let handbook = files
        .iter()
        .position(|f| f.ends_with("Team handbook.pdf"))
        .unwrap_or(0);
    board.select(&[handbook]);
    let query = "split";
    for i in 0..secs(3.0) {
        let t = i as f32 / FPS;
        let typed = ((t / 0.12) as usize).min(query.len());
        board.set_search(&query[..typed], true);
        let pm = render_app(&mut board, &mut ui, |_| {});
        canvas.begin();
        canvas.window(&pm, 1.0);
        canvas.pointer(app_pt(200.0, 130.0), false);
        canvas.caption("Find any tool by typing", 1.0);
        video.push(&canvas.end());
    }

    // ── 6. Split into your own groups (page picker) ─────────────────────────────────
    let mut picker = PagePicker::new(
        tk.clone(),
        noop.clone(),
        in_work("Team handbook.pdf"),
        PageOp::Split,
    );
    picker.settle();
    picker.start_groups("");
    let mut pui = Ui::new(Theme::dark(), fonts());
    for _ in 0..120 {
        render_app(&mut picker, &mut pui, |_| {});
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let spec = "1-3, 5; 6-9; 10-13";
    for i in 0..secs(6.0) {
        let t = i as f32 / FPS;
        let typed = ((t / 0.14) as usize).min(spec.len());
        let s = &spec[..typed];
        let _ = picker.set_groups(if s.is_empty() { " " } else { s });
        let pm = render_app(&mut picker, &mut pui, |_| {});
        canvas.begin();
        canvas.window(&pm, 1.0);
        canvas.caption(
            "Split a PDF into your own groups: 1-3, 5 · 6-9 · 10-13",
            1.0,
        );
        video.push(&canvas.end());
    }

    // ── 7. Live previews: watermark, crop, redact ───────────────────────────────────
    board.set_search("", true);
    board.select(&[handbook]);
    board.open_tool("pdf-watermark");
    board.form_set("text", "DRAFT");
    board.form_set("color", "blue");
    let _ = board.preview_wait();
    for i in 0..secs(5.0) {
        let t = i as f32 / FPS;
        let (x, y) = (
            lerp(50.0, 30.0, ease((t - 1.5) / 1.0)),
            lerp(50.0, 25.0, ease((t - 1.5) / 1.0)),
        );
        if (1.5..2.6).contains(&t) {
            board.form_set("place", &format!("{}, {}", x.round(), y.round()));
            board.form_set("angle", "0");
            board.form_set("size", "40");
        }
        if (t - 2.7).abs() < 0.02 {
            let _ = board.preview_wait();
        }
        let pm = render_app(&mut board, &mut ui, |_| {});
        canvas.begin();
        canvas.window(&pm, 1.0);
        canvas.caption(
            "See every change before you save it, and drag it where you want",
            1.0,
        );
        video.push(&canvas.end());
    }
    board.open_tool("pdf-crop");
    for i in 0..secs(4.0) {
        let t = i as f32 / FPS;
        let m = lerp(0.0, 22.0, ease((t - 0.6) / 1.6));
        for k in ["left", "right", "top", "bottom"] {
            board.form_set(k, &format!("{m:.0}"));
        }
        let pm = render_app(&mut board, &mut ui, |_| {});
        canvas.begin();
        canvas.window(&pm, 1.0);
        canvas.caption("Crop by dragging: every page, or just this one", 1.0);
        video.push(&canvas.end());
    }
    let resume = files
        .iter()
        .position(|f| f.ends_with("Resume - Jane Doe.pdf"))
        .unwrap_or(0);
    board.select(&[resume]);
    board.open_tool("pdf-redact");
    board.form_set("find", "jane.doe@example.com, +44 20 7946 0000, London");
    let _ = board.preview_wait();
    for _ in 0..secs(4.0) {
        let pm = render_app(&mut board, &mut ui, |_| {});
        canvas.begin();
        canvas.window(&pm, 1.0);
        canvas.caption("Redaction removes the text itself, not just covers it", 1.0);
        video.push(&canvas.end());
    }

    // ── 8. Outro ─────────────────────────────────────────────────────────────────────
    for i in 0..secs(5.0) {
        let t = i as f32 / FPS;
        let a = ease(t / 0.8);
        canvas.begin();
        canvas.title(
            "Swirvo",
            "Coming soon for Windows · free to start",
            a,
            520.0,
        );
        let r = Rect::new(0.0, 700.0, W as f32, 50.0);
        canvas.ui.text_in(
            r,
            "Offline · never overwrites your files · 100+ formats",
            26.0,
            Weight::Regular,
            Rgba::hexa(0xFF9A57, (255.0 * a) as u8),
            Align::Center,
        );
        video.push(&canvas.end());
    }

    let frames = video.frames;
    drop(video.child.stdin.take());
    let ok = video.child.wait().map(|s| s.success()).unwrap_or(false);
    let _ = size::BODY;
    eprintln!(
        "{frames} frames ({:.1} s) → {out}: {}",
        frames as f32 / FPS,
        if ok { "ok" } else { "ffmpeg failed" }
    );
    let _ = Path::new(out);
}
