//! DrawingML styling for slides: colours through the colour map, fills, lines, and the
//! nine-level text styles that placeholders inherit from the master and layout.

use std::collections::HashMap;

use crate::docxlayout::layout::{Paint, Pen};
use crate::docxlayout::props::{Align, Rgb, Underline};
use crate::docxlayout::theme::{Theme, from_hsl, hsl};
use crate::docxlayout::xml::El;

/// EMU per point.
pub const EMU: f32 = 12_700.0;

pub fn emu(v: &str) -> Option<f32> {
    v.trim()
        .parse::<f64>()
        .ok()
        .map(|n| (n / EMU as f64) as f32)
}

pub fn attr_emu(e: &El, k: &str) -> Option<f32> {
    e.attr(k).and_then(emu)
}

/// A DrawingML percentage (100000 = 100 %) as a fraction.
pub fn pct(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(p) = v.strip_suffix('%') {
        return p.parse::<f32>().ok().map(|n| n / 100.0);
    }
    v.parse::<f32>().ok().map(|n| n / 100_000.0)
}

/// A colour with opacity (0–1).
pub type Col = (Rgb, f32);

/// The theme's formatting scheme (fills, lines, backgrounds) that `p:style` refers to.
#[derive(Debug, Default, Clone)]
pub struct Fmt {
    pub fills: Vec<El>,
    pub lines: Vec<El>,
    pub bg_fills: Vec<El>,
}

impl Fmt {
    pub fn parse(theme: &El) -> Fmt {
        let Some(fs) = theme
            .child("themeElements")
            .and_then(|t| t.child("fmtScheme"))
        else {
            return Fmt::default();
        };
        let list = |name: &str| -> Vec<El> {
            fs.child(name)
                .map(|l| l.children().cloned().collect())
                .unwrap_or_default()
        };
        Fmt {
            fills: list("fillStyleLst"),
            lines: list("lnStyleLst"),
            bg_fills: list("bgFillStyleLst"),
        }
    }
}

/// Theme, formatting scheme and colour map for one slide.
#[derive(Debug, Clone)]
pub struct Colors {
    pub theme: Theme,
    pub fmt: Fmt,
    /// Scheme name as used on the slide (`tx1`, `bg2`…) → theme slot (`dk1`, `lt2`…).
    pub map: HashMap<String, String>,
}

impl Colors {
    pub fn with_map(&self, clr_map: Option<&El>) -> Colors {
        let mut c = self.clone();
        if let Some(m) = clr_map {
            for (k, v) in &m.attrs {
                c.map.insert(k.to_string(), v.to_string());
            }
        }
        c
    }

    fn scheme(&self, name: &str) -> Option<Rgb> {
        let slot = self
            .map
            .get(name)
            .map(String::as_str)
            .unwrap_or(match name {
                "tx1" => "dk1",
                "bg1" => "lt1",
                "tx2" => "dk2",
                "bg2" => "lt2",
                other => other,
            });
        self.theme.color(slot)
    }

    /// A colour element (`srgbClr`, `schemeClr`, …) with its modifiers. `ph` is what
    /// `phClr` stands for (the colour a style reference passes in).
    pub fn color(&self, e: &El, ph: Option<Col>) -> Option<Col> {
        let (base, mut alpha) = match &*e.name {
            "srgbClr" => (Rgb::hex(e.val()?)?, 1.0),
            "schemeClr" => match e.val()? {
                "phClr" => ph.unwrap_or((Rgb(0, 0, 0), 1.0)),
                n => (self.scheme(n)?, 1.0),
            },
            "sysClr" => (
                e.attr("lastClr")
                    .and_then(Rgb::hex)
                    .unwrap_or(match e.val() {
                        Some("window") => Rgb(255, 255, 255),
                        _ => Rgb(0, 0, 0),
                    }),
                1.0,
            ),
            "prstClr" => (preset(e.val()?)?, 1.0),
            "scrgbClr" => {
                let lin = |k: &str| {
                    let v = e.attr(k).and_then(pct).unwrap_or(0.0).clamp(0.0, 1.0);
                    let s = if v <= 0.003_130_8 {
                        v * 12.92
                    } else {
                        1.055 * v.powf(1.0 / 2.4) - 0.055
                    };
                    (s * 255.0).round() as u8
                };
                (Rgb(lin("r"), lin("g"), lin("b")), 1.0)
            }
            "hslClr" => {
                let h = e
                    .attr("hue")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0)
                    / 21_600_000.0;
                let s = e.attr("sat").and_then(pct).unwrap_or(0.0);
                let l = e.attr("lum").and_then(pct).unwrap_or(0.0);
                (from_hsl(h, s, l), 1.0)
            }
            _ => return None,
        };
        let mut c = base;
        let mut lum_mod = None;
        let mut lum_off = None;
        let mut sat_mod = None;
        for m in e.children() {
            let v = m.val().and_then(pct).unwrap_or(1.0);
            match &*m.name {
                "alpha" => alpha = v,
                "alphaMod" => alpha *= v,
                "alphaOff" => alpha += v,
                "lumMod" => lum_mod = Some(v),
                "lumOff" => lum_off = Some(v),
                "satMod" => sat_mod = Some(v),
                // PowerPoint tints and shades in linear light (scRGB), which keeps
                // colours more saturated than doing it on sRGB values.
                "tint" => c = linear(c, |x| x * v + (1.0 - v)),
                "shade" => c = linear(c, |x| x * v),
                "inv" => c = Rgb(255 - c.0, 255 - c.1, 255 - c.2),
                "gray" => {
                    let g = (0.3 * c.0 as f32 + 0.59 * c.1 as f32 + 0.11 * c.2 as f32) as u8;
                    c = Rgb(g, g, g);
                }
                _ => {}
            }
        }
        if lum_mod.is_some() || lum_off.is_some() || sat_mod.is_some() {
            let (h, s, l) = hsl(c);
            let l = (l * lum_mod.unwrap_or(1.0) + lum_off.unwrap_or(0.0)).clamp(0.0, 1.0);
            let s = (s * sat_mod.unwrap_or(1.0)).clamp(0.0, 1.0);
            c = from_hsl(h, s, l);
        }
        Some((c, alpha.clamp(0.0, 1.0)))
    }

    /// The first colour child of `e`.
    pub fn color_in(&self, e: &El, ph: Option<Col>) -> Option<Col> {
        e.children().find_map(|c| self.color(c, ph))
    }
}

fn linear(c: Rgb, f: impl Fn(f32) -> f32) -> Rgb {
    let to_lin = |v: u8| {
        let s = v as f32 / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |l: f32| {
        let l = l.clamp(0.0, 1.0);
        let s = if l <= 0.003_130_8 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0).round() as u8
    };
    Rgb(
        to_srgb(f(to_lin(c.0))),
        to_srgb(f(to_lin(c.1))),
        to_srgb(f(to_lin(c.2))),
    )
}

fn preset(n: &str) -> Option<Rgb> {
    Some(match n {
        "black" => Rgb(0, 0, 0),
        "white" => Rgb(255, 255, 255),
        "red" => Rgb(255, 0, 0),
        "green" => Rgb(0, 128, 0),
        "lime" => Rgb(0, 255, 0),
        "blue" => Rgb(0, 0, 255),
        "yellow" => Rgb(255, 255, 0),
        "orange" => Rgb(255, 165, 0),
        "gray" | "grey" => Rgb(128, 128, 128),
        "darkGray" => Rgb(169, 169, 169),
        "lightGray" => Rgb(211, 211, 211),
        "navy" => Rgb(0, 0, 128),
        "purple" => Rgb(128, 0, 128),
        "maroon" => Rgb(128, 0, 0),
        "teal" => Rgb(0, 128, 128),
        "silver" => Rgb(192, 192, 192),
        _ => return None,
    })
}

// ------------------------------------------------------------ fills and lines

/// A fill as written: nothing said, explicitly none, or a paint.
#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    None,
    Solid(Col),
    /// Stops (position 0–1, colour) and angle in degrees (clockwise from +x), or a
    /// radial gradient from the centre.
    Gradient {
        stops: Vec<(f32, Col)>,
        angle: f32,
        radial: bool,
    },
    /// A picture fill (relationship id), stretched over the shape.
    Picture(String),
    /// Use the enclosing group's fill.
    Group,
}

impl Fill {
    /// Paint for a box of `w` × `h` (gradients run across it).
    pub fn paint(&self, w: f32, h: f32) -> Option<Paint> {
        match self {
            Fill::Solid((c, a)) => Some(Paint::Solid(*c, *a)),
            Fill::Gradient {
                stops,
                angle,
                radial,
            } => {
                let st: Vec<(f32, Rgb, f32)> =
                    stops.iter().map(|(o, (c, a))| (*o, *c, *a)).collect();
                if st.is_empty() {
                    return None;
                }
                if *radial {
                    return Some(Paint::Radial {
                        center: (w / 2.0, h / 2.0),
                        radius: (w * w + h * h).sqrt() / 2.0,
                        stops: st,
                    });
                }
                let r = angle.to_radians();
                let (dx, dy) = (r.cos(), r.sin());
                let half = ((w * dx).abs() + (h * dy).abs()) / 2.0;
                let (cx, cy) = (w / 2.0, h / 2.0);
                Some(Paint::Linear {
                    from: (cx - dx * half, cy - dy * half),
                    to: (cx + dx * half, cy + dy * half),
                    stops: st,
                })
            }
            _ => None,
        }
    }
}

/// The fill among `e`'s children (`spPr`, `bgPr`, `tcPr`…), if any is given.
pub fn fill_in(c: &Colors, e: &El, ph: Option<Col>) -> Option<Fill> {
    e.children().find_map(|f| fill(c, f, ph))
}

/// A fill element itself.
pub fn fill(c: &Colors, f: &El, ph: Option<Col>) -> Option<Fill> {
    Some(match &*f.name {
        "noFill" => Fill::None,
        "solidFill" => Fill::Solid(c.color_in(f, ph)?),
        "gradFill" => {
            let mut stops: Vec<(f32, Col)> = f
                .child("gsLst")
                .map(|l| {
                    l.all("gs")
                        .filter_map(|g| {
                            Some((
                                g.attr("pos").and_then(pct).unwrap_or(0.0),
                                c.color_in(g, ph)?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            let angle = f
                .child("lin")
                .and_then(|l| l.attr("ang"))
                .and_then(|a| a.parse::<f32>().ok())
                .map_or(90.0, |a| a / 60_000.0);
            let radial = f.child("path").is_some();
            Fill::Gradient {
                stops,
                angle,
                radial,
            }
        }
        "blipFill" => Fill::Picture(f.child("blip")?.attr("embed")?.to_string()),
        "pattFill" => Fill::Solid(
            f.child("fgClr")
                .and_then(|x| c.color_in(x, ph))
                .unwrap_or((Rgb(0, 0, 0), 1.0)),
        ),
        "grpFill" => Fill::Group,
        _ => return None,
    })
}

/// A line as written; fields are None when not given.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Line {
    pub width: Option<f32>,
    pub fill: Option<Fill>,
    pub dash: Option<Vec<f32>>,
    pub round: Option<bool>,
    pub head: Option<String>,
    pub tail: Option<String>,
}

impl Line {
    pub fn parse(c: &Colors, ln: &El, ph: Option<Col>) -> Line {
        let dash = ln.child("prstDash").and_then(|d| d.val()).map(|v| match v {
            "dot" | "sysDot" => vec![1.0, 2.0],
            "dash" | "sysDash" => vec![4.0, 3.0],
            "lgDash" => vec![8.0, 3.0],
            "dashDot" | "sysDashDot" => vec![4.0, 3.0, 1.0, 3.0],
            "lgDashDot" => vec![8.0, 3.0, 1.0, 3.0],
            "lgDashDotDot" | "sysDashDotDot" => vec![8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
            _ => Vec::new(),
        });
        let end = |k: &str| {
            ln.child(k)
                .and_then(|e| e.attr("type"))
                .filter(|t| *t != "none")
                .map(str::to_string)
        };
        Line {
            width: attr_emu(ln, "w"),
            fill: fill_in(c, ln, ph),
            dash,
            round: ln
                .child("round")
                .map(|_| true)
                .or_else(|| ln.attr("cap").map(|v| v == "rnd")),
            head: end("headEnd"),
            tail: end("tailEnd"),
        }
    }

    /// Later values win.
    pub fn over(mut self, o: &Line) -> Line {
        if o.width.is_some() {
            self.width = o.width;
        }
        if o.fill.is_some() {
            self.fill = o.fill.clone();
        }
        if o.dash.is_some() {
            self.dash = o.dash.clone();
        }
        if o.round.is_some() {
            self.round = o.round;
        }
        if o.head.is_some() {
            self.head = o.head.clone();
        }
        if o.tail.is_some() {
            self.tail = o.tail.clone();
        }
        self
    }

    pub fn pen(&self) -> Option<Pen> {
        let (color, alpha) = match self.fill.as_ref()? {
            Fill::Solid(c) => *c,
            Fill::Gradient { stops, .. } => stops.first()?.1,
            _ => return None,
        };
        Some(Pen {
            color,
            alpha,
            // An unspecified width is PowerPoint's default line, 0.75 pt.
            width: self.width.unwrap_or(0.75).max(0.25),
            dash: self.dash.clone().unwrap_or_default(),
            round: self.round.unwrap_or(false),
        })
    }
}

// ------------------------------------------------------------ text styles

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caps {
    None,
    All,
    Small,
}

/// Run properties as written (None = inherit).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RProps {
    pub sz: Option<f32>,
    pub b: Option<bool>,
    pub i: Option<bool>,
    pub u: Option<Underline>,
    pub strike: Option<(bool, bool)>,
    pub baseline: Option<f32>,
    pub caps: Option<Caps>,
    pub spc: Option<f32>,
    pub color: Option<Col>,
    pub latin: Option<String>,
    pub ea: Option<String>,
    pub cs: Option<String>,
    pub highlight: Option<Rgb>,
    pub link: Option<String>,
}

fn on(e: &El, k: &str) -> Option<bool> {
    e.attr(k).map(|v| v == "1" || v == "true")
}

impl RProps {
    pub fn parse(c: &Colors, e: &El) -> RProps {
        let face = |k: &str| {
            e.child(k)
                .and_then(|f| f.attr("typeface"))
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        };
        RProps {
            sz: e
                .attr("sz")
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v / 100.0),
            b: on(e, "b"),
            i: on(e, "i"),
            u: e.attr("u").map(|u| match u {
                "none" => Underline::None,
                "dbl" => Underline::Double,
                "heavy" => Underline::Thick,
                "dotted" | "dottedHeavy" => Underline::Dotted,
                "dash" | "dashHeavy" | "dashLong" | "dashLongHeavy" | "dotDash" => Underline::Dash,
                "wavy" | "wavyHeavy" | "wavyDbl" => Underline::Wave,
                "words" => Underline::Words,
                _ => Underline::Single,
            }),
            strike: e.attr("strike").map(|s| match s {
                "sngStrike" => (true, false),
                "dblStrike" => (false, true),
                _ => (false, false),
            }),
            baseline: e.attr("baseline").and_then(pct),
            caps: e.attr("cap").map(|v| match v {
                "all" => Caps::All,
                "small" => Caps::Small,
                _ => Caps::None,
            }),
            spc: e
                .attr("spc")
                .and_then(|v| v.parse::<f32>().ok())
                .map(|v| v / 100.0),
            color: e
                .child("solidFill")
                .and_then(|f| c.color_in(f, None))
                .or_else(|| {
                    // A gradient fill on text: its first stop.
                    e.child("gradFill")
                        .and_then(|g| g.child("gsLst"))
                        .and_then(|l| l.child("gs"))
                        .and_then(|g| c.color_in(g, None))
                }),
            latin: face("latin"),
            ea: face("ea"),
            cs: face("cs"),
            highlight: e
                .child("highlight")
                .and_then(|h| c.color_in(h, None))
                .map(|c| c.0),
            link: e
                .child("hlinkClick")
                .and_then(|h| h.attr("id"))
                .map(str::to_string),
        }
    }

    pub fn over(&mut self, o: &RProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            sz, b, i, u, strike, baseline, caps, spc, color, latin, ea, cs, highlight, link
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Bullet {
    None,
    Char(String),
    /// Auto-numbering scheme (`arabicPeriod`…) and start number.
    Auto(String, u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spacing {
    /// A multiple of single line spacing.
    Pct(f32),
    Pts(f32),
}

/// Paragraph properties as written (None = inherit), with the level's default run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PProps {
    pub mar_l: Option<f32>,
    pub mar_r: Option<f32>,
    pub indent: Option<f32>,
    pub algn: Option<Align>,
    pub ln_spc: Option<Spacing>,
    pub spc_bef: Option<Spacing>,
    pub spc_aft: Option<Spacing>,
    pub bullet: Option<Bullet>,
    pub bu_font: Option<String>,
    pub bu_clr: Option<Col>,
    pub bu_sz: Option<f32>,
    pub rtl: Option<bool>,
    pub tabs: Option<Vec<f32>>,
    pub def_tab: Option<f32>,
    pub def_r: RProps,
}

fn spacing(e: Option<&El>) -> Option<Spacing> {
    let e = e?;
    if let Some(p) = e.child("spcPct") {
        return p.val().and_then(pct).map(Spacing::Pct);
    }
    e.child("spcPts")
        .and_then(|p| p.val())
        .and_then(|v| v.parse::<f32>().ok())
        .map(|v| Spacing::Pts(v / 100.0))
}

impl PProps {
    pub fn parse(c: &Colors, e: &El) -> PProps {
        let bullet = if e.child("buNone").is_some() {
            Some(Bullet::None)
        } else if let Some(b) = e.child("buChar") {
            Some(Bullet::Char(b.attr("char").unwrap_or("•").to_string()))
        } else {
            e.child("buAutoNum").map(|b| {
                Bullet::Auto(
                    b.attr("type").unwrap_or("arabicPeriod").to_string(),
                    b.attr("startAt").and_then(|v| v.parse().ok()).unwrap_or(1),
                )
            })
        };
        PProps {
            mar_l: attr_emu(e, "marL"),
            mar_r: attr_emu(e, "marR"),
            indent: attr_emu(e, "indent"),
            algn: e.attr("algn").map(|a| match a {
                "ctr" => Align::Center,
                "r" => Align::Right,
                "just" | "justLow" => Align::Both,
                "dist" | "thaiDist" => Align::Distribute,
                _ => Align::Left,
            }),
            ln_spc: spacing(e.child("lnSpc")),
            spc_bef: spacing(e.child("spcBef")),
            spc_aft: spacing(e.child("spcAft")),
            bullet,
            bu_font: e
                .child("buFont")
                .and_then(|f| f.attr("typeface"))
                .map(str::to_string),
            bu_clr: e.child("buClr").and_then(|b| c.color_in(b, None)),
            bu_sz: e.child("buSzPct").and_then(|b| b.val()).and_then(pct),
            rtl: on(e, "rtl"),
            tabs: e
                .child("tabLst")
                .map(|t| t.all("tab").filter_map(|x| attr_emu(x, "pos")).collect()),
            def_tab: attr_emu(e, "defTabSz"),
            def_r: e
                .child("defRPr")
                .map(|r| RProps::parse(c, r))
                .unwrap_or_default(),
        }
    }

    pub fn over(&mut self, o: &PProps) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(
            mar_l, mar_r, indent, algn, ln_spc, spc_bef, spc_aft, bullet, bu_font, bu_clr, bu_sz,
            rtl, tabs, def_tab
        );
        self.def_r.over(&o.def_r);
    }
}

/// A list style: properties per outline level (0–8).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Levels(pub [PProps; 9]);

impl Levels {
    /// `a:lstStyle`, `p:titleStyle`, `a:defaultTextStyle`…: `defPPr` then `lvlNpPr`.
    pub fn parse(c: &Colors, e: &El) -> Levels {
        let mut l = Levels::default();
        let def = e.child("defPPr").map(|d| PProps::parse(c, d));
        for (i, lvl) in l.0.iter_mut().enumerate() {
            if let Some(d) = &def {
                lvl.over(d);
            }
            if let Some(p) = e.child(&format!("lvl{}pPr", i + 1)) {
                lvl.over(&PProps::parse(c, p));
            }
        }
        l
    }

    pub fn over(&mut self, o: &Levels) {
        for i in 0..9 {
            self.0[i].over(&o.0[i]);
        }
    }
}

/// `a:bodyPr` as written.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Body {
    pub ins: [Option<f32>; 4],
    pub anchor: Option<String>,
    pub wrap: Option<bool>,
    pub font_scale: Option<f32>,
    pub ln_reduce: Option<f32>,
    pub vert: Option<String>,
    pub rot: Option<f32>,
}

impl Body {
    pub fn parse(e: &El) -> Body {
        let auto = e.child("normAutofit");
        Body {
            ins: [
                attr_emu(e, "lIns"),
                attr_emu(e, "tIns"),
                attr_emu(e, "rIns"),
                attr_emu(e, "bIns"),
            ],
            anchor: e.attr("anchor").map(str::to_string),
            wrap: e.attr("wrap").map(|w| w != "none"),
            font_scale: auto.and_then(|a| a.attr("fontScale")).and_then(pct),
            ln_reduce: auto.and_then(|a| a.attr("lnSpcReduction")).and_then(pct),
            vert: e.attr("vert").map(str::to_string),
            rot: e
                .attr("rot")
                .and_then(|r| r.parse::<f32>().ok())
                .map(|r| r / 60_000.0),
        }
    }

    pub fn over(&mut self, o: &Body) {
        for i in 0..4 {
            if o.ins[i].is_some() {
                self.ins[i] = o.ins[i];
            }
        }
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f.clone(); } )* };
        }
        take!(anchor, wrap, font_scale, ln_reduce, vert, rot);
    }
}
