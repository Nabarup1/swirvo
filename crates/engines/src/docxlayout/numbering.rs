//! `numbering.xml`: list definitions and the counters that turn them into labels
//! ("1.", "a)", "iv.", "•") in document order.

use std::collections::{HashMap, HashSet};

use super::props::{Align, ParaProps, RunProps};
use super::styles::Styles;
use super::theme::Theme;
use super::xml::El;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumFmt {
    #[default]
    Decimal,
    DecimalZero,
    UpperRoman,
    LowerRoman,
    UpperLetter,
    LowerLetter,
    Ordinal,
    Bullet,
    None,
}

pub fn num_fmt(v: &str) -> NumFmt {
    match v {
        "decimalZero" => NumFmt::DecimalZero,
        "upperRoman" => NumFmt::UpperRoman,
        "lowerRoman" => NumFmt::LowerRoman,
        "upperLetter" => NumFmt::UpperLetter,
        "lowerLetter" => NumFmt::LowerLetter,
        "ordinal" => NumFmt::Ordinal,
        "bullet" => NumFmt::Bullet,
        "none" => NumFmt::None,
        _ => NumFmt::Decimal,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Suffix {
    #[default]
    Tab,
    Space,
    Nothing,
}

#[derive(Debug, Clone, Default)]
pub struct Level {
    pub start: u32,
    pub fmt: NumFmt,
    pub text: String,
    pub jc: Align,
    pub suffix: Suffix,
    pub ppr: ParaProps,
    pub rpr: RunProps,
    /// `lvlRestart`: restart after this (1-based) level; 0 = never.
    pub restart: Option<u8>,
    pub legal: bool,
}

impl Level {
    fn parse(e: &El, theme: &Theme) -> Level {
        Level {
            start: e
                .child("start")
                .and_then(|s| s.val()?.parse().ok())
                .unwrap_or(1),
            fmt: e
                .child("numFmt")
                .and_then(|f| f.val())
                .map_or(NumFmt::Decimal, num_fmt),
            text: e
                .child("lvlText")
                .and_then(|t| t.val())
                .unwrap_or("")
                .to_string(),
            jc: match e.child("lvlJc").and_then(|j| j.val()) {
                Some("center") => Align::Center,
                Some("right" | "end") => Align::Right,
                _ => Align::Left,
            },
            suffix: match e.child("suff").and_then(|s| s.val()) {
                Some("space") => Suffix::Space,
                Some("nothing") => Suffix::Nothing,
                _ => Suffix::Tab,
            },
            ppr: e
                .child("pPr")
                .map(|p| ParaProps::parse(p, theme))
                .unwrap_or_default(),
            rpr: e
                .child("rPr")
                .map(|r| RunProps::parse(r, theme))
                .unwrap_or_default(),
            restart: e.child("lvlRestart").and_then(|r| r.val()?.parse().ok()),
            legal: e.child("isLgl").is_some_and(El::on),
        }
    }
}

#[derive(Debug, Default)]
struct Abstract {
    levels: [Option<Level>; 9],
    style_link: Option<String>,
}

#[derive(Debug, Default)]
struct Num {
    abs: u32,
    starts: HashMap<u8, u32>,
    levels: HashMap<u8, Level>,
}

#[derive(Debug, Default)]
pub struct Numbering {
    abstracts: HashMap<u32, Abstract>,
    nums: HashMap<u32, Num>,
}

/// List counters, shared by every `w:num` of one abstract definition (as in Word).
#[derive(Debug, Default)]
pub struct Counters {
    values: HashMap<u32, [u32; 9]>,
    used: HashMap<u32, [bool; 9]>,
    started: HashSet<u32>,
}

impl Numbering {
    pub fn parse(root: &El, theme: &Theme) -> Numbering {
        let mut n = Numbering::default();
        for a in root.all("abstractNum") {
            let Some(id) = a.attr("abstractNumId").and_then(|v| v.parse().ok()) else {
                continue;
            };
            let mut abs = Abstract {
                style_link: a
                    .child("numStyleLink")
                    .and_then(|s| s.val())
                    .map(str::to_string),
                ..Default::default()
            };
            for l in a.all("lvl") {
                if let Some(i) = l.attr("ilvl").and_then(|v| v.parse::<usize>().ok())
                    && i < 9
                {
                    abs.levels[i] = Some(Level::parse(l, theme));
                }
            }
            n.abstracts.insert(id, abs);
        }
        for m in root.all("num") {
            let Some(id) = m.attr("numId").and_then(|v| v.parse().ok()) else {
                continue;
            };
            let Some(abs) = m.child("abstractNumId").and_then(|a| a.val()?.parse().ok()) else {
                continue;
            };
            let mut num = Num {
                abs,
                ..Default::default()
            };
            for o in m.all("lvlOverride") {
                let Some(i) = o.attr("ilvl").and_then(|v| v.parse::<u8>().ok()) else {
                    continue;
                };
                if let Some(s) = o.child("startOverride").and_then(|s| s.val()?.parse().ok()) {
                    num.starts.insert(i, s);
                }
                if let Some(l) = o.child("lvl") {
                    num.levels.insert(i, Level::parse(l, theme));
                }
            }
            n.nums.insert(id, num);
        }
        n
    }

    /// The abstract definition a num uses, following one `numStyleLink` indirection.
    fn abstract_of(&self, num: u32, styles: &Styles) -> Option<(u32, &Abstract)> {
        let n = self.nums.get(&num)?;
        let a = self.abstracts.get(&n.abs)?;
        if a.levels.iter().all(Option::is_none)
            && let Some(link) = &a.style_link
            && let Some(other) = styles.numbering_style_num(link)
            && other != num
            && let Some(on) = self.nums.get(&other)
            && let Some(oa) = self.abstracts.get(&on.abs)
        {
            return Some((on.abs, oa));
        }
        Some((n.abs, a))
    }

    /// Advance the counters for a list paragraph and return its label and level.
    pub fn label(
        &self,
        counters: &mut Counters,
        num: u32,
        ilvl: u8,
        styles: &Styles,
    ) -> Option<(String, Level)> {
        let n = self.nums.get(&num)?;
        let (abs_id, abs) = self.abstract_of(num, styles)?;
        let ilvl = ilvl.min(8);
        let level_of = |i: usize| -> Option<Level> {
            n.levels
                .get(&(i as u8))
                .cloned()
                .or_else(|| abs.levels[i].clone())
        };
        let level = level_of(ilvl as usize)?;
        let vals = counters.values.entry(abs_id).or_insert([0; 9]);
        let used = counters.used.entry(abs_id).or_insert([false; 9]);
        if counters.started.insert(num) {
            for (&i, &s) in &n.starts {
                if (i as usize) < 9 {
                    vals[i as usize] = s;
                    used[i as usize] = false;
                }
            }
        }
        let start = |i: usize| {
            n.starts
                .get(&(i as u8))
                .copied()
                .or_else(|| level_of(i).map(|l| l.start))
                .unwrap_or(1)
        };
        let i = ilvl as usize;
        if used[i] {
            vals[i] += 1;
        } else {
            vals[i] = start(i);
            used[i] = true;
        }
        for (d, u) in used.iter_mut().enumerate().skip(i + 1) {
            let restart = level_of(d).and_then(|l| l.restart);
            let resets = match restart {
                None => true,
                Some(0) => false,
                Some(k) => (i as u8) < k,
            };
            if resets {
                *u = false;
            }
        }
        let mut text = String::new();
        let mut chars = level.text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%'
                && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
                && (1..=9).contains(&d)
            {
                chars.next();
                let l = d as usize - 1;
                let v = if used[l] { vals[l] } else { start(l) };
                let fmt = if level.legal && l != i {
                    NumFmt::Decimal
                } else {
                    level_of(l).map_or(NumFmt::Decimal, |x| x.fmt)
                };
                text.push_str(&format_number(v, fmt));
                continue;
            }
            text.push(c);
        }
        if level.fmt == NumFmt::None && !level.text.contains('%') && level.text.is_empty() {
            text.clear();
        }
        Some((text, level))
    }
}

pub fn format_number(v: u32, fmt: NumFmt) -> String {
    match fmt {
        NumFmt::Decimal | NumFmt::Bullet => v.to_string(),
        NumFmt::DecimalZero => format!("{v:02}"),
        NumFmt::UpperRoman => roman(v),
        NumFmt::LowerRoman => roman(v).to_lowercase(),
        NumFmt::UpperLetter => letters(v),
        NumFmt::LowerLetter => letters(v).to_lowercase(),
        NumFmt::Ordinal => {
            let suffix = match (v % 10, v % 100) {
                (1, x) if x != 11 => "st",
                (2, x) if x != 12 => "nd",
                (3, x) if x != 13 => "rd",
                _ => "th",
            };
            format!("{v}{suffix}")
        }
        NumFmt::None => String::new(),
    }
}

fn roman(mut v: u32) -> String {
    if v == 0 || v > 3999 {
        return v.to_string();
    }
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut s = String::new();
    for (n, r) in table {
        while v >= n {
            s.push_str(r);
            v -= n;
        }
    }
    s
}

/// Word's letters: A…Z, then AA…ZZ, AAA… (repeated, not base 26).
fn letters(v: u32) -> String {
    if v == 0 {
        return String::new();
    }
    let n = (v - 1) / 26 + 1;
    let c = (b'A' + ((v - 1) % 26) as u8) as char;
    std::iter::repeat_n(c, n as usize).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format_number(14, NumFmt::UpperRoman), "XIV");
        assert_eq!(format_number(3, NumFmt::LowerLetter), "c");
        assert_eq!(format_number(28, NumFmt::UpperLetter), "BB");
        assert_eq!(format_number(22, NumFmt::Ordinal), "22nd");
        assert_eq!(format_number(7, NumFmt::DecimalZero), "07");
    }

    #[test]
    fn counters_nest_and_restart() {
        let x = r#"<w:numbering xmlns:w="w">
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
            <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"#;
        let n = Numbering::parse(&super::super::xml::parse(x).unwrap(), &Theme::default());
        let st = Styles::default();
        let mut c = Counters::default();
        let mut l = |lvl| n.label(&mut c, 1, lvl, &st).unwrap().0;
        assert_eq!(l(0), "1.");
        assert_eq!(l(1), "1.a)");
        assert_eq!(l(1), "1.b)");
        assert_eq!(l(0), "2.");
        assert_eq!(l(1), "2.a)");
    }
}
