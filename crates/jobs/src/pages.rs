//! Page selections for the page picker and `--pages`: `"1-3, 7, 10-"` ⇄ 0-based indices.
//!
//! Grammar (case-insensitive, separators are commas and/or spaces):
//! `all` · `odd` · `even` · `N` · `N-M` · `N-` (to the end) · `-M` (from the start).
//! Pages are 1-based in text and 0-based in code. Results are sorted and deduplicated.

use std::collections::BTreeSet;

/// One term of a selection, 1-based as typed.
enum Term {
    All,
    Odd,
    Even,
    /// `N`, `N-M`, `N-` (to the end) or `-M` (from the start).
    Range(Option<usize>, Option<usize>),
}

/// Split `spec` into terms, checking only the grammar (no page count needed).
fn terms(spec: &str) -> Result<Vec<Term>, String> {
    if spec.len() > 4096 {
        return Err("that's too long".into());
    }
    // Dashes of any kind, and no spaces around them: "3 – 5" reads as "3-5".
    let mut norm = spec.replace(['–', '—'], "-").replace('\t', " ");
    while norm.contains(" -") || norm.contains("- ") {
        norm = norm.replace(" -", "-").replace("- ", "-");
    }
    let joined: Vec<&str> = norm
        .split([',', ';', ' '])
        .filter(|t| !t.is_empty())
        .collect();
    if joined.is_empty() {
        return Err("type page numbers, like 1-3, 7".into());
    }
    let num = |s: &str| -> Result<usize, String> {
        match s.parse::<usize>() {
            Ok(0) => Err("pages start at 1".into()),
            Ok(n) => Ok(n),
            Err(_) => Err(format!("“{s}” isn't a page number")),
        }
    };
    joined
        .into_iter()
        .map(|t| {
            let t = t.to_ascii_lowercase();
            Ok(match t.as_str() {
                "all" => Term::All,
                "odd" => Term::Odd,
                "even" => Term::Even,
                _ => match t.split_once('-') {
                    None => {
                        let n = num(&t)?;
                        Term::Range(Some(n), Some(n))
                    }
                    Some(("", "")) => return Err("“-” needs a page number".into()),
                    Some((a, b)) => Term::Range(
                        (!a.is_empty()).then(|| num(a)).transpose()?,
                        (!b.is_empty()).then(|| num(b)).transpose()?,
                    ),
                },
            })
        })
        .collect()
}

/// Is `spec` written correctly? (Page numbers are checked against a file by [`parse`].)
pub fn check(spec: &str) -> Result<(), String> {
    terms(spec).map(|_| ())
}

/// [`check`] for several groups ([`parse_groups`]).
pub fn check_groups(spec: &str) -> Result<(), String> {
    let parts = group_parts(spec)?;
    for (k, t) in parts.iter().enumerate() {
        check(t).map_err(|e| format!("PDF {}: {e}", k + 1))?;
    }
    Ok(())
}

/// Parse `spec` for a document of `count` pages.
pub fn parse(spec: &str, count: usize) -> Result<Vec<usize>, String> {
    let mut set = BTreeSet::new();
    let page = |n: usize| -> Result<usize, String> {
        if n > count {
            return Err(format!(
                "page {n} doesn't exist (there {} {count})",
                if count == 1 { "is" } else { "are" }
            ));
        }
        Ok(n - 1)
    };
    for t in terms(spec)? {
        match t {
            Term::All => set.extend(0..count),
            Term::Odd => set.extend((0..count).step_by(2)),
            Term::Even => set.extend((1..count).step_by(2)),
            Term::Range(a, b) => {
                let lo = match a {
                    Some(a) => page(a)?,
                    None => 0,
                };
                let hi = match b {
                    Some(b) => page(b)?,
                    None => count.checked_sub(1).ok_or("the document has no pages")?,
                };
                let (lo, hi) = (lo.min(hi), lo.max(hi));
                set.extend(lo..=hi);
            }
        }
    }
    Ok(set.into_iter().collect())
}

/// The groups of a split spec, trimmed, empty ones dropped.
fn group_parts(spec: &str) -> Result<Vec<&str>, String> {
    if spec.len() > 16_384 {
        return Err("that's too long".into());
    }
    let parts: Vec<&str> = spec
        .split([';', '|', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("type the pages for each PDF, like 1-15, 16; 25-40".into());
    }
    if parts.len() > 1000 {
        return Err("that's more than 1000 PDFs".into());
    }
    Ok(parts)
}

/// Compact text for a sorted selection: `[0,1,2,6,9,10,11]` → `"1-3, 7, 10-12"`.
pub fn format(sel: &[usize]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < sel.len() {
        let start = sel[i];
        let mut end = start;
        while i + 1 < sel.len() && sel[i + 1] == end + 1 {
            i += 1;
            end = sel[i];
        }
        if !out.is_empty() {
            out.push_str(", ");
        }
        if end == start {
            out.push_str(&(start + 1).to_string());
        } else {
            out.push_str(&format!("{}-{}", start + 1, end + 1));
        }
        i += 1;
    }
    out
}

/// Several selections, one per output file: `"1-15, 16, 18-20; 25-40, 42"`. A semicolon,
/// a `|` or a new line starts the next group; empty groups are skipped. Errors name the
/// group ("PDF 2: page 61 doesn't exist").
pub fn parse_groups(spec: &str, count: usize) -> Result<Vec<Vec<usize>>, String> {
    group_parts(spec)?
        .iter()
        .enumerate()
        .map(|(k, t)| parse(t, count).map_err(|e| format!("PDF {}: {e}", k + 1)))
        .collect()
}

/// Text for several groups, the inverse of [`parse_groups`].
pub fn format_groups(groups: &[Vec<usize>]) -> String {
    groups
        .iter()
        .map(|g| format(g))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Every page not in `sel` (sorted), for "Delete pages".
pub fn complement(sel: &[usize], count: usize) -> Vec<usize> {
    let s: BTreeSet<usize> = sel.iter().copied().collect();
    (0..count).filter(|i| !s.contains(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn grammar() {
        assert_eq!(parse("1-3, 7, 10-", 12).unwrap(), [0, 1, 2, 6, 9, 10, 11]);
        assert_eq!(parse("odd", 5).unwrap(), [0, 2, 4]);
        assert_eq!(parse("EVEN", 5).unwrap(), [1, 3]);
        assert_eq!(parse("-2, 5", 6).unwrap(), [0, 1, 4]);
        assert_eq!(parse("3 - 5", 6).unwrap(), [2, 3, 4]);
        assert_eq!(parse("5-3", 6).unwrap(), [2, 3, 4]);
        assert_eq!(parse("2–4", 6).unwrap(), [1, 2, 3]);
        assert_eq!(parse("all", 3).unwrap(), [0, 1, 2]);
        assert_eq!(parse("2,2,2", 3).unwrap(), [1]);
        assert!(parse("0", 3).is_err());
        assert!(parse("4", 3).unwrap_err().contains("doesn't exist"));
        assert!(parse("x", 3).is_err());
        assert!(parse("  ", 3).is_err());
    }

    #[test]
    fn formats_runs() {
        assert_eq!(format(&[0, 1, 2, 6, 9, 10, 11]), "1-3, 7, 10-12");
        assert_eq!(format(&[4]), "5");
        assert_eq!(format(&[]), "");
        assert_eq!(complement(&[1, 3], 5), [0, 2, 4]);
    }

    #[test]
    fn groups() {
        let g = parse_groups("1-15,16,18-20; 25-40, 42", 50).unwrap();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].len(), 19);
        assert_eq!(g[1], (24..40).chain([41]).collect::<Vec<_>>());
        assert_eq!(format_groups(&g), "1-16, 18-20; 25-40, 42");
        assert_eq!(parse_groups("1|2\n3;;", 3).unwrap(), [[0], [1], [2]]);
        assert!(parse_groups("1; 9", 5).unwrap_err().starts_with("PDF 2:"));
        assert!(parse_groups(" ; ", 5).is_err());
        assert!(check("1-3, 7, 10-, odd").is_ok());
        assert_eq!(check("0").unwrap_err(), "pages start at 1");
        assert!(check("2-x").unwrap_err().contains("“x”"));
        assert!(check("-").is_err());
        assert!(
            check_groups("1-2; 9999").is_ok(),
            "no page count: grammar only"
        );
        assert!(check_groups("1-2; b").unwrap_err().starts_with("PDF 2"));
    }

    proptest! {
        #[test]
        fn format_then_parse_round_trips(count in 1usize..300, picks in proptest::collection::vec(0usize..300, 0..60)) {
            let mut sel: Vec<usize> = picks.into_iter().filter(|&p| p < count).collect();
            sel.sort_unstable();
            sel.dedup();
            prop_assume!(!sel.is_empty());
            prop_assert_eq!(parse(&format(&sel), count).unwrap(), sel);
        }
    }
}
