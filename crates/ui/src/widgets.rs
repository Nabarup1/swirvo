//! Widgets. Each is a function that reads input, draws, and returns what happened.

use crate::ctx::{Cursor, EditState, Id, Popup, Response, Ui};
use crate::geom::{Point, Rect};
use crate::icons::Icon;
use crate::input::{Key, Zone};
use crate::paint::Align;
use crate::text::Weight;
use crate::theme::{Rgba, size};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

fn hover_anim(ui: &mut Ui, id: Id, on: bool) -> f32 {
    ui.anim(id.with("hover"), if on { 1.0 } else { 0.0 }, 22.0)
}

/// A push button with an optional leading icon.
pub fn button(
    ui: &mut Ui,
    id: Id,
    r: Rect,
    label: &str,
    icon: Option<Icon>,
    kind: ButtonKind,
    enabled: bool,
) -> Response {
    let resp = if enabled {
        ui.interact(id, r)
    } else {
        Response {
            rect: r,
            ..Response::default()
        }
    };
    let t = ui.theme;
    let h = hover_anim(ui, id, resp.hovered);
    let (mut fill, mut ink, stroke) = match kind {
        ButtonKind::Primary => (t.accent.lerp(t.accent_hover, h), t.on_accent, None),
        ButtonKind::Secondary => (
            t.control.lerp(t.control_hover, h),
            t.text,
            Some(t.control_stroke),
        ),
        ButtonKind::Ghost => (t.control_hover.fade(h), t.text, None),
        ButtonKind::Danger => (t.error.lerp(t.close_hover, h), Rgba::WHITE, None),
    };
    if resp.down {
        fill = match kind {
            ButtonKind::Primary => t.accent_pressed,
            ButtonKind::Danger => t.close_hover,
            _ => t.control_pressed,
        };
    }
    if !enabled {
        fill = fill.fade(0.45);
        ink = ink.fade(0.45);
    }
    ui.fill_rrect(r, size::R_CONTROL, fill);
    if kind == ButtonKind::Primary && enabled {
        // A faint top sheen makes the accent button read as raised glass.
        ui.gradient_rrect(
            r,
            size::R_CONTROL,
            Rgba::WHITE.fade(0.18),
            Rgba::WHITE.fade(0.0),
        );
    }
    if let Some(s) = stroke {
        ui.stroke_rrect(r, size::R_CONTROL, s);
    }
    let weight = if kind == ButtonKind::Primary {
        Weight::Semibold
    } else {
        Weight::Regular
    };
    let tw = ui.measure(label, size::BODY, weight);
    let iw = if icon.is_some() { 16.0 } else { 0.0 };
    let gap = if icon.is_some() && !label.is_empty() {
        8.0
    } else {
        0.0
    };
    let total = (iw + gap + tw).min(r.w - 16.0);
    let mut x = r.x + (r.w - total) / 2.0;
    if let Some(ic) = icon {
        ui.icon(Rect::new(x, r.center().y - 8.0, 16.0, 16.0), ic, ink);
        x += iw + gap;
    }
    if !label.is_empty() {
        let lr = Rect::new(x, r.y, r.right() - x - 6.0, r.h);
        ui.text_in(lr, label, size::BODY, weight, ink, Align::Left);
    }
    resp
}

/// A square icon-only button with a tooltip.
pub fn icon_button(ui: &mut Ui, id: Id, r: Rect, icon: Icon, tip: &str) -> Response {
    let resp = ui.interact(id, r);
    let t = ui.theme;
    let h = hover_anim(ui, id, resp.hovered);
    let fill = if resp.down {
        t.control_pressed
    } else {
        t.control_hover.fade(h)
    };
    ui.fill_rrect(r, size::R_CONTROL, fill);
    ui.icon(r.centered(16.0, 16.0), icon, t.text2.lerp(t.text, h));
    if resp.hovered && !tip.is_empty() && ui.hover_time(id) > 0.5 {
        ui.tooltip(tip);
    } else if resp.hovered && !tip.is_empty() {
        ui.request_repaint(0.55);
    }
    resp
}

/// A selectable pill (filters, quick targets).
pub fn chip(ui: &mut Ui, id: Id, r: Rect, label: &str, selected: bool) -> Response {
    let resp = ui.interact(id, r);
    let t = ui.theme;
    let h = hover_anim(ui, id, resp.hovered);
    let rad = r.h / 2.0;
    if selected {
        ui.fill_rrect(r, rad, t.accent.lerp(t.accent_hover, h));
        ui.text_in(
            r,
            label,
            size::BODY,
            Weight::Semibold,
            t.on_accent,
            Align::Center,
        );
    } else {
        ui.fill_rrect(r, rad, t.control.lerp(t.control_hover, h));
        ui.stroke_rrect(r, rad, t.control_stroke);
        ui.text_in(r, label, size::BODY, Weight::Regular, t.text, Align::Center);
    }
    resp
}

/// An 18 px checkbox. The caller flips its state on `clicked`.
pub fn checkbox(ui: &mut Ui, id: Id, r: Rect, checked: bool) -> Response {
    let resp = ui.interact(id, r.inset(-4.0));
    let t = ui.theme;
    let b = r.centered(18.0, 18.0);
    let k = ui.anim(id.with("on"), if checked { 1.0 } else { 0.0 }, 26.0);
    let h = hover_anim(ui, id, resp.hovered);
    ui.fill_rrect(b, 4.0, t.control.lerp(t.control_hover, h).lerp(t.accent, k));
    if k < 1.0 {
        ui.stroke_rrect(b, 4.0, t.text2.fade(1.0 - k));
    }
    if k > 0.01 {
        ui.icon(b.inset(2.0), Icon::Check, t.on_accent.fade(k));
    }
    resp
}

/// A segmented control; returns true when the selection changed.
pub fn segmented(ui: &mut Ui, id: Id, r: Rect, items: &[&str], sel: &mut usize) -> bool {
    let n = items.len().max(1);
    let t = ui.theme;
    ui.fill_rrect(r, size::R_CONTROL, t.control_pressed);
    ui.stroke_rrect(r, size::R_CONTROL, t.control_stroke);
    let seg = r.w / n as f32;
    let x = ui.anim(id.with("pill"), *sel as f32, 20.0);
    let pill = Rect::new(r.x + x * seg, r.y, seg, r.h).inset(3.0);
    if !t.dark {
        ui.shadow(pill, size::R_CONTROL - 2.0, 3.0, 1.0, t.shadow.fade(0.6));
    }
    ui.fill_rrect(
        pill,
        size::R_CONTROL - 2.0,
        if t.dark {
            Rgba::WHITE.fade(0.14)
        } else {
            Rgba::WHITE
        },
    );
    let mut changed = false;
    for (i, label) in items.iter().enumerate() {
        let cell = Rect::new(r.x + i as f32 * seg, r.y, seg, r.h);
        let resp = ui.interact(id.with(i), cell);
        if resp.clicked && *sel != i {
            *sel = i;
            changed = true;
        }
        let on = *sel == i;
        let ink = if on { t.text } else { t.text2 };
        let w = if on {
            Weight::Semibold
        } else {
            Weight::Regular
        };
        ui.text_in(
            cell.inset_xy(6.0, 0.0),
            label,
            size::BODY,
            w,
            ink,
            Align::Center,
        );
    }
    changed
}

/// A dropdown. Its list opens in a popup above everything; returns true when the
/// selection changed (one frame after the click, when the popup reports back).
pub fn dropdown(ui: &mut Ui, id: Id, r: Rect, items: &[String], sel: &mut usize) -> bool {
    let mut changed = false;
    if let Some((owner, i)) = ui.popup_choice
        && owner == id
    {
        ui.popup_choice = None;
        if i < items.len() && i != *sel {
            *sel = i;
            changed = true;
        }
    }
    let open = ui.popup.as_ref().is_some_and(|p| p.owner == id);
    let resp = ui.interact(id, r);
    let t = ui.theme;
    let h = hover_anim(ui, id, resp.hovered || open);
    ui.fill_rrect(r, size::R_CONTROL, t.control.lerp(t.control_hover, h));
    ui.stroke_rrect(
        r,
        size::R_CONTROL,
        if open { t.accent } else { t.control_stroke },
    );
    let label = items.get(*sel).map_or("—", String::as_str);
    let lr = Rect::new(r.x + 10.0, r.y, r.w - 34.0, r.h);
    ui.text_in(lr, label, size::BODY, Weight::Semibold, t.text, Align::Left);
    ui.icon(
        Rect::new(r.right() - 26.0, r.center().y - 8.0, 16.0, 16.0),
        Icon::ChevronDown,
        t.text2,
    );
    if resp.clicked && !items.is_empty() {
        if open {
            ui.popup = None;
        } else {
            let row = 30.0;
            let longest = items
                .iter()
                .map(|s| ui.fonts.measure(s, size::BODY, Weight::Regular))
                .fold(0.0f32, f32::max);
            let w = r.w.max(longest + 56.0).min(ui.size.0 - 16.0);
            let hgt = (items.len().min(10) as f32) * row + 8.0;
            let below = r.bottom() + 4.0;
            let y = if below + hgt > ui.size.1 - 8.0 && r.y - 4.0 - hgt > 8.0 {
                r.y - 4.0 - hgt
            } else {
                below.min(ui.size.1 - hgt - 8.0).max(8.0)
            };
            let x = r.x.min(ui.size.0 - w - 8.0).max(8.0);
            // Start scrolled so the selected item is visible.
            let max_scroll = (items.len() as f32 * row + 8.0 - hgt).max(0.0);
            let scroll = (*sel as f32 * row - hgt / 2.0 + row).clamp(0.0, max_scroll);
            ui.popup = Some(Popup {
                owner: id,
                anchor: r,
                items: items.to_vec(),
                selected: *sel,
                scroll,
                rect: Rect::new(x, y, w, hgt),
            });
        }
        ui.request_repaint(0.0);
    }
    changed
}

impl Ui {
    pub(crate) fn draw_popup(&mut self) {
        let Some(mut p) = self.popup.take() else {
            return;
        };
        let t = self.theme;
        let row = 30.0;
        let content = p.items.len() as f32 * row + 8.0;
        let max_scroll = (content - p.rect.h).max(0.0);
        let ptr = self.input.pointer;
        let inside = ptr.is_some_and(|q| p.rect.contains(q));
        if inside && self.input.wheel != 0.0 {
            p.scroll = (p.scroll + self.input.wheel).clamp(0.0, max_scroll);
        }
        if self.input.key(Key::Escape) {
            self.request_repaint(0.0);
            return;
        }
        if self.input.pressed && !inside && !ptr.is_some_and(|q| p.anchor.contains(q)) {
            // Click outside: dismiss (the click is not passed to what's underneath).
            self.request_repaint(0.0);
            return;
        }
        let fill = if t.dark {
            Rgba::hexa(0x2C2C2E, 248)
        } else {
            Rgba::hexa(0xFCFCFC, 250)
        };
        self.glass(p.rect, size::R_CARD, fill, 14.0);
        self.push_clip(p.rect.inset(4.0));
        let mut chosen = None;
        for (i, item) in p.items.iter().enumerate() {
            let y = p.rect.y + 4.0 + i as f32 * row - p.scroll;
            if y + row < p.rect.y || y > p.rect.bottom() {
                continue;
            }
            let rr = Rect::new(p.rect.x + 4.0, y, p.rect.w - 8.0, row);
            let hover = ptr.is_some_and(|q| rr.contains(q)) && inside;
            if i == p.selected {
                self.fill_rrect(rr, size::R_CONTROL, t.accent_wash);
                self.fill_rrect(
                    Rect::new(rr.x + 2.0, rr.center().y - 7.0, 3.0, 14.0),
                    1.5,
                    t.accent,
                );
            } else if hover {
                self.fill_rrect(rr, size::R_CONTROL, t.control_hover);
            }
            let w = if i == p.selected {
                Weight::Semibold
            } else {
                Weight::Regular
            };
            self.text_in(
                rr.inset_xy(14.0, 0.0),
                item,
                size::BODY,
                w,
                t.text,
                Align::Left,
            );
            if hover {
                self.cursor = Cursor::Hand;
                if self.input.released {
                    chosen = Some(i);
                }
            }
        }
        self.pop_clip();
        if let Some(i) = chosen {
            self.popup_choice = Some((p.owner, i));
            self.request_repaint(0.0);
        } else {
            self.popup = Some(p);
        }
    }

    pub(crate) fn draw_tooltip(&mut self) {
        let Some((text, at)) = self.tooltip.take() else {
            return;
        };
        let t = self.theme;
        let lines = self
            .fonts
            .wrap(&text, size::CAPTION, Weight::Regular, 320.0);
        let w = lines
            .iter()
            .map(|l| self.fonts.measure(l, size::CAPTION, Weight::Regular))
            .fold(0.0f32, f32::max)
            + 16.0;
        let h = lines.len() as f32 * 16.0 + 10.0;
        let x = (at.x + 12.0).min(self.size.0 - w - 4.0).max(4.0);
        let mut y = at.y + 20.0;
        if y + h > self.size.1 - 4.0 {
            y = at.y - h - 8.0;
        }
        let r = Rect::new(x, y, w, h);
        let fill = if t.dark {
            Rgba::hexa(0x2C2C2E, 250)
        } else {
            Rgba::hexa(0xFFFFFF, 252)
        };
        self.glass(r, 6.0, fill, 8.0);
        for (i, l) in lines.iter().enumerate() {
            let lr = Rect::new(x + 8.0, y + 5.0 + i as f32 * 16.0, w - 16.0, 16.0);
            self.text_in(lr, l, size::CAPTION, Weight::Regular, t.text, Align::Left);
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextResponse {
    pub changed: bool,
    pub submitted: bool,
    pub focused: bool,
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices().nth(char_idx).map_or(s.len(), |(b, _)| b)
}

/// A single-line text field.
pub fn text_field(
    ui: &mut Ui,
    id: Id,
    r: Rect,
    text: &mut String,
    placeholder: &str,
    invalid: bool,
) -> TextResponse {
    edit_field(ui, id, r, text, placeholder, invalid, false)
}

/// A password field: the text is shown as dots.
pub fn secret_field(
    ui: &mut Ui,
    id: Id,
    r: Rect,
    text: &mut String,
    placeholder: &str,
    invalid: bool,
) -> TextResponse {
    edit_field(ui, id, r, text, placeholder, invalid, true)
}

fn edit_field(
    ui: &mut Ui,
    id: Id,
    r: Rect,
    text: &mut String,
    placeholder: &str,
    invalid: bool,
    mask: bool,
) -> TextResponse {
    // What's drawn and measured: the text, or one dot per character.
    let shown = |t: &str| -> String {
        if mask {
            "•".repeat(t.chars().count())
        } else {
            t.to_string()
        }
    };
    let resp = ui.interact(id, r);
    if resp.hovered {
        ui.cursor = Cursor::Text;
    }
    let mut out = TextResponse::default();
    let n = text.chars().count();
    let pad = 10.0;
    if resp.pressed {
        ui.set_focus(id);
        // Caret at the click position.
        let st = ui.edits.get(&id).cloned().unwrap_or_default();
        let px = ui
            .input
            .pointer
            .map_or(0.0, |p| p.x - r.x - pad + st.scroll);
        let mut best = (0usize, f32::MAX);
        let mut acc = String::new();
        for i in 0..=n {
            let w = ui.fonts.measure(&acc, size::BODY, Weight::Regular);
            if (w - px).abs() < best.1 {
                best = (i, (w - px).abs());
            }
            if let Some(c) = text.chars().nth(i) {
                acc.push(if mask { '•' } else { c });
            }
        }
        ui.edits.insert(
            id,
            EditState {
                caret: best.0,
                anchor: best.0,
                scroll: st.scroll,
            },
        );
    }
    let focused = ui.has_focus(id);
    if focused && ui.input.pressed && !resp.hovered {
        ui.focus = Id::NONE;
    }
    let focused = ui.focus == id;
    out.focused = focused;
    let mut st = ui.edits.get(&id).cloned().unwrap_or(EditState {
        caret: n,
        anchor: n,
        scroll: 0.0,
    });
    st.caret = st.caret.min(n);
    st.anchor = st.anchor.min(n);
    if focused {
        let keys = ui.input.keys.clone();
        let typed = ui.input.text.clone();
        let (lo, hi) = (st.caret.min(st.anchor), st.caret.max(st.anchor));
        let delete_sel = |text: &mut String, st: &mut EditState| -> bool {
            if lo == hi {
                return false;
            }
            let (a, b) = (byte_at(text, lo), byte_at(text, hi));
            text.replace_range(a..b, "");
            st.caret = lo;
            st.anchor = lo;
            true
        };
        let clean: String = typed.chars().filter(|c| !c.is_control()).collect();
        if !clean.is_empty() {
            delete_sel(text, &mut st);
            let b = byte_at(text, st.caret);
            text.insert_str(b, &clean);
            st.caret += clean.chars().count();
            st.anchor = st.caret;
            out.changed = true;
        }
        for (k, m) in keys {
            let len = text.chars().count();
            match k {
                Key::Backspace => {
                    if !delete_sel(text, &mut st) && st.caret > 0 {
                        let (a, b) = (byte_at(text, st.caret - 1), byte_at(text, st.caret));
                        text.replace_range(a..b, "");
                        st.caret -= 1;
                        st.anchor = st.caret;
                    }
                    out.changed = true;
                }
                Key::Delete => {
                    if !delete_sel(text, &mut st) && st.caret < len {
                        let (a, b) = (byte_at(text, st.caret), byte_at(text, st.caret + 1));
                        text.replace_range(a..b, "");
                    }
                    out.changed = true;
                }
                Key::Left => {
                    st.caret = st.caret.saturating_sub(1);
                    if !m.shift {
                        st.anchor = st.caret;
                    }
                }
                Key::Right => {
                    st.caret = (st.caret + 1).min(len);
                    if !m.shift {
                        st.anchor = st.caret;
                    }
                }
                Key::Home => {
                    st.caret = 0;
                    if !m.shift {
                        st.anchor = 0;
                    }
                }
                Key::End => {
                    st.caret = len;
                    if !m.shift {
                        st.anchor = len;
                    }
                }
                Key::A if m.ctrl => {
                    st.anchor = 0;
                    st.caret = len;
                }
                Key::Enter => out.submitted = true,
                Key::Escape => ui.focus = Id::NONE,
                _ => {}
            }
        }
        // Blink: on for 0.53 s, off for 0.53 s.
        ui.request_repaint(0.53);
    }
    let t = ui.theme;
    let h = hover_anim(ui, id, resp.hovered);
    ui.fill_rrect(
        r,
        size::R_CONTROL,
        if focused {
            if t.dark {
                Rgba::hexa(0x1E1E1E, 200)
            } else {
                Rgba::WHITE
            }
        } else {
            t.control.lerp(t.control_hover, h)
        },
    );
    let border = if invalid { t.error } else { t.control_stroke };
    ui.stroke_rrect(r, size::R_CONTROL, border);
    if focused && !invalid {
        // Fluent-style focus: an accent underline.
        ui.push_clip(r);
        ui.fill_rect(Rect::new(r.x, r.bottom() - 2.0, r.w, 2.0), t.accent);
        ui.pop_clip();
    }
    let inner = r.inset_xy(pad, 0.0);
    ui.push_clip(inner);
    let caret_x = ui.measure(
        &shown(&text.chars().take(st.caret).collect::<String>()),
        size::BODY,
        Weight::Regular,
    );
    // Keep the caret in view.
    if caret_x - st.scroll > inner.w - 2.0 {
        st.scroll = caret_x - inner.w + 2.0;
    }
    if caret_x < st.scroll {
        st.scroll = caret_x;
    }
    let x0 = inner.x - st.scroll;
    let base = r.y + ui.fonts.baseline(size::BODY, Weight::Regular, r.h);
    if text.is_empty() && !focused {
        ui.text(
            inner.x,
            base,
            placeholder,
            size::BODY,
            Weight::Regular,
            t.text3,
        );
    } else {
        if focused && st.caret != st.anchor {
            let (lo, hi) = (st.caret.min(st.anchor), st.caret.max(st.anchor));
            let a = ui.measure(
                &shown(&text.chars().take(lo).collect::<String>()),
                size::BODY,
                Weight::Regular,
            );
            let b = ui.measure(
                &shown(&text.chars().take(hi).collect::<String>()),
                size::BODY,
                Weight::Regular,
            );
            ui.fill_rect(
                Rect::new(x0 + a, r.y + 7.0, b - a, r.h - 14.0),
                t.accent.fade(0.35),
            );
        }
        ui.text(x0, base, &shown(text), size::BODY, Weight::Regular, t.text);
        if focused && (ui.input.time * 1.9).fract() < 0.5 {
            ui.fill_rect(Rect::new(x0 + caret_x, r.y + 7.0, 1.0, r.h - 14.0), t.text);
        }
    }
    ui.pop_clip();
    ui.edits.insert(id, st);
    out
}

/// A thin progress bar; `None` = indeterminate.
pub fn progress(ui: &mut Ui, r: Rect, frac: Option<f32>) {
    let t = ui.theme;
    ui.fill_rrect(r, r.h / 2.0, t.control_pressed.lerp(t.text3, 0.25));
    match frac {
        Some(f) => {
            let w = r.w * f.clamp(0.0, 1.0);
            if w > 0.5 {
                ui.fill_rrect(Rect::new(r.x, r.y, w.max(r.h), r.h), r.h / 2.0, t.accent);
            }
        }
        None => {
            let p = (ui.input.time * 0.8).fract() as f32;
            let seg = r.w * 0.3;
            let x = r.x - seg + (r.w + seg) * p;
            ui.push_clip(r);
            ui.fill_rrect(Rect::new(x, r.y, seg, r.h), r.h / 2.0, t.accent);
            ui.pop_clip();
            ui.request_repaint(1.0 / 60.0);
        }
    }
}

/// A rotating arc.
pub fn spinner(ui: &mut Ui, c: Point, radius: f32, color: Rgba) {
    let a0 = (ui.input.time * std::f64::consts::TAU * 0.9) as f32;
    ui.stroke_logical(
        |pb, s| {
            let n = 24;
            for i in 0..=n {
                let a = a0 + 4.4 * i as f32 / n as f32;
                let (x, y) = ((c.x + radius * a.cos()) * s, (c.y + radius * a.sin()) * s);
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
        },
        color,
        (radius / 4.0).clamp(1.5, 3.0),
    );
    ui.request_repaint(1.0 / 60.0);
}

/// Begin a vertical scroll area. Returns the current offset; draw content shifted up by
/// it, then call [`scroll_end`].
pub fn scroll_begin(ui: &mut Ui, id: Id, view: Rect, content_h: f32) -> f32 {
    let max = (content_h - view.h).max(0.0);
    let (mut off, mut target) = ui.scrolls.get(&id).copied().unwrap_or((0.0, 0.0));
    if ui.pointer_in(view) && ui.input.wheel != 0.0 {
        target += ui.input.wheel;
    }
    target = target.clamp(0.0, max);
    let k = 1.0 - (-ui.input.dt.clamp(0.0, 0.1) * 22.0).exp();
    off += (target - off) * k;
    if (target - off).abs() < 0.5 {
        off = target;
    } else {
        ui.request_repaint(1.0 / 60.0);
    }
    off = off.clamp(0.0, max);
    ui.scrolls.insert(id, (off, target));
    ui.push_clip(view);
    off
}

/// Jump (smoothly) to `y` in a scroll area.
pub fn scroll_to(ui: &mut Ui, id: Id, y: f32) {
    let e = ui.scrolls.entry(id).or_insert((0.0, 0.0));
    e.1 = y.max(0.0);
    ui.request_repaint(0.0);
}

pub fn scroll_end(ui: &mut Ui, id: Id, view: Rect, content_h: f32) {
    ui.pop_clip();
    if content_h <= view.h + 0.5 {
        return;
    }
    let (off, _) = ui.scrolls.get(&id).copied().unwrap_or((0.0, 0.0));
    let max = content_h - view.h;
    let th = (view.h * view.h / content_h).max(28.0);
    let ty = view.y + (view.h - th) * (off / max);
    let track = Rect::new(view.right() - 10.0, view.y, 10.0, view.h);
    let bar_id = id.with("bar");
    let resp = ui.interact(bar_id, track);
    let t = ui.theme;
    let h = hover_anim(ui, bar_id, resp.hovered || resp.down);
    if resp.down
        && let Some(p) = ui.input.pointer
    {
        let f = ((p.y - view.y - th / 2.0) / (view.h - th)).clamp(0.0, 1.0);
        ui.scrolls.insert(id, (f * max, f * max));
    }
    let w = 3.0 + 3.0 * h;
    let bar = Rect::new(view.right() - 3.0 - w, ty, w, th);
    ui.fill_rrect(bar, w / 2.0, t.text3.lerp(t.text2, h));
}

/// The title bar's minimize / maximize / close buttons. When the OS draws native ones
/// (Windows with the frame extended into the client area), this draws nothing; otherwise
/// (snapshots, hosts without native buttons) it draws ours and records hit zones.
pub fn window_controls(ui: &mut Ui, r: Rect, closable_only: bool) {
    if ui.caption_buttons.is_some() {
        // The OS draws native caption buttons there (hover, Snap Layouts, accessibility).
        return;
    }
    let t = ui.theme;
    let bw = 46.0;
    let mut row = r;
    let mut place = |ui: &mut Ui, zone: Zone, icon: Icon| {
        let b = row.cut_right(bw);
        ui.add_zone(b, zone);
        let hover = ui.input.zone_hover == Some(zone);
        let down = ui.input.zone_pressed == Some(zone);
        let (fill, ink) = match (zone, hover, down) {
            (Zone::Close, true, _) => (t.close_hover, Rgba::WHITE),
            (_, true, true) => (t.control_pressed, t.text),
            (_, true, false) => (t.control_hover, t.text),
            _ => (Rgba::CLEAR, t.text2),
        };
        ui.fill_rect(b, fill);
        ui.icon(b.centered(13.0, 13.0), icon, ink);
    };
    place(ui, Zone::Close, Icon::Close);
    if !closable_only {
        let max_icon = if ui.input.maximized {
            Icon::Restore
        } else {
            Icon::Maximize
        };
        place(ui, Zone::Maximize, max_icon);
        place(ui, Zone::Minimize, Icon::Minimize);
    }
}

/// Mark `r` as the draggable title bar.
pub fn caption(ui: &mut Ui, r: Rect) {
    ui.add_zone(r, Zone::Caption);
}
