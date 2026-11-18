//! Subtitles through a small IR: SRT, WebVTT, YouTube SBV and ASS/SSA in; the same plus a
//! plain transcript out. Legacy encodings are fixed on the way (docs/05 §8).

use std::fmt::Write as _;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

use crate::text::read_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: u64,
    pub end_ms: u64,
    /// Plain text; lines separated by '\n'.
    pub text: String,
}

pub struct SubtitleEngine;

const IN: &[&str] = &["srt", "vtt", "sbv", "ass"];
const OUT: &[&str] = &["srt", "vtt", "sbv", "ass", "txt"];

impl Engine for SubtitleEngine {
    fn id(&self) -> &'static str {
        "subtitles"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in IN {
            for &to in OUT {
                if from != to {
                    let cap = Capability::new("subtitles", from, to).time(2, 5);
                    caps.push(match (from, to) {
                        (_, "txt") => cap.loss(10).note("transcript, timestamps removed"),
                        ("ass", _) => cap.loss(3).note("styling removed"),
                        _ => cap,
                    });
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let src = read_text(req.input)?;
        let cues = parse(&src, req.from)?;
        if cues.is_empty() {
            return Err(EngineError::Decode("no subtitle cues found".into()));
        }
        std::fs::write(req.output, render(&cues, req.to)?)?;
        Ok(())
    }
}

/// Parse "HH:MM:SS,mmm", "MM:SS.mmm", "H:MM:SS.cc" (ASS centiseconds) and friends.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim();
    let (hms, frac) = match s.rfind([',', '.']) {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, "0"),
    };
    let parts: Vec<u64> = hms
        .split(':')
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        _ => return None,
    };
    let digits = frac.len().min(3);
    let f: u64 = frac.get(..digits)?.parse().ok()?;
    let ms = f * 10u64.pow(3 - digits as u32);
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

/// Remove `<i>`, `<font …>` and `{\an8}`-style tags, keeping the text between them.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut angle, mut brace) = (false, false);
    for c in s.chars() {
        match c {
            '<' => angle = true,
            '>' if angle => angle = false,
            '{' => brace = true,
            '}' if brace => brace = false,
            _ if angle || brace => {}
            _ => out.push(c),
        }
    }
    out
}

pub fn parse(src: &str, from: &str) -> Result<Vec<Cue>, EngineError> {
    let src = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    match from {
        "srt" | "vtt" => {
            for block in src.split("\n\n") {
                let mut lines = block.lines().skip_while(|l| l.trim().is_empty());
                let mut timing = None;
                for l in lines.by_ref() {
                    if l.contains("-->") {
                        timing = Some(l);
                        break;
                    }
                }
                let Some(t) = timing else { continue };
                let bad = || EngineError::Decode(format!("bad timing line: {t}"));
                let (a, b) = t.split_once("-->").ok_or_else(bad)?;
                // VTT may append cue settings after the end time.
                let b = b.split_whitespace().next().unwrap_or("");
                let (Some(start_ms), Some(end_ms)) = (parse_time(a), parse_time(b)) else {
                    return Err(bad());
                };
                let text = lines.map(strip_tags).collect::<Vec<_>>().join("\n");
                cues.push(Cue {
                    start_ms,
                    end_ms,
                    text,
                });
            }
        }
        "sbv" => {
            for block in src.split("\n\n") {
                let mut lines = block.lines().skip_while(|l| l.trim().is_empty());
                let Some(t) = lines.next() else { continue };
                let Some((a, b)) = t.split_once(',') else {
                    continue;
                };
                let (Some(start_ms), Some(end_ms)) = (parse_time(a), parse_time(b)) else {
                    continue;
                };
                cues.push(Cue {
                    start_ms,
                    end_ms,
                    text: lines.collect::<Vec<_>>().join("\n"),
                });
            }
        }
        "ass" => {
            let mut fields: Vec<String> = Vec::new();
            for line in src.lines() {
                let line = line.trim();
                if let Some(f) = line.strip_prefix("Format:") {
                    fields = f
                        .split(',')
                        .map(|s| s.trim().to_ascii_lowercase())
                        .collect();
                } else if let Some(d) = line.strip_prefix("Dialogue:") {
                    let n = if fields.is_empty() { 10 } else { fields.len() };
                    let parts: Vec<&str> = d.splitn(n, ',').collect();
                    let idx = |name: &str, dflt: usize| {
                        fields.iter().position(|f| f == name).unwrap_or(dflt)
                    };
                    let (si, ei, ti) = (idx("start", 1), idx("end", 2), idx("text", n - 1));
                    let (Some(s), Some(e), Some(t)) = (parts.get(si), parts.get(ei), parts.get(ti))
                    else {
                        continue;
                    };
                    let (Some(start_ms), Some(end_ms)) = (parse_time(s), parse_time(e)) else {
                        continue;
                    };
                    let text = strip_tags(
                        &t.replace("\\N", "\n")
                            .replace("\\n", "\n")
                            .replace("\\h", " "),
                    );
                    cues.push(Cue {
                        start_ms,
                        end_ms,
                        text,
                    });
                }
            }
        }
        other => {
            return Err(EngineError::Invalid(format!(
                "subtitle engine can't read {other}"
            )));
        }
    }
    cues.sort_by_key(|c| (c.start_ms, c.end_ms));
    Ok(cues)
}

fn ts(ms: u64, sep: char, hours_width: usize) -> String {
    let (h, m, s, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h:0hours_width$}:{m:02}:{s:02}{sep}{f:03}")
}

pub fn render(cues: &[Cue], to: &str) -> Result<String, EngineError> {
    let mut out = String::new();
    match to {
        "srt" => {
            for (i, c) in cues.iter().enumerate() {
                let _ = write!(
                    out,
                    "{}\n{} --> {}\n{}\n\n",
                    i + 1,
                    ts(c.start_ms, ',', 2),
                    ts(c.end_ms, ',', 2),
                    c.text
                );
            }
        }
        "vtt" => {
            out.push_str("WEBVTT\n\n");
            for c in cues {
                // A blank line would end the cue early, so VTT text can't contain one.
                let text = c
                    .text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = write!(
                    out,
                    "{} --> {}\n{}\n\n",
                    ts(c.start_ms, '.', 2),
                    ts(c.end_ms, '.', 2),
                    text
                );
            }
        }
        "sbv" => {
            for c in cues {
                let _ = write!(
                    out,
                    "{},{}\n{}\n\n",
                    ts(c.start_ms, '.', 1),
                    ts(c.end_ms, '.', 1),
                    c.text
                );
            }
        }
        "ass" => {
            out.push_str("[Script Info]\nScriptType: v4.00+\nPlayResX: 1920\nPlayResY: 1080\n\n[V4+ Styles]\n");
            out.push_str("Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
            out.push_str("Style: Default,Arial,64,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,3,1,2,60,60,50,1\n\n[Events]\n");
            out.push_str(
                "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
            );
            let cs = |ms: u64| {
                format!(
                    "{}:{:02}:{:02}.{:02}",
                    ms / 3_600_000,
                    ms / 60_000 % 60,
                    ms / 1000 % 60,
                    ms % 1000 / 10
                )
            };
            for c in cues {
                let _ = writeln!(
                    out,
                    "Dialogue: 0,{},{},Default,,0,0,0,,{}",
                    cs(c.start_ms),
                    cs(c.end_ms),
                    c.text.replace('\n', "\\N")
                );
            }
        }
        "txt" => {
            let mut last = "";
            for c in cues {
                for l in c.text.lines().map(str::trim).filter(|l| !l.is_empty()) {
                    if l != last {
                        out.push_str(l);
                        out.push('\n');
                        last = l;
                    }
                }
            }
        }
        other => {
            return Err(EngineError::Invalid(format!(
                "subtitle engine can't write {other}"
            )));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,500\r\n<i>Hello</i>\r\nthere\r\n\r\n2\r\n01:02:03,040 --> 01:02:04,000\r\nBye\r\n";

    #[test]
    fn srt_round_trips_through_every_format() {
        let cues = parse(SRT, "srt").unwrap();
        assert_eq!(
            cues[0],
            Cue {
                start_ms: 1000,
                end_ms: 2500,
                text: "Hello\nthere".into()
            }
        );
        assert_eq!(cues[1].start_ms, 3_723_040);
        for fmt in ["srt", "vtt", "sbv", "ass"] {
            let back = parse(&render(&cues, fmt).unwrap(), fmt).unwrap();
            assert_eq!(back.len(), 2, "{fmt}");
            assert_eq!(back[0].text, "Hello\nthere", "{fmt}");
            // ASS stores centiseconds.
            assert_eq!(back[1].start_ms / 10, cues[1].start_ms / 10, "{fmt}");
        }
        assert_eq!(render(&cues, "txt").unwrap(), "Hello\nthere\nBye\n");
    }

    #[test]
    fn vtt_with_settings_and_short_times() {
        let cues = parse(
            "WEBVTT\n\n00:01.000 --> 00:02.000 align:start\n{\\an8}Hi\n",
            "vtt",
        )
        .unwrap();
        assert_eq!(
            cues,
            vec![Cue {
                start_ms: 1000,
                end_ms: 2000,
                text: "Hi".into()
            }]
        );
    }
}
