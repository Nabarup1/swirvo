//! Reading a presentation: slides in order, each resolved against its layout and master
//! into positioned items (shapes with text, pictures, tables) ready to draw.

use std::collections::HashMap;
use std::sync::Arc;

use swirvo_core::EngineError;

use super::geom;
use super::style::{
    Body, Bullet, Caps, Col, Colors, Fill, Fmt, Levels, Line, PProps, RProps, Spacing, attr_emu,
};
use crate::docxlayout::layout::{Paint, PathCmd, Pen};
use crate::docxlayout::model::{
    Block, Cell, Item as RunItem, Label, Link, Media, Meta, Para, ParaStyle, Row, RunStyle, Table,
};
use crate::docxlayout::numbering::Suffix;
use crate::docxlayout::props::{
    Align, Border, BorderStyle, Leader, LineRule, Rgb, TabDef, TabKind, Underline, VAlign, VMerge,
    Vert, Width,
};
use crate::docxlayout::theme::Theme;
use crate::docxlayout::xml::El;
use crate::opc::{Package, Rels};

pub struct Deck {
    pub w: f32,
    pub h: f32,
    pub slides: Vec<Slide>,
    pub meta: Meta,
}

pub struct Slide {
    /// The title placeholder's text (for the PDF outline).
    pub title: Option<String>,
    pub bg: Option<Paint>,
    pub bg_pic: Option<Arc<Media>>,
    pub items: Vec<Item>,
}

/// Where an item sits: its box on the slide, turned `rot` degrees clockwise about its
/// centre and optionally mirrored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

pub struct Item {
    pub at: Place,
    pub kind: Kind,
    /// Alternative text (the shape's description, else its title).
    pub alt: Option<String>,
    /// The slide's title placeholder.
    pub title: bool,
}

pub struct Text {
    pub blocks: Vec<Block>,
    pub ins: [f32; 4],
    pub anchor: VAlign,
    pub wrap: bool,
    /// Vertical text (`vert`, `vert270`…): the box is laid out turned.
    pub vert: Option<String>,
}

pub enum Kind {
    Shape {
        path: Vec<PathCmd>,
        open: bool,
        fill: Option<Paint>,
        pic: Option<Arc<Media>>,
        pen: Option<Pen>,
        head: Option<String>,
        tail: Option<String>,
        text: Option<Text>,
    },
    Picture {
        media: Arc<Media>,
        crop: [f32; 4],
        pen: Option<Pen>,
    },
    Table(Box<Table>),
    Chart(Box<crate::chart::Chart>),
}

/// A placeholder on a layout or master: what slide placeholders inherit.
#[derive(Debug, Clone, Default)]
struct Ph {
    kind: String,
    idx: Option<String>,
    xfrm: Option<Xfrm>,
    body: Body,
    lst: Levels,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Xfrm {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rot: f32,
    flip_h: bool,
    flip_v: bool,
    ch: Option<(f32, f32, f32, f32)>,
}

fn xfrm(e: Option<&El>) -> Option<Xfrm> {
    let e = e?;
    let off = e.child("off");
    let ext = e.child("ext")?;
    let chs = match (e.child("chOff"), e.child("chExt")) {
        (Some(o), Some(x)) => Some((
            attr_emu(o, "x").unwrap_or(0.0),
            attr_emu(o, "y").unwrap_or(0.0),
            attr_emu(x, "cx").unwrap_or(0.0),
            attr_emu(x, "cy").unwrap_or(0.0),
        )),
        _ => None,
    };
    Some(Xfrm {
        x: off.and_then(|o| attr_emu(o, "x")).unwrap_or(0.0),
        y: off.and_then(|o| attr_emu(o, "y")).unwrap_or(0.0),
        w: attr_emu(ext, "cx").unwrap_or(0.0),
        h: attr_emu(ext, "cy").unwrap_or(0.0),
        rot: e
            .attr("rot")
            .and_then(|r| r.parse::<f32>().ok())
            .unwrap_or(0.0)
            / 60_000.0,
        flip_h: e.attr("flipH") == Some("1"),
        flip_v: e.attr("flipV") == Some("1"),
        ch: chs,
    })
}

/// Maps child coordinates of a group into its parent's space.
#[derive(Debug, Clone, Copy)]
struct GroupMap {
    sx: f32,
    sy: f32,
    dx: f32,
    dy: f32,
    /// The group's own rotation and flips, applied about its centre.
    rot: f32,
    flip_h: bool,
    flip_v: bool,
    cx: f32,
    cy: f32,
}

impl GroupMap {
    fn identity() -> GroupMap {
        GroupMap {
            sx: 1.0,
            sy: 1.0,
            dx: 0.0,
            dy: 0.0,
            rot: 0.0,
            flip_h: false,
            flip_v: false,
            cx: 0.0,
            cy: 0.0,
        }
    }

    fn place(&self, x: &Xfrm) -> Place {
        let mut p = Place {
            x: self.dx + x.x * self.sx,
            y: self.dy + x.y * self.sy,
            w: x.w * self.sx,
            h: x.h * self.sy,
            rot: x.rot,
            flip_h: x.flip_h,
            flip_v: x.flip_v,
        };
        if self.flip_h || self.flip_v || self.rot != 0.0 {
            // Move the child's centre with the group, and turn it with the group.
            let (mut cx, mut cy) = (p.x + p.w / 2.0 - self.cx, p.y + p.h / 2.0 - self.cy);
            if self.flip_h {
                cx = -cx;
                p.flip_h = !p.flip_h;
                p.rot = -p.rot;
            }
            if self.flip_v {
                cy = -cy;
                p.flip_v = !p.flip_v;
                p.rot = -p.rot;
            }
            let r = self.rot.to_radians();
            let (nx, ny) = (cx * r.cos() - cy * r.sin(), cx * r.sin() + cy * r.cos());
            p.x = self.cx + nx - p.w / 2.0;
            p.y = self.cy + ny - p.h / 2.0;
            p.rot += self.rot;
        }
        p
    }

    /// The map for a group with this `xfrm`, nested in `self`.
    fn child(&self, x: &Xfrm) -> GroupMap {
        let outer = self.place(x);
        let (chx, chy, chw, chh) = x.ch.unwrap_or((x.x, x.y, x.w, x.h));
        let sx = if chw > 0.0 { outer.w / chw } else { 1.0 };
        let sy = if chh > 0.0 { outer.h / chh } else { 1.0 };
        GroupMap {
            sx,
            sy,
            dx: outer.x - chx * sx,
            dy: outer.y - chy * sy,
            rot: outer.rot,
            flip_h: outer.flip_h,
            flip_v: outer.flip_v,
            cx: outer.x + outer.w / 2.0,
            cy: outer.y + outer.h / 2.0,
        }
    }
}

/// A master or layout, read once.
struct Base {
    colors: Colors,
    tree: El,
    rels: Rels,
    bg: Option<El>,
    phs: Vec<Ph>,
    show_master: bool,
    /// Master only: title, body and other text styles.
    styles: Option<[Levels; 3]>,
}

struct Reader<'a> {
    pkg: Package<'a>,
    masters: HashMap<String, Arc<Base>>,
    layouts: HashMap<String, (Arc<Base>, String)>,
    media: HashMap<String, Option<Arc<Media>>>,
    table_styles: HashMap<String, El>,
    default_text: Levels,
    default_text_el: Option<El>,
}

/// Placeholder families for matching and for picking the master text style.
fn ph_family(kind: &str) -> &str {
    match kind {
        "title" | "ctrTitle" => "title",
        "body" | "subTitle" | "obj" | "" => "body",
        other => other,
    }
}

fn ph_of(sp: &El) -> Option<(String, Option<String>)> {
    let ph = sp
        .children()
        .find(|c| c.name.starts_with("nv"))?
        .child("nvPr")?
        .child("ph")?;
    Some((
        ph.attr("type").unwrap_or("obj").to_string(),
        ph.attr("idx").map(str::to_string),
    ))
}

/// A shape's alternative text: `descr`, else `title`, on its non-visual properties.
fn alt_of(sp: &El) -> Option<String> {
    let c = sp
        .children()
        .find(|c| c.name.starts_with("nv"))?
        .child("cNvPr")?;
    ["descr", "title"]
        .iter()
        .filter_map(|k| c.attr(k))
        .map(str::trim)
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

fn hidden(sp: &El) -> bool {
    sp.children()
        .find(|c| c.name.starts_with("nv"))
        .and_then(|n| n.child("cNvPr"))
        .is_some_and(|c| c.attr("hidden") == Some("1"))
}

impl<'a> Reader<'a> {
    fn base_colors(&mut self, part: &str, rels: &Rels) -> Colors {
        let theme_el = rels
            .of_type("/theme")
            .map(|r| r.target.clone())
            .and_then(|t| self.pkg.xml(&t));
        let theme = theme_el.as_ref().map(Theme::parse).unwrap_or_default();
        let fmt = theme_el.as_ref().map(Fmt::parse).unwrap_or_default();
        let _ = part;
        Colors {
            theme,
            fmt,
            map: HashMap::new(),
        }
    }

    fn master(&mut self, path: &str) -> Arc<Base> {
        if let Some(m) = self.masters.get(path) {
            return m.clone();
        }
        let root = self.pkg.xml(path).unwrap_or_default();
        let rels = self.pkg.rels(path);
        let colors = self.base_colors(path, &rels).with_map(root.child("clrMap"));
        let styles = root.child("txStyles").map(|t| {
            let get = |n: &str| {
                t.child(n)
                    .map(|e| Levels::parse(&colors, e))
                    .unwrap_or_default()
            };
            [get("titleStyle"), get("bodyStyle"), get("otherStyle")]
        });
        let csld = root.child("cSld");
        let tree = csld
            .and_then(|c| c.child("spTree"))
            .cloned()
            .unwrap_or_default();
        let phs = collect_phs(&colors, &tree);
        let b = Arc::new(Base {
            bg: csld.and_then(|c| c.child("bg")).cloned(),
            colors,
            tree,
            rels,
            phs,
            show_master: true,
            styles,
        });
        self.masters.insert(path.to_string(), b.clone());
        b
    }

    fn layout(&mut self, path: &str) -> (Arc<Base>, Arc<Base>) {
        if let Some((l, m)) = self.layouts.get(path).cloned() {
            let master = self.master(&m);
            return (l, master);
        }
        let root = self.pkg.xml(path).unwrap_or_default();
        let rels = self.pkg.rels(path);
        let mpath = rels
            .of_type("/slideMaster")
            .map(|r| r.target.clone())
            .unwrap_or_default();
        let master = self.master(&mpath);
        let colors = master.colors.with_map(
            root.child("clrMapOvr")
                .and_then(|o| o.child("overrideClrMapping")),
        );
        let csld = root.child("cSld");
        let tree = csld
            .and_then(|c| c.child("spTree"))
            .cloned()
            .unwrap_or_default();
        let phs = collect_phs(&colors, &tree);
        let l = Arc::new(Base {
            bg: csld.and_then(|c| c.child("bg")).cloned(),
            colors,
            tree,
            rels,
            phs,
            show_master: root.attr("showMasterSp") != Some("0"),
            styles: None,
        });
        self.layouts.insert(path.to_string(), (l.clone(), mpath));
        (l, master)
    }

    fn media(&mut self, rels: &Rels, id: &str) -> Option<Arc<Media>> {
        let target = rels.target(id)?.to_string();
        if let Some(m) = self.media.get(&target) {
            return m.clone();
        }
        let m = self
            .pkg
            .bytes(&target)
            .map(|b| Arc::new(Media { bytes: Arc::new(b) }));
        // Raster pictures and metafiles (EMF/WMF, drawn as vectors).
        let m = m.filter(|m| crate::emf::drawable(&m.bytes));
        self.media.insert(target, m.clone());
        m
    }
}

fn collect_phs(colors: &Colors, tree: &El) -> Vec<Ph> {
    let mut out = Vec::new();
    for sp in tree.children() {
        let Some((kind, idx)) = ph_of(sp) else {
            continue;
        };
        let sppr = sp.child("spPr");
        let tx = sp.child("txBody");
        out.push(Ph {
            kind,
            idx,
            xfrm: xfrm(sppr.and_then(|s| s.child("xfrm"))),
            body: tx
                .and_then(|t| t.child("bodyPr"))
                .map(Body::parse)
                .unwrap_or_default(),
            lst: tx
                .and_then(|t| t.child("lstStyle"))
                .map(|l| Levels::parse(colors, l))
                .unwrap_or_default(),
        });
    }
    out
}

fn find_ph<'p>(phs: &'p [Ph], kind: &str, idx: Option<&str>) -> Option<&'p Ph> {
    // By index first (layouts number their placeholders), then by family.
    idx.and_then(|i| phs.iter().find(|p| p.idx.as_deref() == Some(i)))
        .or_else(|| {
            phs.iter()
                .find(|p| ph_family(&p.kind) == ph_family(kind) && p.kind == kind)
        })
        .or_else(|| phs.iter().find(|p| ph_family(&p.kind) == ph_family(kind)))
}

/// One slide being read: its parts and inheritance chain.
struct Ctx<'x> {
    colors: &'x Colors,
    rels: &'x Rels,
    layout: Option<&'x Base>,
    master: Option<&'x Base>,
    number: usize,
    default_tab: f32,
}

pub fn read(bytes: &[u8]) -> Result<Deck, EngineError> {
    let mut pkg = Package::open(bytes)?;
    let main = pkg.main_part("ppt/presentation.xml");
    let pres = pkg.xml(&main).ok_or_else(|| {
        EngineError::Decode("not a PowerPoint file (no presentation part)".into())
    })?;
    let rels = pkg.rels(&main);
    let size = pres.child("sldSz");
    let w = size.and_then(|s| attr_emu(s, "cx")).unwrap_or(720.0);
    let h = size.and_then(|s| attr_emu(s, "cy")).unwrap_or(540.0);
    let table_styles = rels
        .of_type("/tableStyles")
        .map(|r| r.target.clone())
        .and_then(|t| pkg.xml(&t))
        .map(|t| {
            t.all("tblStyle")
                .filter_map(|s| Some((s.attr("styleId")?.to_string(), s.clone())))
                .collect()
        })
        .unwrap_or_default();
    let mut r = Reader {
        pkg,
        masters: HashMap::new(),
        layouts: HashMap::new(),
        media: HashMap::new(),
        table_styles,
        default_text: Levels::default(),
        default_text_el: pres.child("defaultTextStyle").cloned(),
    };
    // `r:id` is stored under its local name `id`; the slide id is the other `id`.
    let slide_parts: Vec<String> = pres
        .child("sldIdLst")
        .map(|l| {
            l.all("sldId")
                .filter_map(|s| {
                    s.attrs
                        .iter()
                        .filter(|(k, _)| &**k == "id")
                        .filter_map(|(_, v)| rels.target(v))
                        .next()
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut slides = Vec::new();
    for (n, part) in slide_parts.iter().enumerate() {
        let Some(root) = r.pkg.xml(part) else {
            continue;
        };
        if root.attr("show") == Some("0") {
            continue; // Hidden slides aren't exported, as in PowerPoint.
        }
        slides.push(read_slide(&mut r, part, &root, n + 1)?);
    }
    let meta = read_meta(&mut r.pkg);
    if slides.is_empty() {
        return Err(EngineError::Decode(
            "this presentation has no slides".into(),
        ));
    }
    Ok(Deck { w, h, slides, meta })
}

fn read_meta(pkg: &mut Package<'_>) -> Meta {
    let Some(core) = pkg.xml("docProps/core.xml") else {
        return Meta::default();
    };
    let get = |k: &str| {
        core.child(k)
            .map(|e| e.text().trim().to_string())
            .filter(|s| !s.is_empty())
    };
    Meta {
        title: get("title"),
        author: get("creator"),
        subject: get("subject"),
        keywords: get("keywords"),
        language: get("language"),
    }
}

fn read_slide(
    r: &mut Reader<'_>,
    part: &str,
    root: &El,
    number: usize,
) -> Result<Slide, EngineError> {
    let rels = r.pkg.rels(part);
    let lpath = rels
        .of_type("/slideLayout")
        .map(|x| x.target.clone())
        .unwrap_or_default();
    let (layout, master) = r.layout(&lpath);
    let colors = layout.colors.with_map(
        root.child("clrMapOvr")
            .and_then(|o| o.child("overrideClrMapping")),
    );
    if let Some(d) = r.default_text_el.clone() {
        r.default_text = Levels::parse(&colors, &d);
    }
    let csld = root.child("cSld");
    // Background: slide, then layout, then master.
    let (bg_el, bg_rels, bg_colors) = match csld.and_then(|c| c.child("bg")) {
        Some(b) => (Some(b.clone()), rels.clone(), colors.clone()),
        None => match &layout.bg {
            Some(b) => (Some(b.clone()), layout.rels.clone(), layout.colors.clone()),
            None => (
                master.bg.clone(),
                master.rels.clone(),
                master.colors.clone(),
            ),
        },
    };
    let (bg, bg_pic) = background(r, bg_el.as_ref(), &bg_rels, &bg_colors);
    let mut items = Vec::new();
    let show_master = root.attr("showMasterSp") != Some("0");
    if show_master && layout.show_master {
        let ctx = Ctx {
            colors: &master.colors,
            rels: &master.rels,
            layout: None,
            master: Some(&master),
            number,
            default_tab: 72.0,
        };
        walk(
            r,
            &ctx,
            &master.tree,
            GroupMap::identity(),
            false,
            &mut items,
        );
    }
    if show_master {
        let ctx = Ctx {
            colors: &layout.colors,
            rels: &layout.rels,
            layout: None,
            master: Some(&master),
            number,
            default_tab: 72.0,
        };
        walk(
            r,
            &ctx,
            &layout.tree,
            GroupMap::identity(),
            false,
            &mut items,
        );
    }
    if let Some(tree) = csld.and_then(|c| c.child("spTree")) {
        let ctx = Ctx {
            colors: &colors,
            rels: &rels,
            layout: Some(&layout),
            master: Some(&master),
            number,
            default_tab: 72.0,
        };
        walk(r, &ctx, tree, GroupMap::identity(), true, &mut items);
    }
    let title = csld.and_then(|c| c.child("spTree")).and_then(|t| {
        t.children()
            .filter(|sp| ph_of(sp).is_some_and(|(k, _)| ph_family(&k) == "title"))
            .find_map(|sp| {
                let txt: Vec<String> = sp.child("txBody")?.all("p").map(El::text).collect();
                let t = txt
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                (!t.is_empty()).then_some(t)
            })
    });
    Ok(Slide {
        title,
        bg,
        bg_pic,
        items,
    })
}

fn background(
    r: &mut Reader<'_>,
    bg: Option<&El>,
    rels: &Rels,
    colors: &Colors,
) -> (Option<Paint>, Option<Arc<Media>>) {
    let Some(bg) = bg else {
        return (None, None);
    };
    let fill = if let Some(pr) = bg.child("bgPr") {
        super::style::fill_in(colors, pr, None)
    } else if let Some(rf) = bg.child("bgRef") {
        let idx = rf
            .attr("idx")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let ph = colors.color_in(rf, None);
        let list = if idx >= 1001 {
            colors.fmt.bg_fills.get(idx - 1001)
        } else {
            colors.fmt.fills.get(idx.saturating_sub(1))
        };
        list.and_then(|f| super::style::fill(colors, f, ph))
            .or(ph.map(Fill::Solid))
    } else {
        None
    };
    match fill {
        Some(Fill::Picture(id)) => (None, r.media(rels, &id)),
        Some(f) => (f.paint(1.0, 1.0), None),
        None => (None, None),
    }
}

/// Walk a shape tree, appending drawable items. `slide` is false for master and layout
/// trees, whose placeholders aren't drawn.
fn walk(
    r: &mut Reader<'_>,
    ctx: &Ctx<'_>,
    tree: &El,
    map: GroupMap,
    slide: bool,
    out: &mut Vec<Item>,
) {
    for el in tree.children() {
        if hidden(el) {
            continue;
        }
        match &*el.name {
            "sp" | "cxnSp" => {
                let ph = ph_of(el);
                if ph.is_some() && !slide {
                    continue;
                }
                if let Some(it) = shape(r, ctx, el, map, ph, None) {
                    out.push(it);
                }
            }
            "pic" => {
                let ph = ph_of(el);
                if ph.is_some() && !slide {
                    continue;
                }
                if let Some(it) = picture(r, ctx, el, map, ph) {
                    out.push(it);
                }
            }
            "grpSp" => {
                let Some(x) = xfrm(el.child("grpSpPr").and_then(|g| g.child("xfrm"))) else {
                    continue;
                };
                walk(r, ctx, el, map.child(&x), slide, out);
            }
            "graphicFrame" => frame(r, ctx, el, map, out),
            _ => {}
        }
    }
}

/// The placeholder chain for a slide shape: (layout ph, master ph).
fn chain<'x>(
    ctx: &'x Ctx<'_>,
    ph: &Option<(String, Option<String>)>,
) -> (Option<&'x Ph>, Option<&'x Ph>) {
    let Some((kind, idx)) = ph else {
        return (None, None);
    };
    let lp = ctx
        .layout
        .and_then(|l| find_ph(&l.phs, kind, idx.as_deref()));
    // The master placeholder matches by type (the layout's type when the slide gives none).
    let mkind = lp.map_or(kind.as_str(), |p| p.kind.as_str());
    let mp = ctx.master.and_then(|m| find_ph(&m.phs, mkind, None));
    (lp, mp)
}

fn style_ref<'e>(sp: &'e El, name: &str) -> Option<&'e El> {
    sp.child("style").and_then(|s| s.child(name))
}

fn shape(
    r: &mut Reader<'_>,
    ctx: &Ctx<'_>,
    sp: &El,
    map: GroupMap,
    ph: Option<(String, Option<String>)>,
    _base: Option<()>,
) -> Option<Item> {
    let c = ctx.colors;
    let sppr = sp.child("spPr");
    let (lp, mp) = chain(ctx, &ph);
    let x = xfrm(sppr.and_then(|s| s.child("xfrm")))
        .or(lp.and_then(|p| p.xfrm))
        .or(mp.and_then(|p| p.xfrm))?;
    let at = map.place(&x);
    if at.w <= 0.0 && at.h <= 0.0 {
        return None;
    }
    // Geometry.
    let (path, open) = if let Some(g) = sppr.and_then(|s| s.child("custGeom")) {
        geom::custom(g, at.w, at.h)
    } else {
        let g = sppr.and_then(|s| s.child("prstGeom"));
        let name = g.and_then(|g| g.attr("prst")).unwrap_or("rect");
        let av = g.map(geom::adjusts).unwrap_or_default();
        geom::preset(name, at.w, at.h, &av)
    };
    // Fill: the shape's own, else its style's.
    let style_fill = style_ref(sp, "fillRef").and_then(|f| {
        let idx = f
            .attr("idx")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let phc = c.color_in(f, None);
        if idx == 0 {
            return Some(Fill::None);
        }
        let list = if idx >= 1001 {
            c.fmt.bg_fills.get(idx - 1001)
        } else {
            c.fmt.fills.get(idx - 1)
        };
        list.and_then(|e| super::style::fill(c, e, phc))
            .or(phc.map(Fill::Solid))
    });
    let own_fill = sppr.and_then(|s| super::style::fill_in(c, s, None));
    let fill = own_fill.or(style_fill);
    let (fill_paint, pic) = match &fill {
        Some(Fill::Picture(id)) => (None, r.media(ctx.rels, id)),
        Some(f) if !open => (f.paint(at.w, at.h), None),
        _ => (None, None),
    };
    // Line: the style's, then the shape's own on top.
    let mut line = style_ref(sp, "lnRef")
        .and_then(|l| {
            let idx = l
                .attr("idx")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let phc = c.color_in(l, None);
            if idx == 0 {
                return None;
            }
            c.fmt.lines.get(idx - 1).map(|e| Line::parse(c, e, phc))
        })
        .unwrap_or_default();
    if let Some(ln) = sppr.and_then(|s| s.child("ln")) {
        line = line.over(&Line::parse(c, ln, None));
    }
    let pen = line.pen();
    // Text.
    let text = sp
        .child("txBody")
        .and_then(|t| text_body(r, ctx, sp, t, &ph, lp, mp, at.w));
    let title = ph.as_ref().is_some_and(|(k, _)| ph_family(k) == "title");
    Some(Item {
        alt: alt_of(sp),
        title,
        at,
        kind: Kind::Shape {
            path,
            open,
            fill: fill_paint,
            pic,
            pen,
            head: line.head,
            tail: line.tail,
            text,
        },
    })
}

fn picture(
    r: &mut Reader<'_>,
    ctx: &Ctx<'_>,
    el: &El,
    map: GroupMap,
    ph: Option<(String, Option<String>)>,
) -> Option<Item> {
    let (lp, mp) = chain(ctx, &ph);
    let sppr = el.child("spPr");
    let x = xfrm(sppr.and_then(|s| s.child("xfrm")))
        .or(lp.and_then(|p| p.xfrm))
        .or(mp.and_then(|p| p.xfrm))?;
    let blip = el.child("blipFill")?;
    let id = blip.child("blip")?.attr("embed")?;
    let media = r.media(ctx.rels, id)?;
    let crop = blip
        .child("srcRect")
        .map(|s| {
            let f = |k: &str| s.attr(k).and_then(super::style::pct).unwrap_or(0.0);
            [f("l"), f("t"), f("r"), f("b")]
        })
        .unwrap_or([0.0; 4]);
    let pen = sppr
        .and_then(|s| s.child("ln"))
        .map(|l| Line::parse(ctx.colors, l, None))
        .and_then(|l| l.pen());
    Some(Item {
        at: map.place(&x),
        kind: Kind::Picture { media, crop, pen },
        alt: alt_of(el),
        title: false,
    })
}

/// Tables and SmartArt (through the drawing PowerPoint saves with it). Charts and other
/// objects aren't drawn yet; their space stays empty.
fn frame(r: &mut Reader<'_>, ctx: &Ctx<'_>, el: &El, map: GroupMap, out: &mut Vec<Item>) {
    let Some(x) = xfrm(el.child("xfrm")) else {
        return;
    };
    let Some(data) = el.child("graphic").and_then(|g| g.child("graphicData")) else {
        return;
    };
    if let Some(tbl) = data.child("tbl") {
        if let Some(t) = table(r, ctx, tbl) {
            out.push(Item {
                at: map.place(&x),
                kind: Kind::Table(Box::new(t)),
                alt: alt_of(el),
                title: false,
            });
        }
        return;
    }
    if let Some(ch) = data.child("chart") {
        // A chart: drawn from the values cached in its part.
        let part = ch
            .attr("id")
            .and_then(|id| ctx.rels.target(id))
            .map(str::to_string);
        if let Some(root) = part.and_then(|p| r.pkg.xml(&p))
            && let Some(c) = crate::chart::parse(&root, ctx.colors, &ctx.colors.theme.minor[0])
        {
            out.push(Item {
                at: map.place(&x),
                kind: Kind::Chart(Box::new(c)),
                alt: alt_of(el),
                title: false,
            });
        }
        return;
    }
    if let Some(rel_ids) = data.child("relIds") {
        // SmartArt: the data model part points (in its extension list) to the drawing
        // PowerPoint rendered, which is ordinary shapes.
        let Some(dm) = rel_ids
            .attr("dm")
            .and_then(|id| ctx.rels.target(id))
            .map(str::to_string)
        else {
            return;
        };
        let Some(dm_root) = r.pkg.xml(&dm) else {
            return;
        };
        let Some(drawing_id) = dm_root
            .find("dataModelExt")
            .and_then(|e| e.attr("relId"))
            .map(str::to_string)
        else {
            return;
        };
        let Some(dpath) = ctx.rels.target(&drawing_id).map(str::to_string) else {
            return;
        };
        let Some(d) = r.pkg.xml(&dpath) else { return };
        let drels = r.pkg.rels(&dpath);
        let Some(tree) = d.child("spTree") else {
            return;
        };
        let inner = Ctx {
            colors: ctx.colors,
            rels: &drels,
            layout: None,
            master: None,
            number: ctx.number,
            default_tab: ctx.default_tab,
        };
        let gx = Xfrm {
            ch: Some((0.0, 0.0, x.w, x.h)),
            ..x
        };
        walk(r, &inner, tree, map.child(&gx), true, out);
    }
}

// ------------------------------------------------------------ text

/// Resolved run formatting.
fn run_style(c: &Colors, rp: &RProps, scale: f32) -> RunStyle {
    let theme = &c.theme;
    let face = |v: &Option<String>, slot: usize| -> String {
        let f = v.clone().unwrap_or_else(|| "+mn".to_string());
        if f.starts_with("+mj") {
            theme.major[slot.min(2)].clone()
        } else if f.starts_with("+mn") {
            theme.minor[slot.min(2)].clone()
        } else {
            f
        }
    };
    let latin = {
        let f = face(&rp.latin, 0);
        if f.is_empty() {
            "Calibri".to_string()
        } else {
            f
        }
    };
    let ea = Some(face(&rp.ea, 1))
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| latin.clone());
    let cs = Some(face(&rp.cs, 2))
        .filter(|f| !f.is_empty())
        .unwrap_or_else(|| latin.clone());
    let sz = (rp.sz.unwrap_or(18.0) * scale).clamp(1.0, 4000.0);
    let (color, alpha_lost) = rp.color.unwrap_or((Rgb(0, 0, 0), 1.0));
    let _ = alpha_lost;
    let base = rp.baseline.unwrap_or(0.0);
    RunStyle {
        fonts: [latin.clone(), latin, ea, cs],
        b: rp.b.unwrap_or(false),
        i: rp.i.unwrap_or(false),
        b_cs: rp.b.unwrap_or(false),
        i_cs: rp.i.unwrap_or(false),
        sz,
        sz_cs: sz,
        color,
        auto_color: false,
        u: rp.u.unwrap_or(Underline::None),
        u_color: None,
        strike: rp.strike.is_some_and(|s| s.0),
        dstrike: rp.strike.is_some_and(|s| s.1),
        caps: rp.caps == Some(Caps::All),
        small_caps: rp.caps == Some(Caps::Small),
        vanish: false,
        vert: if base > 0.0 {
            Vert::Super
        } else if base < 0.0 {
            Vert::Sub
        } else {
            Vert::Baseline
        },
        position: 0.0,
        spacing: rp.spc.unwrap_or(0.0),
        scale: 1.0,
        kern: None,
        highlight: rp.highlight,
        shd: None,
        bdr: None,
        rtl: false,
        cs: false,
    }
}

fn auto_label(scheme: &str, n: u32) -> String {
    use crate::docxlayout::numbering::{NumFmt, format_number};
    let (fmt, pre, post) = match scheme {
        s if s.starts_with("arabic") => (NumFmt::Decimal, "", ""),
        s if s.starts_with("romanUc") => (NumFmt::UpperRoman, "", ""),
        s if s.starts_with("romanLc") => (NumFmt::LowerRoman, "", ""),
        s if s.starts_with("alphaUc") => (NumFmt::UpperLetter, "", ""),
        s if s.starts_with("alphaLc") => (NumFmt::LowerLetter, "", ""),
        _ => (NumFmt::Decimal, "", ""),
    };
    let body = format_number(n, fmt);
    let (pre, post) = if scheme.ends_with("ParenBoth") {
        ("(", ")")
    } else if scheme.ends_with("ParenR") {
        (pre, ")")
    } else if scheme.ends_with("Period") {
        (pre, ".")
    } else if scheme.ends_with("Minus") {
        ("", " -")
    } else {
        (pre, post)
    };
    format!("{pre}{body}{post}")
}

fn spacing_pts(s: Option<Spacing>, size: f32) -> f32 {
    match s {
        Some(Spacing::Pts(p)) => p,
        Some(Spacing::Pct(f)) => f * size * 1.2,
        None => 0.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn text_body(
    r: &mut Reader<'_>,
    ctx: &Ctx<'_>,
    sp: &El,
    tx: &El,
    ph: &Option<(String, Option<String>)>,
    lp: Option<&Ph>,
    mp: Option<&Ph>,
    box_w: f32,
) -> Option<Text> {
    let c = ctx.colors;
    // Body properties: master placeholder, layout placeholder, the shape.
    let mut body = Body::default();
    for b in [mp.map(|p| &p.body), lp.map(|p| &p.body)]
        .into_iter()
        .flatten()
    {
        body.over(b);
    }
    if let Some(b) = tx.child("bodyPr") {
        body.over(&Body::parse(b));
    }
    // List styles: presentation default (or the master's style for this placeholder
    // family), the master and layout placeholders, the shape's own.
    let mut levels = r.default_text.clone();
    if let (Some((kind, _)), Some(styles)) = (ph, ctx.master.and_then(|m| m.styles.as_ref())) {
        let i = match ph_family(kind) {
            "title" => 0,
            "body" => 1,
            _ => 2,
        };
        levels.over(&styles[i]);
    }
    for l in [mp.map(|p| &p.lst), lp.map(|p| &p.lst)]
        .into_iter()
        .flatten()
    {
        levels.over(l);
    }
    if let Some(l) = tx.child("lstStyle") {
        levels.over(&Levels::parse(c, l));
    }
    // The shape style's font colour sits between the list styles and the runs.
    let font_ref = style_ref(sp, "fontRef");
    let ref_color = font_ref.and_then(|f| c.color_in(f, None));
    let ref_face = font_ref.and_then(|f| f.attr("idx")).map(|i| {
        if i == "major" {
            "+mj-lt".to_string()
        } else {
            "+mn-lt".to_string()
        }
    });
    let scale = body.font_scale.unwrap_or(1.0);
    let ln_reduce = body.ln_reduce.unwrap_or(0.0);
    let mut counters = [0u32; 9];
    let mut last_auto: Option<usize> = None;
    let mut blocks = Vec::new();
    for p in tx.all("p") {
        let ppr_el = p.child("pPr");
        let lvl = ppr_el
            .and_then(|x| x.attr("lvl"))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(8);
        let mut pp: PProps = levels.0[lvl].clone();
        if let Some(e) = ppr_el {
            pp.over(&PProps::parse(c, e));
        }
        let mut base_r = pp.def_r.clone();
        if let Some(col) = ref_color {
            // A style's font colour beats list-style defaults (white text on a filled
            // shape), but not colours set on the runs themselves.
            base_r.color = Some(col);
        }
        if base_r.latin.is_none() {
            base_r.latin = ref_face.clone();
        }
        if base_r.color.is_none() {
            base_r.color = c.scheme_color("tx1");
        }
        let mut items = Vec::new();
        let mut first_size: Option<f32> = None;
        let mut first_color: Option<Rgb> = None;
        let mut first_rs: Option<Arc<RunStyle>> = None;
        for k in p.children() {
            match &*k.name {
                "r" | "fld" => {
                    let mut rp = base_r.clone();
                    if let Some(x) = k.child("rPr") {
                        rp.over(&RProps::parse(c, x));
                    }
                    let mut text = k.child("t").map(El::text).unwrap_or_default();
                    if &*k.name == "fld" && k.attr("type") == Some("slidenum") {
                        text = ctx.number.to_string();
                    }
                    let link = rp.link.as_deref().and_then(|id| {
                        ctx.rels
                            .get(id)
                            .filter(|x| x.external)
                            .map(|x| Arc::new(Link::Url(x.target.clone())))
                    });
                    // Bullets take the run's own look, before any hyperlink colouring.
                    if first_rs.is_none() {
                        first_rs = Some(Arc::new(run_style(c, &rp, scale)));
                    }
                    if link.is_some() && k.child("rPr").and_then(|x| x.child("solidFill")).is_none()
                    {
                        rp.color = c.scheme_color("hlink");
                        rp.u = Some(Underline::Single);
                    }
                    let rs = Arc::new(run_style(c, &rp, scale));
                    first_size.get_or_insert(rs.sz);
                    first_color.get_or_insert(rs.color);
                    // Tabs inside the text become tab items.
                    for (i, part) in text.split('\t').enumerate() {
                        if i > 0 {
                            items.push(RunItem::Tab { rs: rs.clone() });
                        }
                        if !part.is_empty() {
                            items.push(RunItem::Text {
                                text: part.replace('\u{b}', "\n"),
                                rs: rs.clone(),
                                link: link.clone(),
                            });
                        }
                    }
                }
                "br" => items.push(RunItem::Break(crate::docxlayout::model::BreakKind::Line)),
                _ => {}
            }
        }
        let mut end_rp = base_r.clone();
        if let Some(x) = p.child("endParaRPr") {
            end_rp.over(&RProps::parse(c, x));
        }
        let mark = Arc::new(run_style(c, &end_rp, scale));
        let size = first_size.unwrap_or(mark.sz);
        let empty = items.is_empty();
        // Bullets and numbers (none on empty paragraphs).
        let label = match (&pp.bullet, empty) {
            (Some(Bullet::Char(ch)), false) => {
                last_auto = None;
                let mut rs = (*first_rs.clone().unwrap_or_else(|| mark.clone())).clone();
                if let Some(f) = &pp.bu_font
                    && !f.starts_with('+')
                {
                    rs.fonts = [f.clone(), f.clone(), f.clone(), f.clone()];
                }
                if let Some((col, _)) = pp.bu_clr {
                    rs.color = col;
                }
                rs.sz *= pp.bu_sz.unwrap_or(1.0);
                rs.u = Underline::None;
                Some(Label {
                    text: ch.clone(),
                    rs: Arc::new(rs),
                    suffix: Suffix::Tab,
                    jc: Align::Left,
                })
            }
            (Some(Bullet::Auto(scheme, start)), false) => {
                if last_auto.is_none_or(|l| l < lvl) {
                    counters[lvl] = start.saturating_sub(1);
                }
                for d in counters.iter_mut().skip(lvl + 1) {
                    *d = 0;
                }
                counters[lvl] += 1;
                last_auto = Some(lvl);
                let mut rs = (*first_rs.clone().unwrap_or_else(|| mark.clone())).clone();
                if let Some((col, _)) = pp.bu_clr {
                    rs.color = col;
                }
                rs.sz *= pp.bu_sz.unwrap_or(1.0);
                rs.u = Underline::None;
                Some(Label {
                    text: auto_label(scheme, counters[lvl]),
                    rs: Arc::new(rs),
                    suffix: Suffix::Tab,
                    jc: Align::Left,
                })
            }
            _ => {
                if !empty {
                    last_auto = None;
                }
                None
            }
        };
        let _ = first_color;
        let line = match pp.ln_spc {
            Some(Spacing::Pts(v)) => LineRule::Exact(v * scale),
            Some(Spacing::Pct(f)) => LineRule::Auto((f * (1.0 - ln_reduce)).max(0.1)),
            None => LineRule::Auto((1.0 - ln_reduce).max(0.1)),
        };
        let left = pp.mar_l.unwrap_or(0.0);
        let indent = pp.indent.unwrap_or(0.0);
        let mut tabs: Vec<TabDef> = pp
            .tabs
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|pos| TabDef {
                pos,
                kind: TabKind::Left,
                leader: Leader::None,
            })
            .collect();
        // The bullet's tab lands on the text indent.
        if label.is_some() && indent < 0.0 {
            tabs.push(TabDef {
                pos: left,
                kind: TabKind::Left,
                leader: Leader::None,
            });
        }
        let ps = ParaStyle {
            align: pp.algn.unwrap_or(Align::Left),
            left,
            right: pp.mar_r.unwrap_or(0.0),
            first: indent,
            before: spacing_pts(pp.spc_bef, size) * (1.0 - ln_reduce),
            after: spacing_pts(pp.spc_aft, size) * (1.0 - ln_reduce),
            line,
            contextual: false,
            keep_next: false,
            keep_lines: false,
            page_break_before: false,
            widow: false,
            tabs,
            shd: None,
            borders: [None; 5],
            bidi: pp.rtl.unwrap_or(false),
            style: String::new(),
        };
        blocks.push(Block::Para(Box::new(Para {
            p: ps,
            mark,
            items,
            label,
            outline: None,
        })));
    }
    let _ = box_w;
    let ins = [
        body.ins[0].unwrap_or(7.2),
        body.ins[1].unwrap_or(3.6),
        body.ins[2].unwrap_or(7.2),
        body.ins[3].unwrap_or(3.6),
    ];
    let anchor = match body.anchor.as_deref() {
        Some("ctr") => VAlign::Center,
        Some("b") => VAlign::Bottom,
        _ => VAlign::Top,
    };
    let has_text = blocks.iter().any(|b| match b {
        Block::Para(p) => !p.items.is_empty(),
        Block::Table(_) => true,
    });
    if !has_text {
        return None;
    }
    Some(Text {
        blocks,
        ins,
        anchor,
        wrap: body.wrap.unwrap_or(true),
        vert: body.vert.filter(|v| v != "horz"),
    })
}

// ------------------------------------------------------------ tables

fn table(r: &mut Reader<'_>, ctx: &Ctx<'_>, tbl: &El) -> Option<Table> {
    let c = ctx.colors;
    let grid: Vec<f32> = tbl
        .child("tblGrid")?
        .all("gridCol")
        .map(|g| attr_emu(g, "w").unwrap_or(72.0))
        .collect();
    if grid.is_empty() {
        return None;
    }
    let tpr = tbl.child("tblPr");
    let flag = |k: &str| tpr.and_then(|t| t.attr(k)).is_some_and(|v| v == "1");
    let (first_row, last_row, first_col, last_col, band_row, band_col) = (
        flag("firstRow"),
        flag("lastRow"),
        flag("firstCol"),
        flag("lastCol"),
        flag("bandRow"),
        flag("bandCol"),
    );
    let style = tpr
        .and_then(|t| t.child("tableStyleId"))
        .map(|s| s.text().trim().to_string())
        .and_then(|id| r.table_styles.get(&id).cloned());
    let rows_el: Vec<&El> = tbl.all("tr").collect();
    let nrows = rows_el.len();
    let mut rows = Vec::new();
    for (ri, tr) in rows_el.iter().enumerate() {
        let mut cells = Vec::new();
        let mut col = 0;
        for tc in tr.all("tc") {
            if tc.attr("hMerge") == Some("1") {
                col += 1;
                continue;
            }
            let span = tc
                .attr("gridSpan")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .max(1);
            let vmerge = if tc.attr("vMerge") == Some("1") {
                Some(VMerge::Continue)
            } else if tc
                .attr("rowSpan")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                > 1
            {
                Some(VMerge::Restart)
            } else {
                None
            };
            // Which table-style regions apply, weakest first.
            let mut regions: Vec<&str> = vec!["wholeTbl"];
            let body_row = if first_row { ri.wrapping_sub(1) } else { ri };
            if band_row && !(first_row && ri == 0) && !(last_row && ri + 1 == nrows) {
                regions.push(if body_row % 2 == 0 {
                    "band1H"
                } else {
                    "band2H"
                });
            }
            if band_col {
                regions.push(if col % 2 == 0 { "band1V" } else { "band2V" });
            }
            if first_col && col == 0 {
                regions.push("firstCol");
            }
            if last_col && col + span >= grid.len() {
                regions.push("lastCol");
            }
            if last_row && ri + 1 == nrows {
                regions.push("lastRow");
            }
            if first_row && ri == 0 {
                regions.push("firstRow");
            }
            let mut shd: Option<Rgb> = None;
            let mut borders: [Option<Border>; 4] = [None; 4];
            let mut txt = RProps::default();
            if let Some(st) = &style {
                for reg in &regions {
                    let Some(part) = st.child(reg) else { continue };
                    if let Some(ts) = part.child("tcTxStyle") {
                        if let Some(b) = ts.attr("b") {
                            txt.b = Some(b == "on");
                        }
                        if let Some(i) = ts.attr("i") {
                            txt.i = Some(i == "on");
                        }
                        if let Some(col) = c.color_in(ts, None) {
                            txt.color = Some(col);
                        }
                        if let Some(f) = ts.child("fontRef")
                            && let Some(col) = c.color_in(f, None)
                        {
                            txt.color = Some(col);
                        }
                    }
                    if let Some(ts) = part.child("tcStyle") {
                        if let Some(f) = ts
                            .child("fill")
                            .and_then(|f| super::style::fill_in(c, f, None))
                        {
                            shd = match f {
                                Fill::Solid((col, _)) => Some(col),
                                Fill::None => None,
                                _ => shd,
                            };
                        }
                        if let Some(fr) = ts.child("fillRef")
                            && let Some((col, _)) = c.color_in(fr, None)
                        {
                            shd = Some(col);
                        }
                        if let Some(bd) = ts.child("tcBdr") {
                            let is_top = ri == 0;
                            let is_bottom = ri + 1 == nrows;
                            let is_left = col == 0;
                            let is_right = col + span >= grid.len();
                            let side = |name: &str| -> Option<Option<Border>> {
                                let ln = bd.child(name)?.child("ln")?;
                                Some(table_border(c, ln))
                            };
                            let pick = |outer: &str, inner: &str, is_outer: bool| {
                                if is_outer { side(outer) } else { side(inner) }
                            };
                            if let Some(b) = pick("top", "insideH", is_top) {
                                borders[0] = b;
                            }
                            if let Some(b) = pick("left", "insideV", is_left) {
                                borders[1] = b;
                            }
                            if let Some(b) = pick("bottom", "insideH", is_bottom) {
                                borders[2] = b;
                            }
                            if let Some(b) = pick("right", "insideV", is_right) {
                                borders[3] = b;
                            }
                        }
                    }
                }
            }
            let tcpr = tc.child("tcPr");
            if let Some(p) = tcpr {
                if let Some(f) = super::style::fill_in(c, p, None) {
                    shd = match f {
                        Fill::Solid((col, _)) => Some(col),
                        Fill::Gradient { stops, .. } => stops.first().map(|s| s.1.0),
                        _ => None,
                    };
                }
                for (i, k) in ["lnT", "lnL", "lnB", "lnR"].iter().enumerate() {
                    if let Some(ln) = p.child(k) {
                        borders[i] = table_border(c, ln);
                    }
                }
            }
            let mar = [
                tcpr.and_then(|p| attr_emu(p, "marT")).unwrap_or(3.6),
                tcpr.and_then(|p| attr_emu(p, "marL")).unwrap_or(7.2),
                tcpr.and_then(|p| attr_emu(p, "marB")).unwrap_or(3.6),
                tcpr.and_then(|p| attr_emu(p, "marR")).unwrap_or(7.2),
            ];
            let valign = match tcpr.and_then(|p| p.attr("anchor")) {
                Some("ctr") => VAlign::Center,
                Some("b") => VAlign::Bottom,
                _ => VAlign::Top,
            };
            let blocks = tc
                .child("txBody")
                .map(|t| cell_text(r, ctx, t, &txt))
                .unwrap_or_default();
            cells.push(Cell {
                col,
                span,
                vmerge,
                width: Some(Width::Pt(
                    grid[col..(col + span).min(grid.len())].iter().sum(),
                )),
                borders,
                shd,
                valign,
                mar,
                blocks,
            });
            col += span;
        }
        rows.push(Row {
            height: attr_emu(tr, "h").map(|h| (h, false)),
            header: false,
            cant_split: true,
            cells,
        });
    }
    Some(Table {
        align: Align::Left,
        indent: 0.0,
        width: Some(Width::Pt(grid.iter().sum())),
        grid,
        rows,
    })
}

fn table_border(c: &Colors, ln: &El) -> Option<Border> {
    let l = Line::parse(c, ln, None);
    let pen = l.pen()?;
    Some(Border {
        style: if pen.dash.is_empty() {
            BorderStyle::Single
        } else {
            BorderStyle::Dashed
        },
        width: l.width.unwrap_or(1.0),
        color: pen.color,
        space: 0.0,
    })
}

/// A table cell's text: the presentation's default text style, the table style's text
/// formatting, then the runs.
fn cell_text(r: &mut Reader<'_>, ctx: &Ctx<'_>, tx: &El, txt: &RProps) -> Vec<Block> {
    let saved = r.default_text.clone();
    let mut lv = saved.clone();
    for l in lv.0.iter_mut() {
        l.def_r.over(txt);
    }
    r.default_text = lv;
    let fake = El::default();
    let out = text_body(r, ctx, &fake, tx, &None, None, None, 0.0)
        .map(|t| t.blocks)
        .unwrap_or_default();
    r.default_text = saved;
    out
}

impl Colors {
    pub(crate) fn scheme_color(&self, name: &str) -> Option<Col> {
        let mut e = El {
            name: "schemeClr".into(),
            ..El::default()
        };
        e.attrs.push(("val".into(), name.into()));
        self.color(&e, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_scale_children_into_place() {
        let g = Xfrm {
            x: 100.0,
            y: 50.0,
            w: 200.0,
            h: 100.0,
            ch: Some((0.0, 0.0, 400.0, 200.0)),
            ..Xfrm::default()
        };
        let m = GroupMap::identity().child(&g);
        let child = Xfrm {
            x: 200.0,
            y: 100.0,
            w: 200.0,
            h: 100.0,
            ..Xfrm::default()
        };
        let p = m.place(&child);
        assert_eq!((p.x, p.y, p.w, p.h), (200.0, 100.0, 100.0, 50.0));
    }

    #[test]
    fn numbering_labels() {
        assert_eq!(auto_label("arabicPeriod", 3), "3.");
        assert_eq!(auto_label("alphaLcParenR", 2), "b)");
        assert_eq!(auto_label("romanUcPeriod", 4), "IV.");
        assert_eq!(auto_label("arabicParenBoth", 1), "(1)");
    }
}
