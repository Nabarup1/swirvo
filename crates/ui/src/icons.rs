//! Stroke icons drawn as paths on a 20-unit grid (Fluent-like: 1.5 px strokes, round caps).
//! Our own drawings, so they look the same on every OS and in snapshots.

use tiny_skia::PathBuilder;

use crate::ctx::Ui;
use crate::geom::Rect;
use crate::theme::{Kind, Rgba};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Add,
    Close,
    Minimize,
    Maximize,
    Restore,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    Folder,
    File,
    Image,
    Document,
    Music,
    Video,
    Archive,
    Data,
    Font,
    Trash,
    Arrow,
    Search,
    Warning,
    Info,
    Pages,
    Rotate,
    Export,
    Sparkle,
    Stop,
    SelectAll,
    Broom,
    Duplicate,
    Crop,
    Pen,
    Highlight,
    Note,
    Square,
    TextBox,
    Undo,
    BlankPage,
    Redact,
    Settings,
}

/// Maps 20-unit icon space into a device-space square.
struct Grid {
    x: f32,
    y: f32,
    k: f32,
}

impl Grid {
    fn p(&self, u: f32, v: f32) -> (f32, f32) {
        (self.x + u * self.k, self.y + v * self.k)
    }
    fn m(&self, pb: &mut PathBuilder, u: f32, v: f32) {
        let (x, y) = self.p(u, v);
        pb.move_to(x, y);
    }
    fn l(&self, pb: &mut PathBuilder, u: f32, v: f32) {
        let (x, y) = self.p(u, v);
        pb.line_to(x, y);
    }
    fn q(&self, pb: &mut PathBuilder, cu: f32, cv: f32, u: f32, v: f32) {
        let (cx, cy) = self.p(cu, cv);
        let (x, y) = self.p(u, v);
        pb.quad_to(cx, cy, x, y);
    }
    /// Rounded rectangle outline in icon units.
    fn rrect(&self, pb: &mut PathBuilder, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) {
        self.m(pb, x0 + r, y0);
        self.l(pb, x1 - r, y0);
        self.q(pb, x1, y0, x1, y0 + r);
        self.l(pb, x1, y1 - r);
        self.q(pb, x1, y1, x1 - r, y1);
        self.l(pb, x0 + r, y1);
        self.q(pb, x0, y1, x0, y1 - r);
        self.l(pb, x0, y0 + r);
        self.q(pb, x0, y0, x0 + r, y0);
        pb.close();
    }
    fn circle(&self, pb: &mut PathBuilder, cu: f32, cv: f32, r: f32) {
        let (cx, cy) = self.p(cu, cv);
        if let Some(c) = PathBuilder::from_circle(cx, cy, r * self.k) {
            pb.push_path(&c);
        }
    }
}

fn arc(g: &Grid, pb: &mut PathBuilder, cu: f32, cv: f32, r: f32, a0: f32, a1: f32) {
    let n = 16;
    for i in 0..=n {
        let a = a0 + (a1 - a0) * i as f32 / n as f32;
        let (u, v) = (cu + r * a.cos(), cv + r * a.sin());
        if i == 0 {
            g.m(pb, u, v);
        } else {
            g.l(pb, u, v);
        }
    }
}

fn page(g: &Grid, pb: &mut PathBuilder) {
    g.m(pb, 5.0, 2.5);
    g.l(pb, 11.5, 2.5);
    g.l(pb, 15.5, 6.5);
    g.l(pb, 15.5, 16.0);
    g.q(pb, 15.5, 17.5, 14.0, 17.5);
    g.l(pb, 6.0, 17.5);
    g.q(pb, 4.5, 17.5, 4.5, 16.0);
    g.l(pb, 4.5, 4.0);
    g.q(pb, 4.5, 2.5, 6.0, 2.5);
    g.m(pb, 11.5, 2.5);
    g.l(pb, 11.5, 6.5);
    g.l(pb, 15.5, 6.5);
}

impl Ui {
    /// Draw `icon` centered in `r` (square, `r.h` tall) in `c`.
    pub fn icon(&mut self, r: Rect, icon: Icon, c: Rgba) {
        let side = r.w.min(r.h);
        let box_ = r.centered(side, side);
        let weight = (side / 20.0 * 1.5).clamp(1.1, 2.4);
        let b = box_;
        let draw = move |pb: &mut PathBuilder, s: f32| {
            let g = Grid {
                x: b.x * s,
                y: b.y * s,
                k: b.w * s / 20.0,
            };
            match icon {
                Icon::Add => {
                    g.m(pb, 10.0, 4.0);
                    g.l(pb, 10.0, 16.0);
                    g.m(pb, 4.0, 10.0);
                    g.l(pb, 16.0, 10.0);
                }
                Icon::Close => {
                    g.m(pb, 5.0, 5.0);
                    g.l(pb, 15.0, 15.0);
                    g.m(pb, 15.0, 5.0);
                    g.l(pb, 5.0, 15.0);
                }
                Icon::Minimize => {
                    g.m(pb, 5.0, 10.0);
                    g.l(pb, 15.0, 10.0);
                }
                Icon::Maximize => g.rrect(pb, 5.0, 5.0, 15.0, 15.0, 1.5),
                Icon::Restore => {
                    g.rrect(pb, 5.0, 7.0, 13.0, 15.0, 1.2);
                    g.m(pb, 7.0, 5.0);
                    g.l(pb, 13.5, 5.0);
                    g.q(pb, 15.0, 5.0, 15.0, 6.5);
                    g.l(pb, 15.0, 13.0);
                }
                Icon::Check => {
                    g.m(pb, 4.5, 10.5);
                    g.l(pb, 8.5, 14.5);
                    g.l(pb, 15.5, 6.0);
                }
                Icon::ChevronDown => {
                    g.m(pb, 5.5, 8.0);
                    g.l(pb, 10.0, 12.5);
                    g.l(pb, 14.5, 8.0);
                }
                Icon::ChevronRight => {
                    g.m(pb, 8.0, 5.5);
                    g.l(pb, 12.5, 10.0);
                    g.l(pb, 8.0, 14.5);
                }
                Icon::ChevronLeft => {
                    g.m(pb, 12.0, 5.5);
                    g.l(pb, 7.5, 10.0);
                    g.l(pb, 12.0, 14.5);
                }
                Icon::Folder => {
                    g.m(pb, 2.5, 5.5);
                    g.q(pb, 2.5, 4.0, 4.0, 4.0);
                    g.l(pb, 7.5, 4.0);
                    g.l(pb, 9.5, 6.0);
                    g.l(pb, 16.0, 6.0);
                    g.q(pb, 17.5, 6.0, 17.5, 7.5);
                    g.l(pb, 17.5, 14.5);
                    g.q(pb, 17.5, 16.0, 16.0, 16.0);
                    g.l(pb, 4.0, 16.0);
                    g.q(pb, 2.5, 16.0, 2.5, 14.5);
                    pb.close();
                    g.m(pb, 2.5, 8.0);
                    g.l(pb, 17.5, 8.0);
                }
                Icon::File => page(&g, pb),
                Icon::Document => {
                    page(&g, pb);
                    for v in [10.0, 12.5, 15.0] {
                        g.m(pb, 7.0, v);
                        g.l(pb, 13.0, v);
                    }
                }
                Icon::Image => {
                    g.rrect(pb, 2.5, 4.0, 17.5, 16.0, 2.0);
                    g.m(pb, 3.0, 14.5);
                    g.l(pb, 7.5, 10.0);
                    g.l(pb, 10.5, 13.0);
                    g.l(pb, 13.0, 10.5);
                    g.l(pb, 17.0, 14.5);
                    g.circle(pb, 13.0, 7.5, 1.2);
                }
                Icon::Music => {
                    g.m(pb, 8.0, 14.5);
                    g.l(pb, 8.0, 4.5);
                    g.l(pb, 16.0, 3.0);
                    g.l(pb, 16.0, 13.0);
                    g.circle(pb, 6.0, 14.5, 2.0);
                    g.circle(pb, 14.0, 13.0, 2.0);
                }
                Icon::Video => {
                    g.rrect(pb, 2.5, 5.0, 13.0, 15.0, 2.0);
                    g.m(pb, 13.0, 8.5);
                    g.l(pb, 17.5, 6.0);
                    g.l(pb, 17.5, 14.0);
                    g.l(pb, 13.0, 11.5);
                }
                Icon::Archive => {
                    g.rrect(pb, 3.0, 3.0, 17.0, 17.0, 2.0);
                    for v in [5.5, 8.0, 10.5] {
                        g.m(pb, 9.0, v);
                        g.l(pb, 11.0, v);
                    }
                    g.rrect(pb, 8.5, 12.0, 11.5, 15.0, 0.8);
                }
                Icon::Data => {
                    g.rrect(pb, 3.0, 4.0, 17.0, 16.0, 2.0);
                    g.m(pb, 3.0, 8.0);
                    g.l(pb, 17.0, 8.0);
                    g.m(pb, 3.0, 12.0);
                    g.l(pb, 17.0, 12.0);
                    g.m(pb, 8.0, 4.0);
                    g.l(pb, 8.0, 16.0);
                }
                Icon::Font => {
                    g.m(pb, 4.0, 16.0);
                    g.l(pb, 10.0, 3.5);
                    g.l(pb, 16.0, 16.0);
                    g.m(pb, 6.3, 11.5);
                    g.l(pb, 13.7, 11.5);
                }
                Icon::Trash => {
                    g.m(pb, 3.5, 5.5);
                    g.l(pb, 16.5, 5.5);
                    g.m(pb, 8.0, 5.5);
                    g.l(pb, 8.0, 3.5);
                    g.l(pb, 12.0, 3.5);
                    g.l(pb, 12.0, 5.5);
                    g.m(pb, 5.0, 5.5);
                    g.l(pb, 6.0, 16.0);
                    g.q(pb, 6.2, 17.0, 7.2, 17.0);
                    g.l(pb, 12.8, 17.0);
                    g.q(pb, 13.8, 17.0, 14.0, 16.0);
                    g.l(pb, 15.0, 5.5);
                }
                Icon::Arrow => {
                    g.m(pb, 3.5, 10.0);
                    g.l(pb, 16.0, 10.0);
                    g.m(pb, 11.5, 5.5);
                    g.l(pb, 16.0, 10.0);
                    g.l(pb, 11.5, 14.5);
                }
                Icon::Settings => {
                    // A gear: eight teeth around a ring, and the hub.
                    for k in 0..8 {
                        let a = k as f32 * std::f32::consts::FRAC_PI_4;
                        let (s, c) = a.sin_cos();
                        g.m(pb, 10.0 + 5.6 * c, 10.0 + 5.6 * s);
                        g.l(pb, 10.0 + 7.6 * c, 10.0 + 7.6 * s);
                    }
                    g.circle(pb, 10.0, 10.0, 5.2);
                    g.circle(pb, 10.0, 10.0, 2.2);
                }
                Icon::Search => {
                    g.circle(pb, 8.5, 8.5, 5.0);
                    g.m(pb, 12.2, 12.2);
                    g.l(pb, 16.5, 16.5);
                }
                Icon::Warning => {
                    g.m(pb, 10.0, 3.0);
                    g.l(pb, 17.5, 16.5);
                    g.l(pb, 2.5, 16.5);
                    pb.close();
                    g.m(pb, 10.0, 8.0);
                    g.l(pb, 10.0, 11.5);
                    g.m(pb, 10.0, 14.0);
                    g.l(pb, 10.0, 14.1);
                }
                Icon::Info => {
                    g.circle(pb, 10.0, 10.0, 7.5);
                    g.m(pb, 10.0, 9.0);
                    g.l(pb, 10.0, 14.0);
                    g.m(pb, 10.0, 6.2);
                    g.l(pb, 10.0, 6.3);
                }
                Icon::Pages => {
                    g.rrect(pb, 6.5, 2.5, 16.5, 14.5, 1.5);
                    g.m(pb, 4.0, 5.5);
                    g.l(pb, 3.5, 5.5);
                    g.l(pb, 3.5, 16.0);
                    g.q(pb, 3.5, 17.5, 5.0, 17.5);
                    g.l(pb, 13.5, 17.5);
                }
                Icon::Rotate => {
                    arc(&g, pb, 10.0, 10.5, 6.0, -2.6, 2.2);
                    g.m(pb, 4.2, 3.6);
                    g.l(pb, 4.6, 7.4);
                    g.l(pb, 8.3, 6.6);
                }
                Icon::Export => {
                    g.m(pb, 10.0, 12.5);
                    g.l(pb, 10.0, 2.5);
                    g.m(pb, 6.0, 6.5);
                    g.l(pb, 10.0, 2.5);
                    g.l(pb, 14.0, 6.5);
                    g.m(pb, 3.5, 11.0);
                    g.l(pb, 3.5, 16.0);
                    g.q(pb, 3.5, 17.5, 5.0, 17.5);
                    g.l(pb, 15.0, 17.5);
                    g.q(pb, 16.5, 17.5, 16.5, 16.0);
                    g.l(pb, 16.5, 11.0);
                }
                Icon::Sparkle => {
                    g.m(pb, 10.0, 2.5);
                    g.q(pb, 10.8, 9.2, 17.5, 10.0);
                    g.q(pb, 10.8, 10.8, 10.0, 17.5);
                    g.q(pb, 9.2, 10.8, 2.5, 10.0);
                    g.q(pb, 9.2, 9.2, 10.0, 2.5);
                    pb.close();
                }
                Icon::Stop => g.rrect(pb, 5.0, 5.0, 15.0, 15.0, 2.0),
                Icon::SelectAll => {
                    g.rrect(pb, 3.0, 3.0, 17.0, 17.0, 3.0);
                    g.m(pb, 6.5, 10.0);
                    g.l(pb, 9.0, 12.5);
                    g.l(pb, 13.5, 7.5);
                }
                Icon::Broom => {
                    g.m(pb, 15.5, 2.5);
                    g.l(pb, 10.0, 9.5);
                    g.m(pb, 6.5, 8.5);
                    g.l(pb, 12.0, 12.5);
                    g.l(pb, 9.0, 17.5);
                    g.l(pb, 2.5, 13.0);
                    pb.close();
                }
                Icon::Duplicate => {
                    g.rrect(pb, 7.0, 7.0, 17.0, 17.0, 2.0);
                    g.m(pb, 13.0, 4.5);
                    g.l(pb, 13.0, 4.0);
                    g.q(pb, 13.0, 3.0, 12.0, 3.0);
                    g.l(pb, 4.0, 3.0);
                    g.q(pb, 3.0, 3.0, 3.0, 4.0);
                    g.l(pb, 3.0, 12.0);
                    g.q(pb, 3.0, 13.0, 4.0, 13.0);
                    g.l(pb, 4.5, 13.0);
                }
                Icon::Crop => {
                    g.m(pb, 5.5, 2.5);
                    g.l(pb, 5.5, 14.5);
                    g.l(pb, 17.5, 14.5);
                    g.m(pb, 2.5, 5.5);
                    g.l(pb, 14.5, 5.5);
                    g.l(pb, 14.5, 17.5);
                }
                Icon::Pen => {
                    g.m(pb, 3.5, 16.5);
                    g.l(pb, 4.5, 12.5);
                    g.l(pb, 13.5, 3.5);
                    g.l(pb, 16.5, 6.5);
                    g.l(pb, 7.5, 15.5);
                    pb.close();
                    g.m(pb, 11.5, 5.5);
                    g.l(pb, 14.5, 8.5);
                }
                Icon::Highlight => {
                    g.m(pb, 6.0, 12.0);
                    g.l(pb, 12.5, 3.5);
                    g.l(pb, 16.5, 7.5);
                    g.l(pb, 8.0, 14.0);
                    pb.close();
                    g.m(pb, 6.0, 12.0);
                    g.l(pb, 4.0, 15.0);
                    g.l(pb, 6.5, 15.5);
                    g.l(pb, 8.0, 14.0);
                    g.m(pb, 2.5, 17.5);
                    g.l(pb, 17.5, 17.5);
                }
                Icon::Note => {
                    g.m(pb, 3.5, 4.5);
                    g.q(pb, 3.5, 3.0, 5.0, 3.0);
                    g.l(pb, 15.0, 3.0);
                    g.q(pb, 16.5, 3.0, 16.5, 4.5);
                    g.l(pb, 16.5, 12.0);
                    g.q(pb, 16.5, 13.5, 15.0, 13.5);
                    g.l(pb, 9.0, 13.5);
                    g.l(pb, 5.5, 17.0);
                    g.l(pb, 5.5, 13.5);
                    g.l(pb, 5.0, 13.5);
                    g.q(pb, 3.5, 13.5, 3.5, 12.0);
                    pb.close();
                    g.m(pb, 6.5, 7.0);
                    g.l(pb, 13.5, 7.0);
                    g.m(pb, 6.5, 10.0);
                    g.l(pb, 11.0, 10.0);
                }
                Icon::Square => g.rrect(pb, 3.5, 4.5, 16.5, 15.5, 1.5),
                Icon::TextBox => {
                    g.m(pb, 4.5, 5.5);
                    g.l(pb, 4.5, 3.5);
                    g.l(pb, 15.5, 3.5);
                    g.l(pb, 15.5, 5.5);
                    g.m(pb, 10.0, 3.5);
                    g.l(pb, 10.0, 16.5);
                    g.m(pb, 7.5, 16.5);
                    g.l(pb, 12.5, 16.5);
                }
                Icon::Undo => {
                    g.m(pb, 7.0, 4.0);
                    g.l(pb, 3.5, 7.5);
                    g.l(pb, 7.0, 11.0);
                    g.m(pb, 3.5, 7.5);
                    g.l(pb, 12.0, 7.5);
                    g.q(pb, 16.5, 7.5, 16.5, 12.0);
                    g.q(pb, 16.5, 16.5, 12.0, 16.5);
                    g.l(pb, 8.0, 16.5);
                }
                Icon::BlankPage => {
                    page(&g, pb);
                    g.m(pb, 10.0, 9.0);
                    g.l(pb, 10.0, 15.0);
                    g.m(pb, 7.0, 12.0);
                    g.l(pb, 13.0, 12.0);
                }
                Icon::Redact => {
                    g.rrect(pb, 2.5, 6.5, 17.5, 13.5, 1.5);
                    g.m(pb, 5.0, 9.0);
                    g.l(pb, 15.0, 9.0);
                    g.m(pb, 5.0, 11.0);
                    g.l(pb, 15.0, 11.0);
                }
            }
        };
        if icon == Icon::Sparkle {
            self.fill_logical(draw, c);
        } else {
            self.stroke_logical(draw, c, weight);
        }
    }

    /// A file-type badge: a tinted rounded square with the category icon.
    pub fn kind_badge(&mut self, r: Rect, kind: Kind) {
        let tint = self.theme.kind(kind);
        self.fill_rrect(r, r.h * 0.28, tint.fade(0.16));
        let icon = match kind {
            Kind::Image => Icon::Image,
            Kind::Document => Icon::Document,
            Kind::Audio => Icon::Music,
            Kind::Video => Icon::Video,
            Kind::Archive => Icon::Archive,
            Kind::Data => Icon::Data,
            Kind::Font => Icon::Font,
            Kind::Other => Icon::File,
        };
        self.icon(r.inset(r.h * 0.2), icon, tint);
    }
}
