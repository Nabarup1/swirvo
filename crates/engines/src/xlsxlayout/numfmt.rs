//! Excel number formats: what a cell's value looks like (`#,##0.00`, `0%`,
//! `d-mmm-yy`, `[Red](#,##0)`, `General`…).

use crate::docxlayout::props::Rgb;

/// The built-in formats by id (ECMA-376 §18.8.30), with US-English dates.
pub fn builtin(id: u32) -> Option<&'static str> {
    Some(match id {
        0 => "General",
        1 => "0",
        2 => "0.00",
        3 => "#,##0",
        4 => "#,##0.00",
        5 => "$#,##0_);($#,##0)",
        6 => "$#,##0_);[Red]($#,##0)",
        7 => "$#,##0.00_);($#,##0.00)",
        8 => "$#,##0.00_);[Red]($#,##0.00)",
        9 => "0%",
        10 => "0.00%",
        11 => "0.00E+00",
        12 => "# ?/?",
        13 => "# ??/??",
        14 => "m/d/yyyy",
        15 => "d-mmm-yy",
        16 => "d-mmm",
        17 => "mmm-yy",
        18 => "h:mm AM/PM",
        19 => "h:mm:ss AM/PM",
        20 => "h:mm",
        21 => "h:mm:ss",
        22 => "m/d/yyyy h:mm",
        37 => "#,##0 ;(#,##0)",
        38 => "#,##0 ;[Red](#,##0)",
        39 => "#,##0.00;(#,##0.00)",
        40 => "#,##0.00;[Red](#,##0.00)",
        45 => "mm:ss",
        46 => "[h]:mm:ss",
        47 => "mmss.0",
        48 => "##0.0E+0",
        49 => "@",
        _ => return None,
    })
}

/// Split at `;` outside quotes and brackets.
fn sections(code: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut start, mut quote, mut bracket, mut esc) = (0, false, false, false);
    for (i, c) in code.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '\\' if !quote => esc = true,
            '"' => quote = !quote,
            '[' if !quote => bracket = true,
            ']' if !quote => bracket = false,
            ';' if !quote && !bracket => {
                out.push(&code[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&code[start..]);
    out
}

fn color_name(n: &str) -> Option<Rgb> {
    Some(match n.to_ascii_lowercase().as_str() {
        "red" => Rgb(255, 0, 0),
        "blue" => Rgb(0, 0, 255),
        "green" => Rgb(0, 128, 0),
        "black" => Rgb(0, 0, 0),
        "white" => Rgb(255, 255, 255),
        "yellow" => Rgb(255, 255, 0),
        "magenta" => Rgb(255, 0, 255),
        "cyan" => Rgb(0, 255, 255),
        _ => return None,
    })
}

/// One section with its brackets taken out: colour, condition, elapsed-time flags.
struct Section {
    body: String,
    color: Option<Rgb>,
    cond: Option<(String, f64)>,
    elapsed: Option<char>,
}

fn parse_section(s: &str) -> Section {
    let mut body = String::new();
    let mut color = None;
    let mut cond = None;
    let mut elapsed = None;
    let mut chars = s.chars().peekable();
    let mut quote = false;
    while let Some(c) = chars.next() {
        if quote {
            body.push(c);
            if c == '"' {
                quote = false;
            }
            continue;
        }
        match c {
            '"' => {
                quote = true;
                body.push(c);
            }
            '\\' => {
                body.push(c);
                if let Some(n) = chars.next() {
                    body.push(n);
                }
            }
            '[' => {
                let mut inner = String::new();
                for n in chars.by_ref() {
                    if n == ']' {
                        break;
                    }
                    inner.push(n);
                }
                if let Some(col) = color_name(&inner) {
                    color = Some(col);
                } else if let Some(rest) = inner.strip_prefix('$') {
                    // [$€-407]: a currency symbol (before the dash) and a locale.
                    let sym = rest.split('-').next().unwrap_or("");
                    body.push('"');
                    body.push_str(sym);
                    body.push('"');
                } else if inner.starts_with(['<', '>', '=']) {
                    let op: String = inner.chars().take_while(|c| "<>=".contains(*c)).collect();
                    let v = inner[op.len()..].trim().parse::<f64>().unwrap_or(0.0);
                    cond = Some((op, v));
                } else if matches!(
                    inner.to_ascii_lowercase().chars().next(),
                    Some('h' | 'm' | 's')
                ) {
                    elapsed = inner.to_ascii_lowercase().chars().next();
                    body.push('\u{1}');
                }
            }
            _ => body.push(c),
        }
    }
    Section {
        body,
        color,
        cond,
        elapsed,
    }
}

fn test(cond: &(String, f64), v: f64) -> bool {
    match cond.0.as_str() {
        ">" => v > cond.1,
        ">=" => v >= cond.1,
        "<" => v < cond.1,
        "<=" => v <= cond.1,
        "=" => v == cond.1,
        "<>" => v != cond.1,
        _ => false,
    }
}

/// Excel's "General": up to 11 characters, integers as is, scientific when needed.
pub fn general(v: f64) -> String {
    if !v.is_finite() {
        return "#NUM!".into();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if (1e-10..1e11).contains(&a) {
        // The most decimals that fit in 11 characters (sign not counted).
        for d in (0..=10usize).rev() {
            let s = round_fmt(a, d);
            let s = if s.contains('.') {
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            } else {
                s
            };
            if s.len() <= 11 && s != "0" {
                return if v < 0.0 { format!("-{s}") } else { s };
            }
        }
    }
    let s = format!("{v:.5E}");
    // 1.23450E7 → 1.2345E+07
    let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
    let m = if m.contains('.') {
        m.trim_end_matches('0').trim_end_matches('.')
    } else {
        m
    };
    let e: i32 = e.parse().unwrap_or(0);
    format!("{m}E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
}

/// Fixed decimals, rounding halves away from zero as Excel does (Rust rounds them to
/// even).
fn round_fmt(a: f64, d: usize) -> String {
    let p = 10f64.powi(d as i32);
    let r = ((a * p) + 1e-7_f64.min(a * p * 1e-12).max(1e-9)).round() / p;
    format!("{r:.d$}")
}

/// Format `v` with `code`. Returns the text and the colour a section asks for.
pub fn format(v: f64, code: &str, date1904: bool) -> (String, Option<Rgb>) {
    if code.eq_ignore_ascii_case("general") || code.is_empty() {
        return (general(v), None);
    }
    let secs: Vec<Section> = sections(code).into_iter().map(parse_section).collect();
    // Pick the section: explicit conditions, else positive; negative; zero.
    let explicit = secs.iter().any(|s| s.cond.is_some());
    let (sec, signless) = if explicit {
        let i = secs
            .iter()
            .position(|s| s.cond.as_ref().is_some_and(|c| test(c, v)))
            .unwrap_or(secs.len().saturating_sub(1).min(2));
        (&secs[i], i > 0 && v < 0.0)
    } else {
        match (secs.len(), v) {
            (n, x) if n >= 3 && x == 0.0 => (&secs[2], true),
            (n, x) if n >= 2 && x < 0.0 => (&secs[1], true),
            _ => (&secs[0], false),
        }
    };
    let v = if signless { v.abs() } else { v };
    let body = &sec.body;
    if body.trim().eq_ignore_ascii_case("general") {
        return (general(v), sec.color);
    }
    let text = if is_datetime(body) || sec.elapsed.is_some() {
        format_date(v, body, date1904, sec.elapsed)
    } else {
        format_number(v, body)
    };
    (text, sec.color)
}

/// A text value through a format's fourth section (or one with `@`).
pub fn format_text(s: &str, code: &str) -> String {
    let secs = sections(code);
    let sec = match secs.len() {
        4.. => secs[3],
        _ if secs.iter().any(|x| x.contains('@')) => secs
            .iter()
            .find(|x| x.contains('@'))
            .copied()
            .unwrap_or("@"),
        _ => return s.to_string(),
    };
    let body = parse_section(sec).body;
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        match c {
            '@' => out.push_str(s),
            '"' => {
                for n in chars.by_ref() {
                    if n == '"' {
                        break;
                    }
                    out.push(n);
                }
            }
            '\\' => out.extend(chars.next()),
            '_' => {
                chars.next();
                out.push(' ');
            }
            '*' => {
                chars.next();
            }
            c => out.push(c),
        }
    }
    out
}

fn is_datetime(body: &str) -> bool {
    let mut quote = false;
    let mut esc = false;
    for c in body.chars() {
        if esc {
            esc = false;
            continue;
        }
        match c {
            '"' => quote = !quote,
            '\\' => esc = true,
            'y' | 'Y' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' | 'm' | 'M' if !quote => return true,
            _ => {}
        }
    }
    false
}

// ------------------------------------------------------------ numbers

fn format_number(v: f64, body: &str) -> String {
    // Fractions (`# ?/?`, `?/8`) are one unit, spaces and all.
    let unquoted: String = body.split('"').step_by(2).collect();
    if unquoted.contains('/') && unquoted.contains(['?', '#']) {
        return fraction(v, &unquoted);
    }
    // Literals and placeholders, in order.
    enum Tok {
        Lit(String),
        Digits,
    }
    let mut toks: Vec<Tok> = Vec::new();
    let mut pattern = String::new();
    let mut pct = 0;
    let mut chars = body.chars().peekable();
    let mut in_pattern = false;
    let push_lit = |toks: &mut Vec<Tok>, s: String| {
        if let Some(Tok::Lit(l)) = toks.last_mut() {
            l.push_str(&s);
        } else {
            toks.push(Tok::Lit(s));
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '0' | '#' | '?' | '.' | ',' | 'E' | 'e' | '/' => {
                if (c == 'E' || c == 'e') && !in_pattern {
                    push_lit(&mut toks, c.to_string());
                    continue;
                }
                if !in_pattern {
                    toks.push(Tok::Digits);
                    in_pattern = true;
                }
                pattern.push(c);
                if c == 'E' || c == 'e' {
                    // E+00 / E-00.
                    if let Some(&s) = chars.peek()
                        && (s == '+' || s == '-')
                    {
                        pattern.push(s);
                        chars.next();
                    }
                }
            }
            '%' => {
                pct += 1;
                push_lit(&mut toks, "%".into());
                in_pattern = in_pattern && false;
            }
            '"' => {
                let mut s = String::new();
                for n in chars.by_ref() {
                    if n == '"' {
                        break;
                    }
                    s.push(n);
                }
                push_lit(&mut toks, s);
                in_pattern = false;
            }
            '\\' => {
                push_lit(
                    &mut toks,
                    chars.next().map(String::from).unwrap_or_default(),
                );
                in_pattern = false;
            }
            '_' => {
                chars.next();
                push_lit(&mut toks, " ".into());
                in_pattern = false;
            }
            '*' => {
                chars.next();
            }
            '\u{1}' | '@' => {}
            c => {
                // A space or other literal between digit groups ends the pattern.
                push_lit(&mut toks, c.to_string());
                in_pattern = false;
            }
        }
    }
    let mut v = v * 100f64.powi(pct);
    let digits = render_digits(&mut v, &pattern);
    let mut out = String::new();
    for t in toks {
        match t {
            Tok::Lit(s) => out.push_str(&s),
            Tok::Digits => out.push_str(&digits),
        }
    }
    out
}

/// Render the number placeholders of one pattern (`#,##0.00`, `0.0E+00`, `# ?/?`).
fn render_digits(v: &mut f64, pat: &str) -> String {
    if pat.is_empty() {
        return String::new();
    }
    if let Some(e) = pat.find(['E', 'e']) {
        let (mant, exp) = pat.split_at(e);
        let exp_digits = exp.chars().filter(|c| *c == '0').count().max(1);
        let decimals = mant.split_once('.').map_or(0, |(_, d)| d.len());
        let int_digits = mant
            .split('.')
            .next()
            .unwrap_or("0")
            .chars()
            .filter(|c| matches!(c, '0' | '#' | '?'))
            .count()
            .max(1);
        if *v == 0.0 {
            return format!("{:.*}E+{:0w$}", decimals, 0.0, 0, w = exp_digits);
        }
        // Engineering-style when the integer part has several digits (##0.0E+0).
        let mut e10 = v.abs().log10().floor() as i32;
        if int_digits > 1 {
            e10 -= e10.rem_euclid(int_digits as i32);
        }
        let m = *v / 10f64.powi(e10);
        let sign = if e10 < 0 { '-' } else { '+' };
        return format!("{m:.decimals$}E{sign}{:0w$}", e10.abs(), w = exp_digits);
    }
    if pat.contains('/') {
        return fraction(*v, pat);
    }
    let thousands = {
        // A comma between digit placeholders groups thousands; trailing commas scale.
        let core = pat.trim_end_matches(',');
        let scale = pat.len() - core.len();
        *v /= 1000f64.powi(scale as i32);
        core.contains(',')
    };
    let core = pat.trim_end_matches(',').replace(',', "");
    let (ip, dp) = core.split_once('.').unwrap_or((&core, ""));
    let decimals = dp.len();
    let neg = *v < 0.0;
    let s = round_fmt(v.abs(), decimals);
    let (mut ints, frac) = match s.split_once('.') {
        Some((a, b)) => (a.to_string(), b.to_string()),
        None => (s.clone(), String::new()),
    };
    let min_int = ip.chars().filter(|c| *c == '0').count();
    if ints == "0" && min_int == 0 {
        ints.clear();
    }
    while ints.len() < min_int {
        ints.insert(0, '0');
    }
    if thousands && ints.len() > 3 {
        let b = ints.as_bytes();
        let mut g = String::new();
        for (i, ch) in b.iter().enumerate() {
            if i > 0 && (b.len() - i) % 3 == 0 {
                g.push(',');
            }
            g.push(*ch as char);
        }
        ints = g;
    }
    // Optional decimals (#) drop trailing zeros; `?` pads with spaces.
    let mut frac: Vec<char> = frac.chars().collect();
    let dpc: Vec<char> = dp.chars().collect();
    for (i, &p) in dpc.iter().enumerate().rev() {
        if i < frac.len() && frac[i] == '0' {
            match p {
                '#' => {
                    frac.truncate(i);
                    continue;
                }
                '?' => frac[i] = ' ',
                _ => {}
            }
        }
        break;
    }
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&ints);
    if !dp.is_empty() && (!frac.is_empty() || dp.starts_with('0')) {
        out.push('.');
        out.extend(frac);
    } else if pat.ends_with('.') {
        out.push('.');
    }
    out
}

fn fraction(v: f64, pat: &str) -> String {
    let denom_digits = pat.rsplit('/').next().map_or(1, |d| {
        d.chars()
            .filter(|c| matches!(c, '?' | '#' | '0'))
            .count()
            .max(1)
    });
    let max_den = 10u64.pow(denom_digits as u32) - 1;
    let whole = v.trunc();
    let f = (v - whole).abs();
    let (mut best_n, mut best_d, mut err) = (0u64, 1u64, f64::MAX);
    for d in 1..=max_den {
        let n = (f * d as f64).round() as u64;
        let e = (f - n as f64 / d as f64).abs();
        if e < err - 1e-12 {
            (best_n, best_d, err) = (n, d, e);
        }
    }
    let w = whole as i64;
    match (w, best_n) {
        (0, 0) => "0".into(),
        (_, 0) => w.to_string(),
        (0, n) => format!("{}{n}/{best_d}", if v < 0.0 { "-" } else { "" }),
        (w, n) => format!("{w} {n}/{best_d}"),
    }
}

// ------------------------------------------------------------ dates and times

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// A serial date → (year, month, day, weekday 0 = Sunday).
pub fn civil(serial: f64, date1904: bool) -> (i64, u32, u32, u32) {
    let serial = serial.floor() as i64;
    // Days since 1970-01-01. The 1900 system counts a phantom 1900-02-29 (serial 60).
    let days = if date1904 {
        serial - 24_107
    } else if serial >= 61 {
        serial - 25_569
    } else {
        serial - 25_568
    };
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    // 1970-01-01 was a Thursday. Excel's phantom leap day shifts its weekdays before
    // March 1900 by one, and we show what Excel shows.
    let shift = i64::from(!date1904 && serial < 61);
    let wd = (days + 4 - shift).rem_euclid(7) as u32;
    (y, m, d, wd)
}

fn format_date(v: f64, body: &str, date1904: bool, elapsed: Option<char>) -> String {
    let (y, mo, d, wd) = civil(v, date1904);
    let day_frac = v - v.floor();
    let total_secs = (day_frac * 86_400.0).round() as i64;
    let (mut hh, mm, ss) = (
        total_secs / 3600 % 24,
        total_secs / 60 % 60,
        total_secs % 60,
    );
    let ampm =
        body.to_ascii_uppercase().contains("AM/PM") || body.to_ascii_uppercase().contains("A/P");
    let pm = hh >= 12;
    if ampm {
        hh %= 12;
        if hh == 0 {
            hh = 12;
        }
    }
    let total_h = (v * 24.0).floor() as i64;
    let total_m = (v * 1440.0).floor() as i64;
    // Tokenize: runs of the same letter.
    let chars: Vec<char> = body.chars().collect();
    let mut toks: Vec<(char, usize)> = Vec::new();
    let mut lits: Vec<Option<String>> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let lc = c.to_ascii_lowercase();
        if c == '"' {
            let mut s = String::new();
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            i += 1;
            toks.push(('"', 0));
            lits.push(Some(s));
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            toks.push(('"', 0));
            lits.push(Some(chars[i + 1].to_string()));
            i += 2;
            continue;
        }
        if body[body.char_indices().nth(i).map_or(0, |x| x.0)..]
            .to_ascii_uppercase()
            .starts_with("AM/PM")
        {
            toks.push(('A', 5));
            lits.push(None);
            i += 5;
            continue;
        }
        if matches!(lc, 'y' | 'm' | 'd' | 'h' | 's') {
            let mut n = 1;
            while i + n < chars.len() && chars[i + n].to_ascii_lowercase() == lc {
                n += 1;
            }
            toks.push((lc, n));
            lits.push(None);
            i += n;
            continue;
        }
        if c == '\u{1}' {
            toks.push(('E', 0));
            lits.push(None);
            i += 1;
            continue;
        }
        if c == '.' && i + 1 < chars.len() && chars[i + 1] == '0' {
            let mut n = 0;
            while i + 1 + n < chars.len() && chars[i + 1 + n] == '0' {
                n += 1;
            }
            toks.push(('.', n));
            lits.push(None);
            i += 1 + n;
            continue;
        }
        toks.push(('"', 0));
        lits.push(Some(c.to_string()));
        i += 1;
    }
    // `m` means minutes right after hours or right before seconds.
    let kinds: Vec<char> = toks.iter().map(|t| t.0).collect();
    let is_minute = |k: usize| -> bool {
        let prev = kinds[..k]
            .iter()
            .rev()
            .find(|c| matches!(c, 'y' | 'm' | 'd' | 'h' | 's' | 'E'));
        let next = kinds[k + 1..]
            .iter()
            .find(|c| matches!(c, 'y' | 'm' | 'd' | 'h' | 's' | 'E'));
        matches!(prev, Some('h'))
            || matches!(next, Some('s'))
            || (matches!(prev, Some('E')) && elapsed == Some('h'))
    };
    let mut out = String::new();
    for (k, (c, n)) in toks.iter().enumerate() {
        match (c, n) {
            ('"', _) => out.push_str(lits[k].as_deref().unwrap_or("")),
            ('y', 1 | 2) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            ('y', _) => out.push_str(&format!("{y:04}")),
            ('m', n) if *n <= 2 && is_minute(k) => out.push_str(&if *n == 2 {
                format!("{mm:02}")
            } else {
                mm.to_string()
            }),
            ('m', 1) => out.push_str(&mo.to_string()),
            ('m', 2) => out.push_str(&format!("{mo:02}")),
            ('m', 3) => out.push_str(&MONTHS[(mo - 1) as usize][..3]),
            ('m', 5) => out.push_str(&MONTHS[(mo - 1) as usize][..1]),
            ('m', _) => out.push_str(MONTHS[(mo - 1) as usize]),
            ('d', 1) => out.push_str(&d.to_string()),
            ('d', 2) => out.push_str(&format!("{d:02}")),
            ('d', 3) => out.push_str(&DAYS[wd as usize][..3]),
            ('d', _) => out.push_str(DAYS[wd as usize]),
            ('h', 1) => out.push_str(&hh.to_string()),
            ('h', _) => out.push_str(&format!("{hh:02}")),
            ('s', 1) => out.push_str(&ss.to_string()),
            ('s', _) => out.push_str(&format!("{ss:02}")),
            ('.', n) => {
                let frac = day_frac * 86_400.0 - (day_frac * 86_400.0).floor();
                let s = format!("{frac:.n$}", n = *n);
                out.push_str(s.trim_start_matches('0'));
            }
            ('A', _) => out.push_str(if pm { "PM" } else { "AM" }),
            ('E', _) => out.push_str(&match elapsed {
                Some('h') => total_h.to_string(),
                Some('m') => total_m.to_string(),
                _ => ((v * 86_400.0).round() as i64).to_string(),
            }),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: f64, code: &str) -> String {
        format(v, code, false).0
    }

    #[test]
    fn numbers() {
        assert_eq!(f(1234.567, "#,##0.00"), "1,234.57");
        assert_eq!(f(1234.5, "0"), "1235");
        assert_eq!(f(0.256, "0.0%"), "25.6%");
        assert_eq!(f(-1234.0, "#,##0 ;[Red](#,##0)"), "(1,234)");
        assert_eq!(format(-5.0, "0;[Red]-0", false).1, Some(Rgb(255, 0, 0)));
        assert_eq!(f(0.0, "0.00;-0.00;\"-\""), "-");
        assert_eq!(f(1500.0, "\"$\"#,##0"), "$1,500");
        assert_eq!(f(1500.0, "[$€-407] #,##0.00"), "€ 1,500.00");
        assert_eq!(f(12345.678, "0.00E+00"), "1.23E+04");
        assert_eq!(f(0.5, "# ?/?"), "1/2");
        assert_eq!(f(2.75, "# ?/?"), "2 3/4");
        assert_eq!(f(3.0, "0.0#"), "3.0");
        assert_eq!(f(1_250_000.0, "#,##0,\"K\""), "1,250K");
        assert_eq!(f(7.0, "000"), "007");
    }

    #[test]
    fn general_is_excel_like() {
        assert_eq!(general(3.0), "3");
        assert_eq!(general(0.1 + 0.2), "0.3");
        assert_eq!(general(1.0 / 3.0), "0.333333333");
        assert_eq!(general(123_456_789_012.0), "1.23457E+11");
        assert_eq!(general(-42.5), "-42.5");
    }

    #[test]
    fn dates_and_times() {
        // 45000 = 2023-03-15 (a Wednesday) in the 1900 system.
        assert_eq!(civil(45_000.0, false), (2023, 3, 15, 3));
        assert_eq!(civil(1.0, false), (1900, 1, 1, 0));
        assert_eq!(civil(61.0, false), (1900, 3, 1, 4));
        assert_eq!(f(45_000.0, "m/d/yyyy"), "3/15/2023");
        assert_eq!(f(45_000.0, "d-mmm-yy"), "15-Mar-23");
        assert_eq!(
            f(45_000.0, "dddd, mmmm d, yyyy"),
            "Wednesday, March 15, 2023"
        );
        assert_eq!(f(45_000.75, "h:mm AM/PM"), "6:00 PM");
        assert_eq!(f(45_000.5, "yyyy-mm-dd hh:mm:ss"), "2023-03-15 12:00:00");
        assert_eq!(f(1.5, "[h]:mm:ss"), "36:00:00");
        // 1904 system: day 0 = 1904-01-01.
        assert_eq!(civil(0.0, true), (1904, 1, 1, 5));
        assert_eq!(format_text("abc", "0;-0;0;\"Name: \"@"), "Name: abc");
    }
}
