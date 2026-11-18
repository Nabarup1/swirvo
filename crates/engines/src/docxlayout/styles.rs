//! `styles.xml`: document defaults, style inheritance (`basedOn`), and table styles with
//! their conditional formats (header row, banding, first column…).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::props::{CellProps, ParaProps, RowProps, RunProps, TblProps};
use super::theme::Theme;
use super::xml::El;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Para,
    Char,
    Table,
    Numbering,
}

/// Table-style regions, in increasing precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cond {
    Whole,
    Band1Vert,
    Band2Vert,
    Band1Horz,
    Band2Horz,
    FirstCol,
    LastCol,
    FirstRow,
    LastRow,
    NeCell,
    NwCell,
    SeCell,
    SwCell,
}

fn cond(v: &str) -> Option<Cond> {
    Some(match v {
        "wholeTable" => Cond::Whole,
        "band1Vert" => Cond::Band1Vert,
        "band2Vert" => Cond::Band2Vert,
        "band1Horz" => Cond::Band1Horz,
        "band2Horz" => Cond::Band2Horz,
        "firstCol" => Cond::FirstCol,
        "lastCol" => Cond::LastCol,
        "firstRow" => Cond::FirstRow,
        "lastRow" => Cond::LastRow,
        "neCell" => Cond::NeCell,
        "nwCell" => Cond::NwCell,
        "seCell" => Cond::SeCell,
        "swCell" => Cond::SwCell,
        _ => return None,
    })
}

#[derive(Debug, Clone, Default)]
pub struct Layer {
    pub ppr: ParaProps,
    pub rpr: RunProps,
    pub tbl: TblProps,
    pub tc: CellProps,
    pub tr: RowProps,
}

impl Layer {
    fn parse(e: &El, theme: &Theme) -> Layer {
        let mut l = Layer::default();
        if let Some(p) = e.child("pPr") {
            l.ppr = ParaProps::parse(p, theme);
        }
        if let Some(r) = e.child("rPr") {
            l.rpr = RunProps::parse(r, theme);
        }
        if let Some(t) = e.child("tblPr") {
            l.tbl = TblProps::parse(t, theme);
        }
        if let Some(t) = e.child("tcPr") {
            l.tc = CellProps::parse(t, theme);
        }
        if let Some(t) = e.child("trPr") {
            l.tr = RowProps::parse(t);
        }
        l
    }

    pub fn apply(&mut self, o: &Layer) {
        self.ppr.apply(&o.ppr);
        self.rpr.apply(&o.rpr);
        self.tbl.apply(&o.tbl);
        self.tc.apply(&o.tc);
        self.tr.apply(&o.tr);
    }
}

#[derive(Debug, Clone)]
struct Style {
    kind: Kind,
    based_on: Option<String>,
    base: Layer,
    conds: Vec<(Cond, Layer)>,
}

/// A table style flattened along its `basedOn` chain.
#[derive(Debug, Clone, Default)]
pub struct TableStyle {
    pub base: Layer,
    pub conds: HashMap<Cond, Layer>,
}

#[derive(Debug, Default)]
pub struct Styles {
    pub defaults_p: ParaProps,
    pub defaults_r: RunProps,
    map: HashMap<String, Style>,
    pub default_para: Option<String>,
    pub default_table: Option<String>,
    /// The document's language (`w:lang` in the default run properties), e.g. "en-US".
    pub lang: Option<String>,
    /// Paragraph style id → its numbering, for headings and list styles.
    para_cache: RefCell<HashMap<String, Rc<(ParaProps, RunProps)>>>,
    char_cache: RefCell<HashMap<String, Rc<RunProps>>>,
    table_cache: RefCell<HashMap<String, Rc<TableStyle>>>,
}

impl Styles {
    pub fn parse(root: &El, theme: &Theme) -> Styles {
        let mut s = Styles::default();
        if let Some(d) = root.child("docDefaults") {
            if let Some(p) = d.child("pPrDefault").and_then(|p| p.child("pPr")) {
                s.defaults_p = ParaProps::parse(p, theme);
            }
            if let Some(r) = d.child("rPrDefault").and_then(|r| r.child("rPr")) {
                s.defaults_r = RunProps::parse(r, theme);
                s.lang = r
                    .child("lang")
                    .and_then(|l| l.val())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string);
            }
        }
        for st in root.all("style") {
            let Some(id) = st.attr("styleId") else {
                continue;
            };
            let kind = match st.attr("type").unwrap_or("paragraph") {
                "character" => Kind::Char,
                "table" => Kind::Table,
                "numbering" => Kind::Numbering,
                _ => Kind::Para,
            };
            let default = matches!(st.attr("default"), Some("1" | "true" | "on"));
            if default {
                match kind {
                    Kind::Para => s.default_para = Some(id.to_string()),
                    Kind::Table => s.default_table = Some(id.to_string()),
                    _ => {}
                }
            }
            let conds = st
                .all("tblStylePr")
                .filter_map(|c| Some((cond(c.attr("type")?)?, Layer::parse(c, theme))))
                .collect();
            s.map.insert(
                id.to_string(),
                Style {
                    kind,
                    based_on: st
                        .child("basedOn")
                        .and_then(|b| b.val())
                        .map(str::to_string),
                    base: Layer::parse(st, theme),
                    conds,
                },
            );
        }
        s
    }

    /// The style and its ancestors, root first. Cycles and dangling ids are cut.
    fn chain(&self, id: &str) -> Vec<&Style> {
        let mut out = Vec::new();
        let mut cur = Some(id);
        while let Some(i) = cur {
            let Some(st) = self.map.get(i) else { break };
            if out.len() > 24 {
                break;
            }
            out.push(st);
            cur = st.based_on.as_deref().filter(|b| *b != i);
        }
        out.reverse();
        out
    }

    pub fn exists(&self, id: &str) -> bool {
        self.map.contains_key(id)
    }

    /// A paragraph style's properties, flattened (without document defaults).
    pub fn para(&self, id: &str) -> Rc<(ParaProps, RunProps)> {
        if let Some(c) = self.para_cache.borrow().get(id) {
            return c.clone();
        }
        let mut p = ParaProps::default();
        let mut r = RunProps::default();
        for st in self.chain(id) {
            if st.kind == Kind::Para {
                p.apply(&st.base.ppr);
                r.apply(&st.base.rpr);
            }
        }
        let rc = Rc::new((p, r));
        self.para_cache
            .borrow_mut()
            .insert(id.to_string(), rc.clone());
        rc
    }

    pub fn char(&self, id: &str) -> Rc<RunProps> {
        if let Some(c) = self.char_cache.borrow().get(id) {
            return c.clone();
        }
        let mut r = RunProps::default();
        for st in self.chain(id) {
            r.apply(&st.base.rpr);
        }
        let rc = Rc::new(r);
        self.char_cache
            .borrow_mut()
            .insert(id.to_string(), rc.clone());
        rc
    }

    pub fn table(&self, id: &str) -> Rc<TableStyle> {
        if let Some(c) = self.table_cache.borrow().get(id) {
            return c.clone();
        }
        let mut t = TableStyle::default();
        for st in self.chain(id) {
            t.base.apply(&st.base);
            for (c, l) in &st.conds {
                t.conds.entry(*c).or_default().apply(l);
            }
        }
        let rc = Rc::new(t);
        self.table_cache
            .borrow_mut()
            .insert(id.to_string(), rc.clone());
        rc
    }

    /// The numbering a numbering style points at (`numStyleLink`).
    pub fn numbering_style_num(&self, id: &str) -> Option<u32> {
        self.chain(id).iter().rev().find_map(|s| s.base.ppr.num_id)
    }
}
