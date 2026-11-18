//! The package reader: `.docx` (an OPC zip) → [`Document`].
//!
//! Property precedence follows Word: document defaults → table style (above the default
//! paragraph style when the paragraph uses it, below any other style) → paragraph style
//! → list level (its indents win over the style's) → character style → direct.

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::Arc;

use swirvo_core::EngineError;

use super::model::*;
use super::numbering::{Counters, NumFmt, Numbering, num_fmt};
use super::props::{
    Align, Border, CellProps, LineRule, ParaProps, Rgb, RunProps, TblProps, ThemeFont, VAlign,
    VMerge, Width, emu, twips,
};
use super::styles::{Cond, Layer, Styles};
use super::theme::Theme;
use super::xml::{self, El};

type Zip<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// Per-part size cap: a hostile archive can't make us inflate gigabytes.
const PART_LIMIT: u64 = 96 << 20;

#[derive(Debug, Default)]
struct Rels {
    map: HashMap<String, (String, bool)>,
}

impl Rels {
    fn target(&self, id: &str) -> Option<&str> {
        self.map.get(id).map(|(t, _)| t.as_str())
    }
    fn external(&self, id: &str) -> bool {
        self.map.get(id).is_some_and(|(_, e)| *e)
    }
}

fn read_bytes(z: &mut Zip<'_>, name: &str) -> Option<Vec<u8>> {
    let f = z.by_name(name).ok()?;
    let mut v = Vec::new();
    f.take(PART_LIMIT).read_to_end(&mut v).ok()?;
    Some(v)
}

fn read_xml(z: &mut Zip<'_>, name: &str) -> Option<El> {
    let b = read_bytes(z, name)?;
    let s = String::from_utf8_lossy(&b);
    // Strip a BOM; quick-xml handles the declaration.
    xml::parse(s.trim_start_matches('\u{feff}')).ok()
}

fn dir_of(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn rels_for(z: &mut Zip<'_>, part: &str) -> Rels {
    let dir = dir_of(part);
    let file = part.rsplit('/').next().unwrap_or(part);
    let path = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let mut r = Rels::default();
    if let Some(root) = read_xml(z, &path) {
        for rel in root.all("Relationship") {
            let (Some(id), Some(t)) = (rel.attr("Id"), rel.attr("Target")) else {
                continue;
            };
            let external = rel.attr("TargetMode") == Some("External");
            let target = if external {
                t.to_string()
            } else {
                join(dir, t)
            };
            r.map.insert(id.to_string(), (target, external));
        }
    }
    r
}

fn rel_of_type(z: &mut Zip<'_>, part: &str, suffix: &str) -> Option<String> {
    let dir = dir_of(part);
    let file = part.rsplit('/').next().unwrap_or(part);
    let path = if dir.is_empty() {
        format!("_rels/{file}.rels")
    } else {
        format!("{dir}/_rels/{file}.rels")
    };
    let root = read_xml(z, &path)?;
    root.all("Relationship")
        .find(|r| r.attr("Type").is_some_and(|t| t.ends_with(suffix)))
        .and_then(|r| r.attr("Target").map(|t| join(dir, t)))
}

/// A complex field in progress (`w:fldChar` begin … separate … end).
#[derive(Debug, Default)]
struct Field {
    instr: String,
    result: bool,
    kind: Option<FieldKind>,
    fmt: Option<NumFmt>,
    emitted: bool,
    link: Option<Arc<Link>>,
}

fn classify(instr: &str) -> (Option<FieldKind>, Option<NumFmt>, Option<Link>) {
    let mut words = instr.split_whitespace();
    let name = words.next().unwrap_or("").to_ascii_uppercase();
    let fmt = instr
        .split("\\*")
        .nth(1)
        .and_then(|s| match s.split_whitespace().next()? {
            "roman" => Some(NumFmt::LowerRoman),
            "ROMAN" | "Roman" => Some(NumFmt::UpperRoman),
            "alphabetic" => Some(NumFmt::LowerLetter),
            "ALPHABETIC" | "Alphabetic" => Some(NumFmt::UpperLetter),
            "Arabic" | "arabic" => Some(NumFmt::Decimal),
            _ => None,
        });
    match name.as_str() {
        "PAGE" => (Some(FieldKind::Page), fmt, None),
        "NUMPAGES" => (Some(FieldKind::NumPages), fmt, None),
        "SECTIONPAGES" => (Some(FieldKind::SectionPages), fmt, None),
        "HYPERLINK" => {
            let rest: Vec<&str> = instr.split('"').collect();
            let anchor = instr.contains("\\l");
            let target = rest.get(1).map(|s| s.to_string()).unwrap_or_default();
            if target.is_empty() {
                return (None, None, None);
            }
            let link = if anchor && !target.contains("://") {
                Link::Anchor(target)
            } else {
                Link::Url(target)
            };
            (None, None, Some(link))
        }
        _ => (None, None, None),
    }
}

struct Reader<'z> {
    zip: Zip<'z>,
    theme: Theme,
    styles: Styles,
    numbering: Numbering,
    counters: Counters,
    media: HashMap<String, Arc<Media>>,
    fields: Vec<Field>,
    uses_numpages: bool,
    pending_bookmarks: Vec<String>,
    /// Tables and text boxes we're inside. Reading and layout recurse once per level.
    nest: u32,
}

/// Tables and text boxes nested deeper than this are left out. Word documents rarely
/// go past 4; the cap keeps the recursion's stack use small and fixed.
const MAX_NEST: u32 = 16;

/// Table-style properties for the paragraphs of one cell.
#[derive(Clone, Default)]
struct CellLayer {
    ppr: ParaProps,
    rpr: RunProps,
}

/// Where we are while reading a part: its relationships, the table-style layer for the
/// paragraphs, and the hyperlink the runs belong to.
#[derive(Clone, Copy)]
struct Ctx<'a> {
    rels: &'a Rels,
    layer: Option<&'a CellLayer>,
    link: Option<&'a Arc<Link>>,
}

pub fn read(bytes: &[u8]) -> Result<Document, EngineError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(EngineError::decode)?;
    let main = rel_of_type(&mut zip, "", "/officeDocument")
        .filter(|p| zip.by_name(p).is_ok())
        .unwrap_or_else(|| "word/document.xml".to_string());
    let body_root = read_xml(&mut zip, &main)
        .ok_or_else(|| EngineError::Decode("not a Word document (no main part)".into()))?;
    let rels = rels_for(&mut zip, &main);

    let theme = rel_of_type(&mut zip, &main, "/theme")
        .and_then(|p| read_xml(&mut zip, &p))
        .map(|t| Theme::parse(&t))
        .unwrap_or_default();
    let styles = rel_of_type(&mut zip, &main, "/styles")
        .and_then(|p| read_xml(&mut zip, &p))
        .map(|s| Styles::parse(&s, &theme))
        .unwrap_or_default();
    let numbering = rel_of_type(&mut zip, &main, "/numbering")
        .and_then(|p| read_xml(&mut zip, &p))
        .map(|n| Numbering::parse(&n, &theme))
        .unwrap_or_default();

    let mut doc = Document {
        default_tab: 36.0,
        compat15: false,
        expand_shift_return: true,
        ..Default::default()
    };
    if let Some(s) = rel_of_type(&mut zip, &main, "/settings").and_then(|p| read_xml(&mut zip, &p))
    {
        if let Some(t) = s
            .child("defaultTabStop")
            .and_then(|t| t.val())
            .and_then(twips)
            && t > 1.0
        {
            doc.default_tab = t;
        }
        doc.even_odd = s.child("evenAndOddHeaders").is_some_and(El::on);
        if let Some(c) = s.child("compat") {
            doc.compat15 = c.all("compatSetting").any(|cs| {
                cs.attr("name") == Some("compatibilityMode")
                    && cs.attr("val").and_then(|v| v.parse::<u32>().ok()) >= Some(15)
            });
            if c.child("doNotExpandShiftReturn").is_some_and(El::on) {
                doc.expand_shift_return = false;
            }
        }
    }
    if let Some(ft) =
        rel_of_type(&mut zip, &main, "/fontTable").and_then(|p| read_xml(&mut zip, &p))
    {
        for f in ft.all("font") {
            let Some(name) = f.attr("name") else { continue };
            let fixed = f.child("pitch").and_then(|p| p.val()) == Some("fixed");
            let generic = match f.child("family").and_then(|p| p.val()) {
                Some("modern") if fixed => Generic::Mono,
                _ if fixed => Generic::Mono,
                Some("roman") => Generic::Serif,
                _ => Generic::Sans,
            };
            doc.generics.insert(name.to_ascii_lowercase(), generic);
        }
    }
    if let Some(core) = read_xml(&mut zip, "docProps/core.xml") {
        let get = |n: &str| {
            core.child(n)
                .map(|e| e.text().trim().to_string())
                .filter(|s| !s.is_empty())
        };
        doc.meta = Meta {
            title: get("title"),
            author: get("creator"),
            subject: get("subject"),
            keywords: get("keywords"),
            language: get("language"),
        };
    }
    if doc.meta.language.is_none() {
        doc.meta.language = styles.lang.clone();
    }

    let mut r = Reader {
        zip,
        theme,
        styles,
        numbering,
        counters: Counters::default(),
        media: HashMap::new(),
        fields: Vec::new(),
        uses_numpages: false,
        pending_bookmarks: Vec::new(),
        nest: 0,
    };
    let body = body_root
        .child("body")
        .ok_or_else(|| EngineError::Decode("the document has no body".into()))?;
    let ctx = Ctx {
        rels: &rels,
        layer: None,
        link: None,
    };
    let mut blocks = Vec::new();
    let mut sections = Vec::new();
    r.blocks(body, ctx, &mut blocks, Some(&mut sections));
    let last = body.child("sectPr").map(|s| r.sect(s)).unwrap_or_default();
    sections.push(Section { sect: last, blocks });
    // Header/footer references inherit from the previous section.
    let mut prev_h: [Option<String>; 3] = Default::default();
    let mut prev_f: [Option<String>; 3] = Default::default();
    for s in &mut sections {
        for i in 0..3 {
            if s.sect.headers[i].is_none() {
                s.sect.headers[i] = prev_h[i].clone();
            }
            if s.sect.footers[i].is_none() {
                s.sect.footers[i] = prev_f[i].clone();
            }
        }
        prev_h = s.sect.headers.clone();
        prev_f = s.sect.footers.clone();
    }
    let mut ids: Vec<String> = sections
        .iter()
        .flat_map(|s| s.sect.headers.iter().chain(s.sect.footers.iter()))
        .flatten()
        .cloned()
        .collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let Some(path) = rels.target(&id).map(str::to_string) else {
            continue;
        };
        let Some(root) = read_xml(&mut r.zip, &path) else {
            continue;
        };
        let part_rels = rels_for(&mut r.zip, &path);
        let mut out = Vec::new();
        r.fields.clear();
        r.blocks(
            &root,
            Ctx {
                rels: &part_rels,
                layer: None,
                link: None,
            },
            &mut out,
            None,
        );
        doc.parts.insert(id, out);
    }
    for (kind, tag, map) in [
        ("/footnotes", "footnote", &mut doc.footnotes),
        ("/endnotes", "endnote", &mut doc.endnotes),
    ] {
        let Some(path) = rel_of_type(&mut r.zip, &main, kind) else {
            continue;
        };
        let Some(root) = read_xml(&mut r.zip, &path) else {
            continue;
        };
        let part_rels = rels_for(&mut r.zip, &path);
        for n in root.all(tag) {
            if matches!(
                n.attr("type"),
                Some("separator" | "continuationSeparator" | "continuationNotice")
            ) {
                continue;
            }
            let Some(id) = n.attr("id") else { continue };
            let mut out = Vec::new();
            r.fields.clear();
            r.blocks(
                n,
                Ctx {
                    rels: &part_rels,
                    layer: None,
                    link: None,
                },
                &mut out,
                None,
            );
            map.insert(id.to_string(), out);
        }
    }
    doc.sections = sections;
    doc.uses_numpages = r.uses_numpages;
    Ok(doc)
}

impl Reader<'_> {
    // ------------------------------------------------------------ blocks

    /// Read block-level content. With `sections`, a paragraph that ends a section moves
    /// the blocks so far into a new [`Section`].
    fn blocks(
        &mut self,
        parent: &El,
        ctx: Ctx<'_>,
        out: &mut Vec<Block>,
        mut sections: Option<&mut Vec<Section>>,
    ) {
        for c in parent.children() {
            match &*c.name {
                "p" => {
                    let (p, sect) = self.para(c, ctx);
                    out.push(Block::Para(Box::new(p)));
                    if let (Some(s), Some(secs)) = (sect, sections.as_deref_mut()) {
                        secs.push(Section {
                            sect: s,
                            blocks: std::mem::take(out),
                        });
                    }
                }
                "tbl" if self.nest < MAX_NEST => {
                    self.nest += 1;
                    let t = self.table(c, ctx);
                    self.nest -= 1;
                    out.push(Block::Table(Box::new(t)));
                }
                "sdt" => {
                    if let Some(content) = c.child("sdtContent") {
                        self.blocks(content, ctx, out, sections.as_deref_mut());
                    }
                }
                "customXml" | "ins" | "moveTo" | "txbxContent" => {
                    self.blocks(c, ctx, out, sections.as_deref_mut());
                }
                "bookmarkStart" => {
                    if let Some(n) = c.attr("name") {
                        self.pending_bookmarks.push(n.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------ paragraphs

    fn para(&mut self, el: &El, ctx: Ctx<'_>) -> (Para, Option<SectPr>) {
        let ppr_el = el.child("pPr");
        let direct = ppr_el
            .map(|p| ParaProps::parse(p, &self.theme))
            .unwrap_or_default();
        let mark_direct = ppr_el
            .and_then(|p| p.child("rPr"))
            .map(|r| RunProps::parse(r, &self.theme))
            .unwrap_or_default();
        let style_id = direct
            .style
            .clone()
            .filter(|s| self.styles.exists(s))
            .or_else(|| self.styles.default_para.clone())
            .unwrap_or_default();
        let is_default = self.styles.default_para.as_deref() == Some(style_id.as_str());
        let sp = self.styles.para(&style_id);

        // Numbering: direct, else from the style.
        let num_id = direct.num_id.or(sp.0.num_id).unwrap_or(0);
        let ilvl = direct.ilvl.or(sp.0.ilvl).unwrap_or(0);
        let label = if num_id > 0 {
            self.numbering
                .label(&mut self.counters, num_id, ilvl, &self.styles)
        } else {
            None
        };

        let mut p = self.styles.defaults_p.clone();
        if let Some(l) = ctx.layer.filter(|_| !is_default) {
            p.apply(&l.ppr);
        }
        p.apply(&sp.0);
        if let Some(l) = ctx.layer.filter(|_| is_default) {
            p.apply(&l.ppr);
        }
        // Word applies the list level's indents over the paragraph style's ("List
        // Paragraph" has its own), and direct formatting over both.
        if let Some((_, lvl)) = &label {
            p.apply(&lvl.ppr);
        }
        p.apply(&direct);

        let base_r = self.base_runs(&sp.1, ctx.layer, is_default);
        let mut mark = base_r.clone();
        mark.apply(&mark_direct);
        let mark_rs = Arc::new(self.run_style(&mark));

        let label = label.and_then(|(text, lvl)| {
            if text.is_empty() {
                return None;
            }
            let mut lr = mark.clone();
            // The label ignores the paragraph mark's underline and strike.
            lr.u = None;
            lr.strike = None;
            lr.apply(&lvl.rpr);
            Some(Label {
                text,
                rs: Arc::new(self.run_style(&lr)),
                suffix: lvl.suffix,
                jc: lvl.jc,
            })
        });

        let style = self.para_style(&p, &style_id);
        let mut items = Vec::new();
        for b in self.pending_bookmarks.drain(..) {
            items.push(Item::Bookmark(b));
        }
        self.inlines(el, ctx, &base_r, &mut items);
        let sect = ppr_el.and_then(|p| p.child("sectPr")).map(|s| self.sect(s));
        (
            Para {
                p: style,
                mark: mark_rs,
                items,
                label,
                outline: p.outline.filter(|o| *o < 9),
            },
            sect,
        )
    }

    /// Run properties every run of a paragraph starts from.
    fn base_runs(
        &self,
        style_r: &RunProps,
        layer: Option<&CellLayer>,
        is_default: bool,
    ) -> RunProps {
        let mut r = self.styles.defaults_r.clone();
        if let Some(l) = layer.filter(|_| !is_default) {
            r.apply(&l.rpr);
        }
        r.apply(style_r);
        if let Some(l) = layer.filter(|_| is_default) {
            r.apply(&l.rpr);
        }
        r
    }

    fn para_style(&self, p: &ParaProps, style: &str) -> ParaStyle {
        let mut tabs: Vec<_> = p
            .tabs
            .iter()
            .copied()
            .filter(|t| t.kind != super::props::TabKind::Clear)
            .collect();
        tabs.sort_by(|a, b| a.pos.total_cmp(&b.pos));
        let auto = 14.0;
        ParaStyle {
            align: p.jc.unwrap_or(Align::Left),
            left: p.left.unwrap_or(0.0),
            right: p.right.unwrap_or(0.0),
            first: p.first.unwrap_or(0.0),
            before: if p.before_auto == Some(true) {
                auto
            } else {
                p.before.unwrap_or(0.0)
            },
            after: if p.after_auto == Some(true) {
                auto
            } else {
                p.after.unwrap_or(0.0)
            },
            line: p.line.unwrap_or(LineRule::Auto(1.0)),
            contextual: p.contextual.unwrap_or(false),
            keep_next: p.keep_next.unwrap_or(false),
            keep_lines: p.keep_lines.unwrap_or(false),
            page_break_before: p.page_break_before.unwrap_or(false),
            widow: p.widow.unwrap_or(false),
            tabs,
            shd: p.shd.flatten(),
            borders: p.borders,
            bidi: p.bidi.unwrap_or(false),
            style: style.to_string(),
        }
    }

    fn theme_font(&self, t: ThemeFont) -> Option<String> {
        let th = &self.theme;
        let s = match t {
            ThemeFont::MajorLatin => &th.major[0],
            ThemeFont::MinorLatin => &th.minor[0],
            ThemeFont::MajorEa => &th.major[1],
            ThemeFont::MinorEa => &th.minor[1],
            ThemeFont::MajorCs => &th.major[2],
            ThemeFont::MinorCs => &th.minor[2],
        };
        Some(s.clone()).filter(|s| !s.is_empty())
    }

    fn run_style(&self, r: &RunProps) -> RunStyle {
        let name = |f: &Option<super::props::FontRef>| -> Option<String> {
            match f {
                Some(super::props::FontRef::Name(n)) if !n.is_empty() => Some(n.clone()),
                Some(super::props::FontRef::Theme(t)) => self.theme_font(*t),
                _ => None,
            }
        };
        let ascii = name(&r.ascii).or_else(|| name(&r.hansi));
        let hansi = name(&r.hansi).or_else(|| ascii.clone());
        let ascii = ascii.unwrap_or_else(|| "Times New Roman".to_string());
        let hansi = hansi.unwrap_or_else(|| ascii.clone());
        let ea = name(&r.ea).unwrap_or_else(|| ascii.clone());
        let cs = name(&r.cs_font).unwrap_or_else(|| ascii.clone());
        let sz = r.sz.unwrap_or(10.0).clamp(1.0, 1638.0);
        let color = r.color.flatten();
        RunStyle {
            fonts: [ascii, hansi, ea, cs],
            b: r.b.unwrap_or(false),
            i: r.i.unwrap_or(false),
            b_cs: r.b_cs.or(r.b).unwrap_or(false),
            i_cs: r.i_cs.or(r.i).unwrap_or(false),
            sz,
            sz_cs: r.sz_cs.unwrap_or(sz).clamp(1.0, 1638.0),
            color: color.unwrap_or(Rgb(0, 0, 0)),
            auto_color: color.is_none(),
            u: r.u.unwrap_or_default(),
            u_color: r.u_color.flatten(),
            strike: r.strike.unwrap_or(false),
            dstrike: r.dstrike.unwrap_or(false),
            caps: r.caps.unwrap_or(false),
            small_caps: r.small_caps.unwrap_or(false),
            vanish: r.vanish.unwrap_or(false),
            vert: r.vert.unwrap_or_default(),
            position: r.position.unwrap_or(0.0),
            spacing: r.spacing.unwrap_or(0.0),
            scale: r.scale.unwrap_or(100.0).clamp(1.0, 600.0) / 100.0,
            kern: r.kern,
            highlight: r.highlight.flatten(),
            shd: r.shd.flatten(),
            bdr: r.bdr.flatten(),
            rtl: r.rtl.unwrap_or(false),
            cs: r.cs.unwrap_or(false),
        }
    }

    // ------------------------------------------------------------ runs

    fn suppressed(&self) -> bool {
        // Inside a field's instructions, or the cached result of a page-number field.
        self.fields
            .iter()
            .any(|f| !f.result || (f.kind.is_some() && f.emitted))
    }

    fn field_link(&self) -> Option<Arc<Link>> {
        self.fields.iter().rev().find_map(|f| f.link.clone())
    }

    fn inlines(&mut self, parent: &El, ctx: Ctx<'_>, base: &RunProps, out: &mut Vec<Item>) {
        for c in parent.children() {
            match &*c.name {
                "r" => self.run(c, ctx, base, out),
                "hyperlink" => {
                    let link = c
                        .attr("id")
                        .filter(|id| ctx.rels.external(id))
                        .and_then(|id| ctx.rels.target(id))
                        .map(|t| Link::Url(t.to_string()))
                        .or_else(|| c.attr("anchor").map(|a| Link::Anchor(a.to_string())))
                        .map(Arc::new);
                    let inner = Ctx {
                        link: link.as_ref().or(ctx.link),
                        ..ctx
                    };
                    self.inlines(c, inner, base, out);
                }
                "fldSimple" => {
                    let instr = c.attr("instr").unwrap_or("");
                    let (kind, fmt, link) = classify(instr);
                    if let Some(kind) = kind {
                        if matches!(kind, FieldKind::NumPages | FieldKind::SectionPages) {
                            self.uses_numpages = true;
                        }
                        let rs = c
                            .find("rPr")
                            .map(|rp| {
                                let mut r = base.clone();
                                r.apply(&RunProps::parse(rp, &self.theme));
                                r
                            })
                            .unwrap_or_else(|| base.clone());
                        out.push(Item::Field {
                            kind,
                            fmt,
                            rs: Arc::new(self.run_style(&rs)),
                        });
                    } else {
                        let link = link.map(Arc::new);
                        let inner = Ctx {
                            link: link.as_ref().or(ctx.link),
                            ..ctx
                        };
                        self.inlines(c, inner, base, out);
                    }
                }
                "smartTag" | "customXml" | "ins" | "moveTo" | "dir" | "bdo" => {
                    self.inlines(c, ctx, base, out)
                }
                "sdt" => {
                    if let Some(content) = c.child("sdtContent") {
                        self.inlines(content, ctx, base, out);
                    }
                }
                "bookmarkStart" => {
                    if let Some(n) = c.attr("name") {
                        out.push(Item::Bookmark(n.to_string()));
                    }
                }
                "oMathPara" | "oMath" => {
                    let text = math_text(c);
                    if !text.is_empty() && !self.suppressed() {
                        let mut r = base.clone();
                        r.ascii = Some(super::props::FontRef::Name("Cambria Math".into()));
                        r.hansi = r.ascii.clone();
                        out.push(Item::Text {
                            text,
                            rs: Arc::new(self.run_style(&r)),
                            link: None,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn run(&mut self, el: &El, ctx: Ctx<'_>, base: &RunProps, out: &mut Vec<Item>) {
        let direct = el
            .child("rPr")
            .map(|r| RunProps::parse(r, &self.theme))
            .unwrap_or_default();
        let mut r = base.clone();
        if let Some(cs) = direct.style.as_deref() {
            r.apply(&self.styles.char(cs));
        }
        r.apply(&direct);
        let rs = Arc::new(self.run_style(&r));
        let link = ctx.link.cloned().or_else(|| self.field_link());
        let hidden = rs.vanish;
        let mut text = String::new();
        let flush = |text: &mut String, out: &mut Vec<Item>, rs: &Arc<RunStyle>| {
            if !text.is_empty() {
                out.push(Item::Text {
                    text: std::mem::take(text),
                    rs: rs.clone(),
                    link: link.clone(),
                });
            }
        };
        for c in el.children() {
            match &*c.name {
                "fldChar" => {
                    flush(&mut text, out, &rs);
                    match c.attr("fldCharType") {
                        Some("begin") => self.fields.push(Field::default()),
                        Some("separate") => {
                            if let Some(f) = self.fields.last_mut() {
                                f.result = true;
                                let (kind, fmt, link) = classify(&f.instr);
                                f.kind = kind;
                                f.fmt = fmt;
                                f.link = link.map(Arc::new);
                            }
                            self.emit_field(&rs, out);
                        }
                        Some("end") => {
                            if let Some(f) = self.fields.last_mut()
                                && !f.result
                            {
                                f.result = true;
                                let (kind, fmt, _) = classify(&f.instr);
                                f.kind = kind;
                                f.fmt = fmt;
                            }
                            self.emit_field(&rs, out);
                            self.fields.pop();
                        }
                        _ => {}
                    }
                    continue;
                }
                "instrText" => {
                    if let Some(f) = self.fields.last_mut()
                        && !f.result
                    {
                        f.instr.push_str(&c.text());
                    }
                    continue;
                }
                _ => {}
            }
            if self.suppressed() || hidden {
                continue;
            }
            match &*c.name {
                "t" => text.push_str(&c.text()),
                "tab" | "ptab" => {
                    flush(&mut text, out, &rs);
                    out.push(Item::Tab { rs: rs.clone() });
                }
                "br" => {
                    flush(&mut text, out, &rs);
                    let kind = match c.attr("type") {
                        Some("page") => BreakKind::Page,
                        Some("column") => BreakKind::Column,
                        _ => BreakKind::Line,
                    };
                    out.push(Item::Break(kind));
                }
                "cr" => {
                    flush(&mut text, out, &rs);
                    out.push(Item::Break(BreakKind::Line));
                }
                "noBreakHyphen" => text.push('\u{2011}'),
                "softHyphen" => text.push('\u{ad}'),
                "sym" => {
                    flush(&mut text, out, &rs);
                    let ch = c
                        .attr("char")
                        .and_then(|h| u32::from_str_radix(h, 16).ok())
                        .and_then(char::from_u32);
                    if let Some(ch) = ch {
                        let mut s = (*rs).clone();
                        if let Some(f) = c.attr("font") {
                            s.fonts = std::array::from_fn(|_| f.to_string());
                        }
                        out.push(Item::Text {
                            text: ch.to_string(),
                            rs: Arc::new(s),
                            link: link.clone(),
                        });
                    }
                }
                "drawing" => {
                    flush(&mut text, out, &rs);
                    for d in c.children() {
                        if let Some(dr) = self.drawing(d, ctx) {
                            out.push(Item::Drawing(Box::new(dr)));
                        }
                    }
                }
                "pict" | "object" => {
                    flush(&mut text, out, &rs);
                    if let Some(dr) = self.vml(c, ctx) {
                        out.push(Item::Drawing(Box::new(dr)));
                    }
                }
                "footnoteReference" | "endnoteReference" => {
                    flush(&mut text, out, &rs);
                    if let Some(id) = c.attr("id") {
                        out.push(Item::NoteRef {
                            id: id.to_string(),
                            endnote: &*c.name == "endnoteReference",
                            rs: rs.clone(),
                        });
                    }
                }
                "footnoteRef" | "endnoteRef" => {
                    flush(&mut text, out, &rs);
                    out.push(Item::NoteMark { rs: rs.clone() });
                }
                _ => {}
            }
        }
        flush(&mut text, out, &rs);
    }

    fn emit_field(&mut self, rs: &Arc<RunStyle>, out: &mut Vec<Item>) {
        let Some(f) = self.fields.last_mut() else {
            return;
        };
        if f.emitted {
            return;
        }
        if let Some(kind) = f.kind {
            f.emitted = true;
            if matches!(kind, FieldKind::NumPages | FieldKind::SectionPages) {
                self.uses_numpages = true;
            }
            let fmt = f.fmt;
            // Only an outer field shows; nested page fields inside instructions don't.
            if self.fields.len() == 1
                || self.fields[..self.fields.len() - 1]
                    .iter()
                    .all(|o| o.result)
            {
                out.push(Item::Field {
                    kind,
                    fmt,
                    rs: rs.clone(),
                });
            }
        }
    }

    // ------------------------------------------------------------ drawings

    fn media(&mut self, ctx: Ctx<'_>, id: &str) -> Option<Arc<Media>> {
        if ctx.rels.external(id) {
            return None;
        }
        let path = ctx.rels.target(id)?.to_string();
        if let Some(m) = self.media.get(&path) {
            return Some(m.clone());
        }
        let bytes = read_bytes(&mut self.zip, &path)?;
        let m = Arc::new(Media {
            bytes: Arc::new(bytes),
        });
        self.media.insert(path, m.clone());
        Some(m)
    }

    fn drawing(&mut self, d: &El, ctx: Ctx<'_>) -> Option<Drawing> {
        let anchored = match &*d.name {
            "inline" => false,
            "anchor" => true,
            _ => return None,
        };
        let ext = d.child("extent");
        let w = ext.and_then(|e| e.attr("cx")).and_then(emu).unwrap_or(0.0);
        let h = ext.and_then(|e| e.attr("cy")).and_then(emu).unwrap_or(0.0);
        let data = d.find("graphicData");
        let kind = match data.and_then(|g| g.children().next()) {
            Some(pic) if &*pic.name == "pic" => {
                let media = pic
                    .find("blip")
                    .and_then(|b| b.attr("embed"))
                    .and_then(|id| self.media(ctx, id));
                let crop = pic.find("srcRect").map_or([0.0; 4], |s| {
                    let f = |k: &str| {
                        s.attr(k)
                            .and_then(|v| v.parse::<f32>().ok())
                            .map_or(0.0, |v| (v / 100_000.0).clamp(-1.0, 0.99))
                    };
                    [f("l"), f("t"), f("r"), f("b")]
                });
                DrawKind::Picture { media, crop }
            }
            Some(wsp) if &*wsp.name == "wsp" => self.shape(wsp, ctx),
            Some(ch) if &*ch.name == "chart" => ch
                .attr("id")
                .filter(|id| !ctx.rels.external(id))
                .and_then(|id| ctx.rels.target(id))
                .map(str::to_string)
                .and_then(|path| read_xml(&mut self.zip, &path))
                .and_then(|root| {
                    let colors = crate::pptxlayout::style::Colors {
                        theme: self.theme.clone(),
                        fmt: Default::default(),
                        map: Default::default(),
                    };
                    let font = if self.theme.minor[0].is_empty() {
                        "Calibri".to_string()
                    } else {
                        self.theme.minor[0].clone()
                    };
                    let mut c = crate::chart::parse(&root, &colors, &font)?;
                    // Word's automatic chart area is white.
                    if root.child("spPr").is_none() {
                        c.background = Some((Rgb(255, 255, 255), 1.0));
                    }
                    Some(c)
                })
                .map_or(DrawKind::Placeholder, |c| DrawKind::Chart(Box::new(c))),
            _ => DrawKind::Placeholder,
        };
        let anchor = if anchored { Some(self.anchor(d)) } else { None };
        // Word keeps alt text on the drawing's properties: a description, else a title.
        let alt = d.child("docPr").and_then(|p| {
            ["descr", "title"]
                .iter()
                .filter_map(|k| p.attr(k))
                .map(str::trim)
                .find(|v| !v.is_empty())
                .map(str::to_string)
        });
        Some(Drawing {
            w,
            h,
            kind,
            anchor,
            alt,
        })
    }

    fn shape(&mut self, wsp: &El, ctx: Ctx<'_>) -> DrawKind {
        let sp = wsp.child("spPr");
        let geom = match sp
            .and_then(|s| s.child("prstGeom"))
            .and_then(|g| g.attr("prst"))
        {
            Some("roundRect") => Geom::RoundRect,
            Some("ellipse") => Geom::Ellipse,
            Some("line" | "straightConnector1") => Geom::Line,
            _ => Geom::Rect,
        };
        let style = wsp.child("style");
        let style_color = |name: &str| -> Option<Rgb> {
            let r = style?.child(name)?;
            if r.attr("idx").and_then(|i| i.parse::<u32>().ok()) == Some(0) {
                return None;
            }
            r.children().find_map(|c| self.theme.drawing_color(c))
        };
        let fill = match sp {
            Some(s) if s.child("noFill").is_some() => None,
            Some(s) if s.child("solidFill").is_some() => s
                .all("solidFill")
                .last()
                .and_then(|f| f.children().find_map(|c| self.theme.drawing_color(c))),
            Some(s) if s.child("gradFill").is_some() => s
                .find("gs")
                .and_then(|g| g.children().find_map(|c| self.theme.drawing_color(c))),
            _ => style_color("fillRef"),
        };
        let ln = sp.and_then(|s| s.child("ln"));
        let line = match ln {
            Some(l) if l.child("noFill").is_some() => None,
            Some(l) => {
                let width = l.attr("w").and_then(emu).unwrap_or(0.75);
                let color = l
                    .child("solidFill")
                    .and_then(|f| f.children().find_map(|c| self.theme.drawing_color(c)))
                    .or_else(|| style_color("lnRef"));
                color.map(|c| (c, width.max(0.25)))
            }
            None => style_color("lnRef").map(|c| (c, 0.75)),
        };
        let mut text = Vec::new();
        if let Some(content) = wsp.child("txbx").and_then(|t| t.child("txbxContent"))
            && self.nest < MAX_NEST
        {
            self.nest += 1;
            self.blocks(content, Ctx { layer: None, ..ctx }, &mut text, None);
            self.nest -= 1;
        }
        let body = wsp.child("bodyPr");
        let ins = |k: &str, d: f32| body.and_then(|b| b.attr(k)).and_then(emu).unwrap_or(d);
        let valign = match body.and_then(|b| b.attr("anchor")) {
            Some("ctr") => VAlign::Center,
            Some("b") => VAlign::Bottom,
            _ => VAlign::Top,
        };
        DrawKind::Shape {
            geom,
            fill,
            line,
            text,
            ins: [
                ins("lIns", 7.2),
                ins("tIns", 3.6),
                ins("rIns", 7.2),
                ins("bIns", 3.6),
            ],
            valign,
        }
    }

    fn anchor(&self, d: &El) -> Anchor {
        let pos = |name: &str| -> (Option<String>, Pos) {
            let Some(p) = d.child(name) else {
                return (None, Pos::Offset(0.0));
            };
            let rel = p.attr("relativeFrom").map(str::to_string);
            if let Some(o) = p.child("posOffset").and_then(|o| emu(&o.text())) {
                return (rel, Pos::Offset(o));
            }
            let a = match p.child("align").map(|a| a.text()).as_deref().map(str::trim) {
                Some("center") => 1,
                Some("right" | "bottom" | "outside") => 2,
                _ => 0,
            };
            (rel, Pos::Align(a))
        };
        let (hr, h) = pos("positionH");
        let (vr, v) = pos("positionV");
        let simple = d.attr("simplePos") == Some("1");
        let (h, v, hr, vr) = if simple {
            let sp = d.child("simplePos");
            let x = sp.and_then(|s| s.attr("x")).and_then(emu).unwrap_or(0.0);
            let y = sp.and_then(|s| s.attr("y")).and_then(emu).unwrap_or(0.0);
            (
                Pos::Offset(x),
                Pos::Offset(y),
                Some("page".to_string()),
                Some("page".to_string()),
            )
        } else {
            (h, v, hr, vr)
        };
        let h_rel = match hr.as_deref() {
            Some("page") => HRel::Page,
            Some("column") => HRel::Column,
            Some("character") => HRel::Character,
            Some("leftMargin" | "insideMargin") => HRel::LeftMargin,
            Some("rightMargin" | "outsideMargin") => HRel::RightMargin,
            _ => HRel::Margin,
        };
        let v_rel = match vr.as_deref() {
            Some("page") => VRel::Page,
            Some("margin") => VRel::Margin,
            Some("line") => VRel::Line,
            Some("topMargin" | "insideMargin") => VRel::TopMargin,
            Some("bottomMargin" | "outsideMargin") => VRel::BottomMargin,
            _ => VRel::Paragraph,
        };
        let wrap = if d.child("wrapTopAndBottom").is_some() {
            Wrap::TopBottom
        } else if d.child("wrapSquare").is_some()
            || d.child("wrapTight").is_some()
            || d.child("wrapThrough").is_some()
        {
            Wrap::Square
        } else {
            Wrap::None
        };
        let dist = |k: &str| d.attr(k).and_then(emu).unwrap_or(0.0);
        Anchor {
            h_rel,
            h,
            v_rel,
            v,
            wrap,
            behind: d.attr("behindDoc") == Some("1"),
            dist: [dist("distT"), dist("distB"), dist("distL"), dist("distR")],
        }
    }

    /// Legacy VML pictures (`w:pict`, `w:object` previews): size from the style string.
    fn vml(&mut self, el: &El, ctx: Ctx<'_>) -> Option<Drawing> {
        let shape = el.find("shape").or_else(|| el.find("rect"))?;
        let img = shape.find("imagedata")?;
        let id = img.attr("id").or_else(|| img.attr("pict"))?;
        let media = self.media(ctx, id);
        let style = shape.attr("style").unwrap_or("");
        let get = |k: &str| {
            style.split(';').find_map(|kv| {
                let (key, v) = kv.split_once(':')?;
                (key.trim() == k).then(|| css_length(v.trim())).flatten()
            })
        };
        let (w, h) = (get("width")?, get("height")?);
        Some(Drawing {
            w,
            h,
            kind: DrawKind::Picture {
                media,
                crop: [0.0; 4],
            },
            anchor: None,
            alt: None,
        })
    }

    // ------------------------------------------------------------ tables

    fn table(&mut self, el: &El, ctx: Ctx<'_>) -> Table {
        let direct = el
            .child("tblPr")
            .map(|t| TblProps::parse(t, &self.theme))
            .unwrap_or_default();
        let style_id = direct
            .style
            .clone()
            .filter(|s| self.styles.exists(s))
            .or_else(|| self.styles.default_table.clone());
        let ts = style_id.as_deref().map(|id| self.styles.table(id));
        let mut tbl = ts.as_ref().map(|t| t.base.tbl.clone()).unwrap_or_default();
        tbl.apply(&direct);
        let look = tbl.look.unwrap_or_default();
        let grid: Vec<f32> = el
            .child("tblGrid")
            .map(|g| {
                g.all("gridCol")
                    .map(|c| c.attr("w").and_then(twips).unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();

        // Rows and cells as elements first: conditional formats need the shape.
        let mut rows: Vec<&El> = Vec::new();
        collect(el, "tr", &mut rows);
        let mut shaped: Vec<(super::props::RowProps, Vec<CellSlot<'_>>)> = Vec::new();
        for tr in &rows {
            let mut rp = ts.as_ref().map(|t| t.base.tr.clone()).unwrap_or_default();
            if let Some(p) = tr.child("trPr") {
                rp.apply(&super::props::RowProps::parse(p));
            }
            let mut cells: Vec<&El> = Vec::new();
            collect(tr, "tc", &mut cells);
            let mut col = rp.grid_before.unwrap_or(0);
            let mut out = Vec::new();
            for tc in cells {
                let span = tc
                    .child("tcPr")
                    .and_then(|p| p.child("gridSpan"))
                    .and_then(|g| g.val()?.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                out.push((tc, col, span));
                col += span;
            }
            shaped.push((rp, out));
        }
        let ncols = grid.len().max(
            shaped
                .iter()
                .map(|(_, c)| c.last().map_or(0, |l| l.1 + l.2))
                .max()
                .unwrap_or(0),
        );
        let nrows = shaped.len();
        let def_mar = [Some(0.0), Some(5.4), Some(0.0), Some(5.4)];
        let mut tmar = def_mar;
        for (d, s) in tmar.iter_mut().zip(tbl.mar.iter()) {
            if s.is_some() {
                *d = *s;
            }
        }
        let rb = tbl.row_band.unwrap_or(1).max(1) as usize;
        let cb = tbl.col_band.unwrap_or(1).max(1) as usize;

        let mut out_rows = Vec::new();
        for (r, (rp, cells)) in shaped.iter().enumerate() {
            let mut out_cells = Vec::new();
            for &(tc, col, span) in cells {
                let mut conds = vec![Cond::Whole];
                if !look.no_vband {
                    let k = col as isize - look.first_col as isize;
                    let is_last = look.last_col && col + span >= ncols;
                    if k >= 0 && !is_last {
                        conds.push(if (k as usize / cb).is_multiple_of(2) {
                            Cond::Band1Vert
                        } else {
                            Cond::Band2Vert
                        });
                    }
                }
                if !look.no_hband {
                    let k = r as isize - look.first_row as isize;
                    let is_last = look.last_row && r + 1 == nrows;
                    if k >= 0 && !is_last {
                        conds.push(if (k as usize / rb).is_multiple_of(2) {
                            Cond::Band1Horz
                        } else {
                            Cond::Band2Horz
                        });
                    }
                }
                if look.first_col && col == 0 {
                    conds.push(Cond::FirstCol);
                }
                if look.last_col && col + span >= ncols {
                    conds.push(Cond::LastCol);
                }
                if look.first_row && r == 0 {
                    conds.push(Cond::FirstRow);
                }
                if look.last_row && r + 1 == nrows {
                    conds.push(Cond::LastRow);
                }
                let mut layer = Layer::default();
                let mut cond_tbl_borders = TblProps::default();
                if let Some(ts) = &ts {
                    layer.ppr = ts.base.ppr.clone();
                    layer.rpr = ts.base.rpr.clone();
                    layer.tc = ts.base.tc.clone();
                    for c in &conds {
                        if let Some(l) = ts.conds.get(c) {
                            layer.ppr.apply(&l.ppr);
                            layer.rpr.apply(&l.rpr);
                            layer.tc.apply(&l.tc);
                            cond_tbl_borders.apply(&l.tbl);
                        }
                    }
                }
                let mut cp = layer.tc.clone();
                if let Some(p) = tc.child("tcPr") {
                    cp.apply(&CellProps::parse(p, &self.theme));
                }
                let last_row = r + 1 == nrows;
                let first_col = col == 0;
                let last_col = col + span >= ncols;
                let tb = |i: usize| cond_tbl_borders.borders[i].or(tbl.borders[i]);
                let edge = |i: usize, outer: bool, inside: usize| -> Option<Border> {
                    cp.borders[i]
                        .or(if outer { None } else { cp.borders[inside] })
                        .or(if outer { tb(i) } else { tb(inside) })
                };
                let borders = [
                    edge(0, r == 0, 4),
                    edge(1, first_col, 5),
                    edge(2, last_row, 4),
                    edge(3, last_col, 5),
                ];
                let mut mar = tmar;
                for (d, s) in mar.iter_mut().zip(cp.mar.iter()) {
                    if s.is_some() {
                        *d = *s;
                    }
                }
                let cell_layer = CellLayer {
                    ppr: layer.ppr,
                    rpr: layer.rpr,
                };
                let mut blocks = Vec::new();
                if cp.vmerge != Some(VMerge::Continue) {
                    self.blocks(
                        tc,
                        Ctx {
                            layer: Some(&cell_layer),
                            ..ctx
                        },
                        &mut blocks,
                        None,
                    );
                }
                out_cells.push(Cell {
                    col,
                    span,
                    vmerge: cp.vmerge,
                    width: cp.width,
                    borders,
                    shd: cp.shd.flatten().or(tbl.shd.flatten()),
                    valign: cp.valign.unwrap_or_default(),
                    mar: mar.map(|m| m.unwrap_or(0.0)),
                    blocks,
                });
            }
            out_rows.push(Row {
                height: rp.height,
                header: rp.header.unwrap_or(false),
                cant_split: rp.cant_split.unwrap_or(false),
                cells: out_cells,
            });
        }
        Table {
            align: tbl.jc.unwrap_or(Align::Left),
            indent: tbl.ind.unwrap_or(0.0),
            width: tbl.width.filter(|w| *w != Width::Auto),
            grid,
            rows: out_rows,
        }
    }

    // ------------------------------------------------------------ sections

    fn sect(&self, el: &El) -> SectPr {
        let mut s = SectPr::default();
        if let Some(sz) = el.child("pgSz") {
            s.w = sz.attr("w").and_then(twips).unwrap_or(s.w);
            s.h = sz.attr("h").and_then(twips).unwrap_or(s.h);
            if sz.attr("orient") == Some("landscape") && s.w < s.h {
                std::mem::swap(&mut s.w, &mut s.h);
            }
        }
        if let Some(m) = el.child("pgMar") {
            let g = |k: &str, d: f32| m.attr(k).and_then(twips).map_or(d, f32::abs);
            s.top = g("top", s.top);
            s.right = g("right", s.right);
            s.bottom = g("bottom", s.bottom);
            s.left = g("left", s.left);
            s.header = g("header", s.header);
            s.footer = g("footer", s.footer);
            s.gutter = g("gutter", 0.0);
        }
        s.left += s.gutter;
        let width = (s.w - s.left - s.right).max(36.0);
        s.cols = Cols {
            cols: vec![(width, 0.0)],
            sep: false,
        };
        if let Some(c) = el.child("cols") {
            let n = c
                .attr("num")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 45);
            let space = c.attr("space").and_then(twips).unwrap_or(36.0);
            let explicit: Vec<(f32, f32)> = c
                .all("col")
                .filter_map(|col| {
                    Some((
                        col.attr("w").and_then(twips)?,
                        col.attr("space").and_then(twips).unwrap_or(0.0),
                    ))
                })
                .collect();
            let equal = c
                .attr("equalWidth")
                .is_none_or(|v| !matches!(v, "0" | "false"));
            s.cols.sep = c.attr("sep").is_some_and(|v| matches!(v, "1" | "true"));
            if !equal && explicit.len() > 1 {
                s.cols.cols = explicit;
            } else if n > 1 {
                let w = ((width - space * (n - 1) as f32) / n as f32).max(18.0);
                s.cols.cols = (0..n).map(|_| (w, space)).collect();
            }
        }
        s.kind = match el.child("type").and_then(|t| t.val()) {
            Some("continuous") => SectKind::Continuous,
            Some("evenPage") => SectKind::EvenPage,
            Some("oddPage") => SectKind::OddPage,
            Some("nextColumn") => SectKind::NextColumn,
            _ => SectKind::NextPage,
        };
        s.title_pg = el.child("titlePg").is_some_and(El::on);
        for (tag, slot) in [("headerReference", 0), ("footerReference", 1)] {
            for h in el.all(tag) {
                let i = match h.attr("type") {
                    Some("first") => 1,
                    Some("even") => 2,
                    _ => 0,
                };
                let id = h.attr("id").map(str::to_string);
                if slot == 0 {
                    s.headers[i] = id;
                } else {
                    s.footers[i] = id;
                }
            }
        }
        if let Some(p) = el.child("pgNumType") {
            s.pg_start = p.attr("start").and_then(|v| v.parse().ok());
            s.pg_fmt = p.attr("fmt").map_or(NumFmt::Decimal, num_fmt);
        }
        s.v_align = match el.child("vAlign").and_then(|v| v.val()) {
            Some("center") => VAlign::Center,
            Some("bottom") => VAlign::Bottom,
            _ => VAlign::Top,
        };
        s
    }
}

/// A table cell element with its first grid column and its span.
type CellSlot<'a> = (&'a El, usize, usize);

/// Children named `name`, looking through content controls and custom XML wrappers.
fn collect<'a>(parent: &'a El, name: &str, out: &mut Vec<&'a El>) {
    for c in parent.children() {
        if &*c.name == name {
            out.push(c);
        } else if matches!(&*c.name, "sdt" | "sdtContent" | "customXml" | "ins") {
            collect(c, name, out);
        }
    }
}

fn math_text(e: &El) -> String {
    let mut s = String::new();
    fn walk(e: &El, s: &mut String) {
        for c in e.children() {
            if &*c.name == "t" {
                s.push_str(&c.text());
            } else {
                walk(c, s);
            }
        }
    }
    walk(e, &mut s);
    s
}

fn css_length(v: &str) -> Option<f32> {
    let split = v.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(v.len());
    let (n, unit) = v.split_at(split);
    let n: f32 = n.trim().parse().ok()?;
    Some(match unit {
        "" | "pt" => n,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "px" => n * 0.75,
        _ => return None,
    })
}
