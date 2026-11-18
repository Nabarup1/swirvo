//! The document theme: its color scheme and its major (headings) and minor (body) fonts.
//! Word's defaults point here ("+Body", "Accent 1"), so most documents depend on it.

use std::collections::HashMap;

use super::props::Rgb;
use super::xml::El;

#[derive(Debug, Default, Clone)]
pub struct Theme {
    colors: HashMap<String, Rgb>,
    /// Latin, East Asian and complex-script typefaces of the major and minor fonts.
    pub major: [String; 3],
    pub minor: [String; 3],
}

impl Theme {
    pub fn parse(root: &El) -> Theme {
        let mut t = Theme::default();
        let Some(elems) = root.child("themeElements") else {
            return t;
        };
        if let Some(cs) = elems.child("clrScheme") {
            for c in cs.children() {
                if let Some(rgb) = c.children().find_map(|k| plain_color(k, &t)) {
                    t.colors.insert(c.name.to_string(), rgb);
                }
            }
        }
        if let Some(fs) = elems.child("fontScheme") {
            let read = |name: &str| -> [String; 3] {
                let f = fs.child(name);
                let face = |k: &str| {
                    f.and_then(|f| f.child(k))
                        .and_then(|e| e.attr("typeface"))
                        .unwrap_or("")
                        .to_string()
                };
                [face("latin"), face("ea"), face("cs")]
            };
            t.major = read("majorFont");
            t.minor = read("minorFont");
        }
        t
    }

    /// A Word theme color name (`accent1`, `text1`, `background2`, `hyperlink`…) or a
    /// DrawingML scheme name (`tx1`, `bg1`, `accent3`…).
    pub fn color(&self, name: &str) -> Option<Rgb> {
        let key = match name {
            "dark1" | "text1" | "tx1" | "dk1" => "dk1",
            "light1" | "background1" | "bg1" | "lt1" => "lt1",
            "dark2" | "text2" | "tx2" | "dk2" => "dk2",
            "light2" | "background2" | "bg2" | "lt2" => "lt2",
            "hyperlink" | "hlink" => "hlink",
            "followedHyperlink" | "folHlink" => "folHlink",
            other => other,
        };
        self.colors.get(key).copied().or(match key {
            "dk1" => Some(Rgb(0, 0, 0)),
            "lt1" => Some(Rgb(255, 255, 255)),
            "hlink" => Some(Rgb(5, 99, 193)),
            _ => None,
        })
    }

    /// A DrawingML color element (`a:srgbClr`, `a:schemeClr`, `a:sysClr`, `a:prstClr`) with
    /// its luminance, tint and shade modifiers applied.
    pub fn drawing_color(&self, e: &El) -> Option<Rgb> {
        let base = plain_color(e, self)?;
        let mut c = base;
        for m in e.children() {
            let v = m
                .val()
                .and_then(|v| v.parse::<f32>().ok())
                .unwrap_or(100_000.0)
                / 100_000.0;
            match &*m.name {
                "lumMod" | "lumOff" => {}
                "shade" => c = c.shade(v),
                "tint" => c = c.tint(v),
                _ => {}
            }
        }
        let lum_mod = e
            .child("lumMod")
            .and_then(|m| m.val()?.parse::<f32>().ok())
            .map(|v| v / 100_000.0);
        let lum_off = e
            .child("lumOff")
            .and_then(|m| m.val()?.parse::<f32>().ok())
            .map(|v| v / 100_000.0);
        if lum_mod.is_some() || lum_off.is_some() {
            let (h, s, l) = hsl(c);
            let l = (l * lum_mod.unwrap_or(1.0) + lum_off.unwrap_or(0.0)).clamp(0.0, 1.0);
            c = from_hsl(h, s, l);
        }
        Some(c)
    }
}

fn plain_color(e: &El, theme: &Theme) -> Option<Rgb> {
    match &*e.name {
        "srgbClr" => e.val().and_then(Rgb::hex),
        "sysClr" => e
            .attr("lastClr")
            .and_then(Rgb::hex)
            .or_else(|| match e.val() {
                Some("windowText") => Some(Rgb(0, 0, 0)),
                Some("window") => Some(Rgb(255, 255, 255)),
                _ => None,
            }),
        "schemeClr" => e.val().and_then(|v| theme.color(v)),
        "prstClr" => match e.val() {
            Some("black") => Some(Rgb(0, 0, 0)),
            Some("white") => Some(Rgb(255, 255, 255)),
            Some("red") => Some(Rgb(255, 0, 0)),
            Some("green") => Some(Rgb(0, 128, 0)),
            Some("blue") => Some(Rgb(0, 0, 255)),
            Some("yellow") => Some(Rgb(255, 255, 0)),
            Some("gray") => Some(Rgb(128, 128, 128)),
            _ => None,
        },
        _ => None,
    }
}

pub(crate) fn hsl(c: Rgb) -> (f32, f32, f32) {
    let (r, g, b) = (c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if (max - min).abs() < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

pub(crate) fn from_hsl(h: f32, s: f32, l: f32) -> Rgb {
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return Rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().clamp(0.0, 255.0) as u8
    };
    Rgb(f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luminance_modifiers() {
        // Word's "Blue, Accent 1, Lighter 80%": lumMod 20000, lumOff 80000.
        let (h, s, l) = hsl(Rgb(68, 114, 196));
        let c = from_hsl(h, s, l * 0.2 + 0.8);
        assert!(c.0 > 210 && c.2 > 235, "{c:?}");
        assert_eq!(from_hsl(h, s, l), Rgb(68, 114, 196));
    }
}
