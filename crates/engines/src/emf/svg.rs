//! A played-back [`Picture`] as SVG: paths, clip groups, embedded PNGs and text (the
//! viewer's fonts; alignment by `text-anchor`, or by the metafile's own advances).

use std::fmt::Write as _;

use super::{Item, Picture, TextItem};
use crate::docxlayout::layout::{PathCmd, Pen};
use crate::docxlayout::props::Rgb;

fn n(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".into()
    } else {
        s.into()
    }
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn d(path: &[PathCmd]) -> String {
    let mut s = String::with_capacity(path.len() * 12);
    for c in path {
        let _ = match *c {
            PathCmd::Move(x, y) => write!(s, "M{} {}", n(x), n(y)),
            PathCmd::Line(x, y) => write!(s, "L{} {}", n(x), n(y)),
            PathCmd::Quad(a, b, x, y) => write!(s, "Q{} {} {} {}", n(a), n(b), n(x), n(y)),
            PathCmd::Cubic(a, b, c2, d2, x, y) => write!(
                s,
                "C{} {} {} {} {} {}",
                n(a),
                n(b),
                n(c2),
                n(d2),
                n(x),
                n(y)
            ),
            PathCmd::Close => write!(s, "Z"),
        };
    }
    s
}

fn esc(t: &str) -> String {
    let mut s = String::with_capacity(t.len());
    for c in t.chars() {
        match c {
            '&' => s.push_str("&amp;"),
            '<' => s.push_str("&lt;"),
            '>' => s.push_str("&gt;"),
            '"' => s.push_str("&quot;"),
            c if (c as u32) < 0x20 => s.push(' '),
            c => s.push(c),
        }
    }
    s
}

pub(crate) fn base64(b: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let v = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..4 {
            if k <= c.len() {
                s.push(T[(v >> (18 - 6 * k) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

fn matrix(m: &[f32; 6]) -> String {
    format!(
        "matrix({} {} {} {} {} {})",
        n(m[0]),
        n(m[1]),
        n(m[2]),
        n(m[3]),
        n(m[4]),
        n(m[5])
    )
}

fn stroke(pen: &Pen) -> String {
    let mut s = format!(
        r#" fill="none" stroke="{}" stroke-width="{}""#,
        hex(pen.color),
        n(pen.width)
    );
    if pen.round {
        s.push_str(r#" stroke-linecap="round" stroke-linejoin="round""#);
    }
    if !pen.dash.is_empty() {
        let a: Vec<String> = pen.dash.iter().map(|v| n(v * pen.width)).collect();
        let _ = write!(s, r#" stroke-dasharray="{}""#, a.join(" "));
    }
    s
}

fn text(t: &TextItem, out: &mut String) {
    // Without the font's metrics, a cell height becomes an em of about 0.85 of it.
    let size = if t.cell { t.size * 0.85 } else { t.size };
    let weight = if t.bold { r#" font-weight="bold""# } else { "" };
    let style = if t.italic {
        r#" font-style="italic""#
    } else {
        ""
    };
    let mut deco = Vec::new();
    if t.underline {
        deco.push("underline");
    }
    if t.strike {
        deco.push("line-through");
    }
    let deco = if deco.is_empty() {
        String::new()
    } else {
        format!(r#" text-decoration="{}""#, deco.join(" "))
    };
    let base = match t.valign {
        0 => r#" dominant-baseline="text-before-edge""#,
        2 => r#" dominant-baseline="text-after-edge""#,
        _ => "",
    };
    let (xs, anchor) = if t.dx.is_empty() {
        let a = match t.halign {
            1 => r#" text-anchor="middle""#,
            2 => r#" text-anchor="end""#,
            _ => "",
        };
        ("0".to_string(), a)
    } else {
        let total: f32 = t.dx.iter().sum();
        let mut x = match t.halign {
            1 => -total / 2.0,
            2 => -total,
            _ => 0.0,
        };
        let mut v = Vec::with_capacity(t.dx.len());
        for a in &t.dx {
            v.push(n(x));
            x += a;
        }
        (v.join(" "), "")
    };
    let _ = write!(
        out,
        r#"<text transform="{}" x="{}" y="0" font-family="{}" font-size="{}" fill="{}"{weight}{style}{deco}{base}{anchor} xml:space="preserve">{}</text>"#,
        matrix(&t.m),
        xs,
        esc(&t.family),
        n(size),
        hex(t.color),
        esc(&t.text)
    );
}

pub(crate) fn to_svg(pic: &Picture) -> String {
    let mut out = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="{w}pt" height="{h}pt" viewBox="0 0 {w} {h}"><defs>"#,
        w = n(pic.w),
        h = n(pic.h)
    );
    let mut defs = String::new();
    let mut body = String::new();
    let mut clips = 0usize;
    let mut grads = 0usize;
    let mut open = 0usize;
    for it in &pic.items {
        match it {
            Item::Fill {
                path,
                color,
                alpha,
                even_odd,
            } => {
                let _ = write!(
                    body,
                    r#"<path d="{}" fill="{}"{}{}/>"#,
                    d(path),
                    hex(*color),
                    if *even_odd {
                        r#" fill-rule="evenodd""#
                    } else {
                        ""
                    },
                    if *alpha < 1.0 {
                        format!(r#" fill-opacity="{}""#, n(*alpha))
                    } else {
                        String::new()
                    }
                );
            }
            Item::Stroke { path, pen } => {
                let _ = write!(body, r#"<path d="{}"{}/>"#, d(path), stroke(pen));
            }
            Item::Gradient {
                path,
                from,
                to,
                c0,
                c1,
            } => {
                grads += 1;
                let _ = write!(
                    defs,
                    r#"<linearGradient id="g{grads}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient>"#,
                    n(from.0),
                    n(from.1),
                    n(to.0),
                    n(to.1),
                    hex(*c0),
                    hex(*c1)
                );
                let _ = write!(body, r#"<path d="{}" fill="url(#g{grads})"/>"#, d(path));
            }
            Item::Image { png, m } => {
                let _ = write!(
                    body,
                    r#"<image transform="{}" width="1" height="1" preserveAspectRatio="none" xlink:href="data:image/png;base64,{}"/>"#,
                    matrix(m),
                    base64(png)
                );
            }
            Item::Clip { path, even_odd } => {
                clips += 1;
                let _ = write!(
                    defs,
                    r#"<clipPath id="c{clips}"><path d="{}"{}/></clipPath>"#,
                    d(path),
                    if *even_odd {
                        r#" clip-rule="evenodd""#
                    } else {
                        ""
                    }
                );
                let _ = write!(body, r#"<g clip-path="url(#c{clips})">"#);
                open += 1;
            }
            Item::Unclip => {
                if open > 0 {
                    body.push_str("</g>");
                    open -= 1;
                }
            }
            Item::Text(t) => text(t, &mut body),
        }
    }
    for _ in 0..open {
        body.push_str("</g>");
    }
    out.push_str(&defs);
    out.push_str("</defs>");
    out.push_str(&body);
    out.push_str("</svg>");
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_pads() {
        assert_eq!(super::base64(b"M"), "TQ==");
        assert_eq!(super::base64(b"Ma"), "TWE=");
        assert_eq!(super::base64(b"Man"), "TWFu");
    }
}
