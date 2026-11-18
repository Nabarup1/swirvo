//! Input for one frame. The host accumulates OS events into an [`Input`]; the UI reads
//! it while building the frame; [`Input::end_frame`] clears the per-frame parts.

use std::path::PathBuf;

use crate::geom::Point;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    A,
    C,
    D,
    F,
    V,
    X,
    Z,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Non-client parts of the window that the UI draws but the host hit-tests (title bar
/// drag area and caption buttons), so Windows keeps Snap Layouts and native dragging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Caption,
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Default)]
pub struct Input {
    /// Pointer in logical px; None when it has left the window.
    pub pointer: Option<Point>,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub double_click: bool,
    pub secondary_pressed: bool,
    /// Scroll in logical px; positive moves content up (wheel down).
    pub wheel: f32,
    pub keys: Vec<(Key, Mods)>,
    pub text: String,
    pub mods: Mods,
    /// Files dropped on the window this frame.
    pub dropped: Vec<PathBuf>,
    /// Files are being dragged over the window.
    pub drag_hover: bool,
    /// Caption button under the pointer / pressed (reported by the host).
    pub zone_hover: Option<Zone>,
    pub zone_pressed: Option<Zone>,
    pub maximized: bool,
    pub active: bool,
    /// Seconds since the previous frame, and since start.
    pub dt: f32,
    pub time: f64,
}

impl Input {
    pub fn key(&self, k: Key) -> bool {
        self.keys.iter().any(|(key, _)| *key == k)
    }
    pub fn ctrl_key(&self, k: Key) -> bool {
        self.keys.iter().any(|(key, m)| *key == k && m.ctrl)
    }
    pub fn end_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.double_click = false;
        self.secondary_pressed = false;
        self.wheel = 0.0;
        self.keys.clear();
        self.text.clear();
        self.dropped.clear();
    }
}
