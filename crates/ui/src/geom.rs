//! Rectangles and RectCut layout: a layout is a sequence of cuts off a rectangle, so
//! there is no constraint solver and no allocation.

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }
    pub fn inset(self, d: f32) -> Rect {
        self.inset_xy(d, d)
    }
    pub fn inset_xy(self, dx: f32, dy: f32) -> Rect {
        Rect::new(
            self.x + dx,
            self.y + dy,
            (self.w - 2.0 * dx).max(0.0),
            (self.h - 2.0 * dy).max(0.0),
        )
    }
    pub fn translate(self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn scale(self, s: f32) -> Rect {
        Rect::new(self.x * s, self.y * s, self.w * s, self.h * s)
    }
    pub fn intersect(self, o: Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    /// A `w` × `h` rectangle centered in this one.
    pub fn centered(self, w: f32, h: f32) -> Rect {
        Rect::new(
            self.x + (self.w - w) / 2.0,
            self.y + (self.h - h) / 2.0,
            w,
            h,
        )
    }

    // RectCut: each cut shrinks `self` and returns the piece cut off.
    pub fn cut_top(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        let r = Rect::new(self.x, self.y, self.w, h);
        self.y += h;
        self.h -= h;
        r
    }
    pub fn cut_bottom(&mut self, h: f32) -> Rect {
        let h = h.min(self.h).max(0.0);
        self.h -= h;
        Rect::new(self.x, self.y + self.h, self.w, h)
    }
    pub fn cut_left(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        let r = Rect::new(self.x, self.y, w, self.h);
        self.x += w;
        self.w -= w;
        r
    }
    pub fn cut_right(&mut self, w: f32) -> Rect {
        let w = w.min(self.w).max(0.0);
        self.w -= w;
        Rect::new(self.x + self.w, self.y, w, self.h)
    }
    /// Cut and leave a gap after the piece (for rows of controls).
    pub fn cut_left_gap(&mut self, w: f32, gap: f32) -> Rect {
        let r = self.cut_left(w);
        self.cut_left(gap);
        r
    }
    pub fn cut_top_gap(&mut self, h: f32, gap: f32) -> Rect {
        let r = self.cut_top(h);
        self.cut_top(gap);
        r
    }
}

/// A grid of equal cells filling `width`: how many columns of at least `min_w` fit, and
/// the resulting cell width.
pub fn grid_columns(width: f32, min_w: f32, gap: f32) -> (usize, f32) {
    let cols = (((width + gap) / (min_w + gap)).floor() as usize).max(1);
    let w = (width - gap * (cols as f32 - 1.0)) / cols as f32;
    (cols, w.max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_cut() {
        let mut r = Rect::new(0.0, 0.0, 100.0, 50.0);
        let top = r.cut_top(10.0);
        let left = r.cut_left(30.0);
        let right = r.cut_right(500.0);
        assert_eq!(top, Rect::new(0.0, 0.0, 100.0, 10.0));
        assert_eq!(left, Rect::new(0.0, 10.0, 30.0, 40.0));
        assert_eq!(right, Rect::new(30.0, 10.0, 70.0, 40.0));
        assert!(r.is_empty());
        let (cols, w) = grid_columns(400.0, 120.0, 10.0);
        assert_eq!(cols, 3);
        assert!((w - 126.666).abs() < 0.01);
        assert_eq!(grid_columns(50.0, 120.0, 10.0).0, 1);
    }
}
