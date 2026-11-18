//! Where a page is, as the reader sees it.
//!
//! A page's user space can be rotated for display (`/Rotate`) and cut down by `/CropBox`.
//! Tools think in *display* coordinates (points, origin bottom-left of the page as shown,
//! upright), and [`Geom::to_user`] maps them into the page's own space.

use lopdf::{Document, Object, ObjectId};

use super::{deref, inherited};

/// `[x0, y0, x1, y1]` in user space, normalised so x0 < x1 and y0 < y1.
pub type Box4 = [f32; 4];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geom {
    /// The visible box (CropBox clipped to MediaBox) in user space.
    pub crop: Box4,
    pub media: Box4,
    /// Clockwise display rotation: 0, 90, 180 or 270.
    pub rotate: i64,
}

fn num(o: &Object) -> Option<f32> {
    match o {
        Object::Integer(i) => Some(*i as f32),
        Object::Real(r) => Some(*r),
        _ => None,
    }
}

pub(crate) fn read_box(doc: &Document, o: &Object) -> Option<Box4> {
    let a = deref(doc, o).as_array().ok()?;
    if a.len() != 4 {
        return None;
    }
    let v: Vec<f32> = a.iter().filter_map(|x| num(deref(doc, x))).collect();
    if v.len() != 4 {
        return None;
    }
    let b = [
        v[0].min(v[2]),
        v[1].min(v[3]),
        v[0].max(v[2]),
        v[1].max(v[3]),
    ];
    (b[2] - b[0] > 0.5 && b[3] - b[1] > 0.5).then_some(b)
}

pub(crate) fn box_object(b: Box4) -> Object {
    Object::Array(b.iter().map(|&v| Object::Real(round(v))).collect())
}

/// Two decimals: plenty for points, and keeps files small and diffs readable.
pub(crate) fn round(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

impl Geom {
    pub fn of(doc: &Document, page: ObjectId) -> Geom {
        let media = inherited(doc, page, b"MediaBox")
            .and_then(|o| read_box(doc, o))
            .unwrap_or([0.0, 0.0, 612.0, 792.0]);
        let crop = inherited(doc, page, b"CropBox")
            .and_then(|o| read_box(doc, o))
            .map(|c| {
                [
                    c[0].max(media[0]),
                    c[1].max(media[1]),
                    c[2].min(media[2]),
                    c[3].min(media[3]),
                ]
            })
            .filter(|c| c[2] - c[0] > 0.5 && c[3] - c[1] > 0.5)
            .unwrap_or(media);
        let rotate = inherited(doc, page, b"Rotate")
            .and_then(|o| deref(doc, o).as_i64().ok())
            .map_or(0, |r| r.rem_euclid(360) / 90 * 90);
        Geom {
            crop,
            media,
            rotate,
        }
    }

    /// Width and height as displayed.
    pub fn display_size(&self) -> (f32, f32) {
        let (w, h) = (self.crop[2] - self.crop[0], self.crop[3] - self.crop[1]);
        if self.rotate % 180 == 0 {
            (w, h)
        } else {
            (h, w)
        }
    }

    /// The matrix `[a b c d e f]` taking display coordinates into user space.
    pub fn to_user(&self) -> [f32; 6] {
        let [x0, y0, x1, y1] = self.crop;
        let (cw, ch) = (x1 - x0, y1 - y0);
        match self.rotate {
            90 => [0.0, 1.0, -1.0, 0.0, x0 + cw, y0],
            180 => [-1.0, 0.0, 0.0, -1.0, x0 + cw, y0 + ch],
            270 => [0.0, -1.0, 1.0, 0.0, x0, y0 + ch],
            _ => [1.0, 0.0, 0.0, 1.0, x0, y0],
        }
    }

    /// A display-space rectangle `[x0, y0, x1, y1]` in user space.
    pub fn rect_to_user(&self, r: Box4) -> Box4 {
        let m = self.to_user();
        let p = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
        let (ax, ay) = p(r[0], r[1]);
        let (bx, by) = p(r[2], r[3]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }
}

/// `a b c d e f cm` with compact numbers.
pub(crate) fn cm(m: [f32; 6]) -> String {
    let f = |v: f32| {
        let s = format!("{:.4}", v);
        let s = s.trim_end_matches('0').trim_end_matches('.');
        if s == "-0" {
            "0".to_string()
        } else {
            s.to_string()
        }
    };
    format!(
        "{} {} {} {} {} {} cm",
        f(m[0]),
        f(m[1]),
        f(m[2]),
        f(m[3]),
        f(m[4]),
        f(m[5])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_corners_map_into_user_space() {
        // A 600 × 800 crop box offset by (10, 20).
        for (rot, top_left_user) in [
            (0, (10.0, 820.0)),
            (90, (10.0, 20.0)),
            (180, (610.0, 20.0)),
            (270, (610.0, 820.0)),
        ] {
            let g = Geom {
                crop: [10.0, 20.0, 610.0, 820.0],
                media: [0.0, 0.0, 700.0, 900.0],
                rotate: rot,
            };
            let (_, h) = g.display_size();
            let m = g.to_user();
            let (x, y) = (0.0, h);
            let u = (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
            assert_eq!(u, top_left_user, "rotate {rot}");
        }
        assert_eq!(cm([1.0, 0.0, 0.0, 1.0, 10.5, -0.0]), "1 0 0 1 10.5 0 cm");
    }
}
