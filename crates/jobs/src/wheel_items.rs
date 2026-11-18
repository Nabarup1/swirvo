//! What the Convert wheel shows for a set of files: candidates from the graph's
//! reachability bitsets, ranked by frecency plus shipped priors, then placed into stable
//! slots (docs/04 §2–3).

use swirvo_core::{Category, FormatId, Preset};
use swirvo_wheel::{Frecency, assign_slots, geom::MAX_WEDGES};

use crate::Toolkit;

#[derive(Debug, Clone)]
pub struct WheelItem {
    pub target: FormatId,
    pub label: String,
    pub caption: String,
    pub lossless: bool,
    pub route: String,
}

/// Cold-start priors: what people usually want from each category, best first.
fn priors(c: Category) -> &'static [&'static str] {
    match c {
        Category::Image => &[
            "jpg", "png", "webp", "pdf", "gif", "ico", "tiff", "bmp", "qoi", "tga", "pnm",
        ],
        Category::Data => &["json", "yaml", "csv", "xlsx", "toml", "ndjson", "tsv"],
        Category::Spreadsheet => &["csv", "json", "xlsx", "tsv", "yaml"],
        Category::Subtitle => &["srt", "vtt", "txt", "ass", "sbv"],
        Category::Archive => &["zip", "tar.zst", "tar.gz", "tar"],
        Category::Document => &["html", "txt"],
        _ => &[],
    }
}

/// Wedge caption: what the target is good for (docs/02 §3.2).
fn purpose(key: &str, lossless: bool) -> &'static str {
    match key {
        "jpg" => "Universal",
        "webp" => {
            if lossless {
                "Web · lossless"
            } else {
                "Web"
            }
        }
        "png" => "Lossless",
        "pdf" => "Document",
        "gif" => "256 colors",
        "ico" => "App icon",
        "tiff" => "Archival",
        "bmp" | "tga" | "pnm" => "Legacy",
        "qoi" => "Fast lossless",
        "json" => "Data",
        "yaml" => "Readable",
        "toml" => "Config",
        "ndjson" => "Streaming",
        "csv" => "Spreadsheet",
        "tsv" => "Tabs",
        "xlsx" => "Excel workbook",
        "html" => "Web page",
        "txt" => "Plain text",
        "srt" => "Universal",
        "vtt" => "Web video",
        "ass" => "Styled",
        "sbv" => "SubViewer",
        "zip" => "Universal",
        "tar.zst" => "Fast · small",
        "tar.gz" => "Classic",
        "tar" => "Uncompressed",
        _ => {
            if lossless {
                "Lossless"
            } else {
                "Lossy"
            }
        }
    }
}

/// Every reachable target for `inputs`, best first (frecency + priors), excluding the
/// inputs' own formats. The Board's dropdowns list all of them.
pub fn ranked_targets(
    tk: &Toolkit,
    inputs: &[FormatId],
    frecency: &Frecency,
    now: u64,
) -> Vec<WheelItem> {
    rank(tk, inputs, frecency, now)
        .into_iter()
        .filter_map(|(key, _)| item(tk, inputs, &key))
        .collect()
}

fn rank(tk: &Toolkit, inputs: &[FormatId], frecency: &Frecency, now: u64) -> Vec<(String, f64)> {
    let reg = tk.registry;
    let candidates = tk.graph.common_targets(inputs);
    let ctx = inputs.first().map_or("", |&f| reg.key(f));
    let cat = inputs.first().map(|&f| reg.get(f).category);
    let prior = cat.map_or(&[][..], priors);
    let mut ranked: Vec<(String, f64)> = candidates
        .iter()
        .filter(|t| !inputs.contains(t))
        .map(|t| {
            let key = reg.key(t);
            let p = prior
                .iter()
                .position(|k| *k == key)
                .map_or(0.0, |i| 1.0 / (i as f64 + 2.0));
            (key.to_string(), frecency.score(ctx, key, now) + p)
        })
        .collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
}

fn item(tk: &Toolkit, inputs: &[FormatId], key: &str) -> Option<WheelItem> {
    let reg = tk.registry;
    let target = reg.id(key)?;
    let def = reg.get(target);
    let route = inputs
        .first()
        .and_then(|&f| tk.plan(f, target, Preset::Balanced, 1_000_000));
    let lossless = route.as_ref().is_some_and(|r| r.is_lossless());
    Some(WheelItem {
        target,
        label: def.exts[0].to_ascii_uppercase(),
        caption: purpose(def.key, lossless).to_string(),
        lossless,
        route: route.map(|r| tk.graph.explain(&r)).unwrap_or_default(),
    })
}

/// Up to 8 wedges; when more targets exist, the last slot is "More…" (caller renders it).
pub fn wheel_items(
    tk: &Toolkit,
    inputs: &[FormatId],
    frecency: &Frecency,
    now: u64,
) -> (Vec<WheelItem>, usize) {
    let ranked = rank(tk, inputs, frecency, now);
    let total = ranked.len();
    let capacity = if total > MAX_WEDGES {
        MAX_WEDGES - 1
    } else {
        total
    };
    let slots = assign_slots(capacity, &[], &[], &ranked, false);
    let items = slots
        .into_iter()
        .flatten()
        .filter_map(|key| item(tk, inputs, &key))
        .collect();
    (items, total.saturating_sub(capacity))
}
