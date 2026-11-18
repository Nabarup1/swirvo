//! The immediate-mode context: per-frame state, widget identity and interaction, small
//! retained memory (animations, scroll offsets, text-field carets), host hit zones.

use std::collections::HashMap;

use tiny_skia::{Mask, Pixmap};

use crate::geom::{Point, Rect};
use crate::input::{Input, Zone};
use crate::text::Fonts;
use crate::theme::Theme;

/// Stable widget identity: a 64-bit hash of a name plus optional indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Id(pub u64);

impl Id {
    pub const NONE: Id = Id(0);
    pub fn new(name: &str) -> Id {
        // FNV-1a: tiny, deterministic, good enough for a few thousand ids.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in name.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        Id(h | 1)
    }
    pub fn with(self, i: impl std::hash::Hash) -> Id {
        use std::hash::{Hash, Hasher};
        let mut s = std::collections::hash_map::DefaultHasher::new();
        self.0.hash(&mut s);
        i.hash(&mut s);
        Id(s.finish() | 1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Arrow,
    Hand,
    Text,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Response {
    pub hovered: bool,
    pub pressed: bool,
    pub down: bool,
    pub clicked: bool,
    pub double_clicked: bool,
    pub rect: Rect,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EditState {
    pub caret: usize,
    pub anchor: usize,
    pub scroll: f32,
}

/// An open dropdown list, drawn on top of everything at the end of the frame.
#[derive(Debug, Clone)]
pub(crate) struct Popup {
    pub owner: Id,
    pub anchor: Rect,
    pub items: Vec<String>,
    pub selected: usize,
    pub scroll: f32,
    pub rect: Rect,
}

/// (width, height, radius, blur) in device px.
pub(crate) type ShadowKey = (u32, u32, u32, u32);

pub struct Ui {
    pub theme: Theme,
    pub fonts: Fonts,
    pub input: Input,
    /// Device pixels per logical pixel.
    pub scale: f32,
    /// Logical size of the canvas.
    pub size: (f32, f32),
    /// True when the OS paints glass behind the window (Acrylic/Mica).
    pub backdrop: bool,
    /// Where the OS draws its own caption buttons (logical px), when it does. Apps keep
    /// that area free; [`crate::widgets::window_controls`] then draws nothing.
    pub caption_buttons: Option<Rect>,
    pub(crate) pm: Pixmap,
    pub(crate) clip: Vec<Rect>,
    pub(crate) mask: Option<Mask>,
    pub(crate) mask_rect: Option<Rect>,
    pub(crate) shadows: HashMap<ShadowKey, (u32, u32, Vec<u8>)>,
    hot: Id,
    hot_rect: Rect,
    hot_next: Id,
    hot_next_rect: Rect,
    pub(crate) active: Id,
    pub(crate) focus: Id,
    focus_seen: bool,
    hover_since: f64,
    anims: HashMap<Id, (f32, u64)>,
    pub(crate) edits: HashMap<Id, EditState>,
    pub(crate) scrolls: HashMap<Id, (f32, f32)>,
    pub(crate) popup: Option<Popup>,
    /// A choice made in a popup, delivered to its dropdown on the next frame.
    pub(crate) popup_choice: Option<(Id, usize)>,
    pub(crate) tooltip: Option<(String, Point)>,
    frame: u64,
    repaint_at: Option<f64>,
    pub cursor: Cursor,
    zones: Vec<(Rect, Zone)>,
    interactive: Vec<Rect>,
}

impl Ui {
    pub fn new(theme: Theme, fonts: Fonts) -> Ui {
        Ui {
            theme,
            fonts,
            input: Input::default(),
            scale: 1.0,
            size: (1.0, 1.0),
            backdrop: false,
            caption_buttons: None,
            pm: Pixmap::new(1, 1).unwrap_or_else(|| unreachable!("1×1 pixmap")),
            clip: Vec::new(),
            mask: None,
            mask_rect: None,
            shadows: HashMap::new(),
            hot: Id::NONE,
            hot_rect: Rect::default(),
            hot_next: Id::NONE,
            hot_next_rect: Rect::default(),
            active: Id::NONE,
            focus: Id::NONE,
            focus_seen: false,
            hover_since: 0.0,
            anims: HashMap::new(),
            edits: HashMap::new(),
            scrolls: HashMap::new(),
            popup: None,
            popup_choice: None,
            tooltip: None,
            frame: 0,
            repaint_at: None,
            cursor: Cursor::Arrow,
            zones: Vec::new(),
            interactive: Vec::new(),
        }
    }

    /// Start a frame of `w` × `h` device pixels. Reuses the canvas unless the size changed.
    pub fn begin_frame(&mut self, w: u32, h: u32, scale: f32) {
        let (w, h) = (w.max(1), h.max(1));
        if (self.pm.width(), self.pm.height()) != (w, h)
            && let Some(p) = Pixmap::new(w, h)
        {
            self.pm = p;
            self.mask = None;
            self.mask_rect = None;
        }
        self.scale = scale.max(0.5);
        self.size = (w as f32 / self.scale, h as f32 / self.scale);
        self.clip.clear();
        self.zones.clear();
        self.interactive.clear();
        self.cursor = Cursor::Arrow;
        self.repaint_at = None;
        self.tooltip = None;
        self.hot_next = Id::NONE;
        self.focus_seen = false;
        self.frame += 1;
        self.paint_background();
    }

    /// Finish the frame: popups and tooltips on top, hover bookkeeping, and when to repaint.
    /// Returns seconds until the next frame is needed (None = only on input).
    pub fn end_frame(&mut self) -> Option<f32> {
        self.draw_popup();
        self.draw_tooltip();
        if self.hot_next != self.hot {
            self.hot = self.hot_next;
            self.hot_rect = self.hot_next_rect;
            self.hover_since = self.input.time;
            // Redraw once so the new hover state shows without waiting for more input.
            self.request_repaint(0.0);
        }
        if !self.focus_seen && self.popup.is_none() {
            self.focus = Id::NONE;
        }
        if self.input.released {
            self.active = Id::NONE;
        }
        if self.frame.is_multiple_of(240) {
            let f = self.frame;
            self.anims.retain(|_, (_, seen)| f - *seen < 240);
        }
        self.input.end_frame();
        self.repaint_at
            .map(|t| (t - self.input.time).max(0.0) as f32)
    }

    pub fn pixmap(&self) -> &Pixmap {
        &self.pm
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.size.0, self.size.1)
    }

    /// Ask for another frame within `secs` (animations, caret blink, spinners).
    pub fn request_repaint(&mut self, secs: f32) {
        let t = self.input.time + secs.max(0.0) as f64;
        self.repaint_at = Some(self.repaint_at.map_or(t, |r| r.min(t)));
    }

    /// Hit zones for the host's non-client hit test, in logical px. Interactive widgets
    /// inside the caption are excluded so they stay clickable.
    pub fn zone_at(&self, p: Point) -> Option<Zone> {
        let z = self
            .zones
            .iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|z| z.1)?;
        if z == Zone::Caption && self.interactive.iter().any(|r| r.contains(p)) {
            return None;
        }
        Some(z)
    }

    pub fn add_zone(&mut self, r: Rect, z: Zone) {
        self.zones.push((r, z));
    }

    pub fn pointer(&self) -> Option<Point> {
        self.input.pointer
    }

    /// True when the pointer is over `r`, inside the current clip and not under a popup.
    pub fn pointer_in(&self, r: Rect) -> bool {
        let Some(p) = self.input.pointer else {
            return false;
        };
        if !r.contains(p) {
            return false;
        }
        if let Some(c) = self.clip.last()
            && !c.scale(1.0 / self.scale).contains(p)
        {
            return false;
        }
        // While a dropdown is open, only its own button stays live underneath it.
        match &self.popup {
            Some(pp) => pp.anchor == r,
            None => true,
        }
    }

    /// Core interaction: hover (topmost widget wins, decided from the previous frame),
    /// press, click on release inside.
    pub fn interact(&mut self, id: Id, r: Rect) -> Response {
        let inside = self.pointer_in(r);
        // Every interactive rect is recorded, hovered or not: the host's hit test runs
        // before the pointer reaches a widget in the title bar, and must not call it caption.
        self.interactive.push(r);
        if inside {
            self.hot_next = id;
            self.hot_next_rect = r;
        }
        let hot_valid = self
            .input
            .pointer
            .is_some_and(|p| self.hot_rect.contains(p));
        let hovered = inside && (self.hot == id || !hot_valid || self.active == id);
        let mut resp = Response {
            hovered,
            rect: r,
            ..Response::default()
        };
        if hovered && self.input.pressed && self.active == Id::NONE {
            self.active = id;
            resp.pressed = true;
        }
        resp.down = self.active == id && self.input.down;
        if self.active == id && self.input.released {
            resp.clicked = inside;
        }
        if hovered && self.input.double_click {
            resp.double_clicked = true;
        }
        if hovered {
            self.cursor = Cursor::Hand;
        }
        resp
    }

    /// Seconds the current hot widget has been hovered.
    pub fn hover_time(&self, id: Id) -> f32 {
        if self.hot == id {
            (self.input.time - self.hover_since) as f32
        } else {
            0.0
        }
    }

    pub fn set_focus(&mut self, id: Id) {
        self.focus = id;
        self.focus_seen = true;
    }
    /// True when some widget (a text field) has keyboard focus.
    pub fn has_any_focus(&self) -> bool {
        self.focus != Id::NONE
    }

    pub fn has_focus(&mut self, id: Id) -> bool {
        if self.focus == id {
            self.focus_seen = true;
            true
        } else {
            false
        }
    }

    /// Smoothly approach `target` (critically damped feel); `speed` ~ 1/seconds.
    pub fn anim(&mut self, id: Id, target: f32, speed: f32) -> f32 {
        let dt = self.input.dt.clamp(0.0, 0.1);
        let frame = self.frame;
        let e = self.anims.entry(id).or_insert((target, frame));
        let k = 1.0 - (-dt * speed).exp();
        e.0 += (target - e.0) * k;
        e.1 = frame;
        if (target - e.0).abs() < 0.002 {
            e.0 = target;
        }
        let v = e.0;
        if v != target {
            self.request_repaint(1.0 / 60.0);
        }
        v
    }

    pub fn tooltip(&mut self, text: impl Into<String>) {
        if let Some(p) = self.input.pointer {
            self.tooltip = Some((text.into(), p));
        }
    }

    /// The pixel rectangle `r` covers on the canvas.
    pub fn device(&self, r: Rect) -> Rect {
        r.scale(self.scale)
    }
}
