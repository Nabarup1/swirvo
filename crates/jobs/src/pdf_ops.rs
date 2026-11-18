//! Running the PDF editing tools (docs/17) on files: options in, new files out, next to the
//! originals (never replacing anything), with each PDF's own permissions honoured.

use std::path::{Path, PathBuf};

use swirvo_core::{CancelToken, EngineError};
use swirvo_engines::os::certsign;
use swirvo_engines::pdfedit::{
    assemble, clean, compare, compress, forms, gray, redact, searchable,
    secure::{self, Need},
    sign::Visible,
    stamp,
};

use crate::Toolkit;
use crate::args::{Field, ToolArgs, Values};
use crate::tools::{
    Tool, ToolError, ToolResult, count, emit, human, into_folder, name_of, par_map, parent_of,
    workers,
};

const PAPER: &[(&str, &str)] = &[
    ("a4", "A4"),
    ("letter", "Letter"),
    ("a3", "A3"),
    ("a5", "A5"),
    ("legal", "Legal"),
];

const SPOTS: &[(&str, &str)] = &[
    ("bottom-center", "Bottom centre"),
    ("bottom-right", "Bottom right"),
    ("bottom-left", "Bottom left"),
    ("top-center", "Top centre"),
    ("top-right", "Top right"),
    ("top-left", "Top left"),
];

const PAGES_HINT: &str = "Which pages, e.g. 2- to skip the cover (empty: all)";

/// The options each PDF tool takes.
pub fn fields(tool: Tool) -> &'static [Field] {
    static COMPRESS: &[Field] = &[
        Field::choice(
            "level",
            "Quality",
            &[
                ("balanced", "Screen · 150 dpi"),
                ("light", "Print · 220 dpi"),
                ("strong", "Email · 100 dpi"),
                ("extreme", "Smallest · 72 dpi"),
            ],
            "balanced",
        ),
        Field::number("target_mb", "Fit under (MB)", "", 0.05, 100_000.0)
            .hint("Optional: keeps going smaller until it fits"),
    ];
    static NUMBERS: &[Field] = &[
        Field::choice(
            "style",
            "Style",
            &[
                ("page-of", "Page 1 of 9"),
                ("n/total", "1 / 9"),
                ("n", "1"),
                ("page", "Page 1"),
            ],
            "page-of",
        ),
        Field::choice("position", "Position", SPOTS, "bottom-center"),
        Field::number("start", "First number", "1", 0.0, 1_000_000.0),
        Field::number("size", "Text size (pt)", "10", 5.0, 48.0),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
    ];
    static HEADER: &[Field] = &[
        Field::text("header", "Header", "")
            .hint("{n} page number, {total} pages, {file} file name, {date} today"),
        Field::text("footer", "Footer", ""),
        Field::choice(
            "align",
            "Alignment",
            &[("center", "Centre"), ("left", "Left"), ("right", "Right")],
            "center",
        ),
        Field::number("size", "Text size (pt)", "9", 5.0, 48.0),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
    ];
    static BATES: &[Field] = &[
        Field::text("prefix", "Prefix", "").hint("e.g. ACME-"),
        Field::number("start", "First number", "1", 0.0, 1e12),
        Field::number("digits", "Digits", "6", 1.0, 12.0),
        Field::choice("position", "Position", SPOTS, "bottom-right"),
    ];
    static WATERMARK: &[Field] = &[
        Field::text("text", "Text", "").hint("e.g. DRAFT, CONFIDENTIAL"),
        Field::file("image", "Or a picture (logo)"),
        Field::choice(
            "strength",
            "Strength",
            &[("0.15", "Light"), ("0.25", "Medium"), ("0.4", "Strong")],
            "0.25",
        ),
        Field::choice(
            "color",
            "Colour",
            &[
                ("red", "Red"),
                ("gray", "Grey"),
                ("blue", "Blue"),
                ("black", "Black"),
            ],
            "red",
        ),
        Field::choice(
            "angle",
            "Angle",
            &[
                ("diagonal", "Diagonal"),
                ("0", "Flat"),
                ("90", "Upright"),
                ("-45", "Diagonal, falling"),
            ],
            "diagonal",
        )
        .hint("Text only"),
        Field::number("size", "Width (% of the page)", "", 5.0, 150.0)
            .hint("Empty: fills the diagonal (text) or half the page (picture)"),
        Field::text("place", "Centre", "")
            .hint("Drag it on the preview, or left, top in % (e.g. 50, 20)"),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
    ];
    static PROTECT: &[Field] = &[
        Field::secret("password", "Password to open").hint("Empty: anyone can open it"),
        Field::flag("print", "Allow printing", true),
        Field::flag("copy", "Allow copying text", true),
        Field::flag("edit", "Allow editing", true),
        Field::secret("owner", "Permissions password").hint("Lets you lift the limits later"),
    ];
    static UNLOCK: &[Field] = &[Field::secret("password", "Password")
        .hint("The open password; restricted files need the permissions password")];
    static TRIM: &[Field] = &[
        Field::number("pad", "Keep (pt)", "6", 0.0, 144.0).hint("Space left around the content"),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
    ];
    static CROP: &[Field] = &[
        Field::number("left", "Left (mm)", "10", 0.0, 1000.0),
        Field::number("right", "Right (mm)", "10", 0.0, 1000.0),
        Field::number("top", "Top (mm)", "10", 0.0, 1000.0),
        Field::number("bottom", "Bottom (mm)", "10", 0.0, 1000.0),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
        Field::text("custom", "Different crops", "")
            .hint("e.g. 3: 0, 0, 20, 20; 5-7: 5, 5, 5, 5 (mm left, right, top, bottom)"),
    ];
    static RESIZE: &[Field] = &[
        Field::choice("paper", "Paper", PAPER, "a4"),
        Field::pages("pages", "Pages").hint(PAGES_HINT),
    ];
    static NUP: &[Field] = &[
        Field::choice(
            "per_sheet",
            "Pages per sheet",
            &[("2", "2"), ("4", "4"), ("6", "6"), ("8", "8"), ("9", "9")],
            "2",
        ),
        Field::choice("paper", "Paper", PAPER, "a4"),
    ];
    static BOOKLET: &[Field] = &[Field::choice("paper", "Paper", PAPER, "a4")];
    static EVERY: &[Field] = &[Field::number(
        "every",
        "Pages per file",
        "5",
        1.0,
        100_000.0,
    )];
    static RANGES: &[Field] = &[Field::page_groups("ranges", "Pages for each PDF")
        .required()
        .hint("e.g. 1-15, 16, 18-20; 25-40, 42 (a ; starts the next PDF)")];
    static SIZE: &[Field] = &[
        Field::number("mb", "Largest part (MB)", "10", 0.05, 100_000.0)
            .hint("Email limits are often 10–25 MB"),
    ];
    static INFO: &[Field] = &[
        Field::text("title", "Title", "").hint("Empty fields stay as they are"),
        Field::text("author", "Author", ""),
        Field::text("subject", "Subject", ""),
        Field::text("keywords", "Keywords", ""),
    ];
    static FILL: &[Field] = &[
        Field::text("name_field", "Name files by column", "").hint("e.g. Name (empty: numbered)"),
        Field::flag("flatten", "Flatten (no longer editable)", false),
    ];
    static FLATTEN: &[Field] = &[Field::choice(
        "what",
        "Flatten",
        &[
            ("forms", "Form fields"),
            ("all", "Form fields and comments"),
        ],
        "forms",
    )];
    static SEARCHABLE: &[Field] = &[Field::flag("all", "Also pages that have text", false)
        .hint("Off: only scanned pages are read")];
    static REDACT: &[Field] = &[
        Field::text("find", "Words to remove", "")
            .hint("Separate with commas, e.g. Jane Doe, Acme, 4471. Only what Swirvo finds is removed: check the result before you share it."),
        Field::flag("emails", "Email addresses", false),
        Field::flag("phones", "Phone numbers", false),
        Field::flag("match_case", "Match case", false),
        Field::flag("searchable", "Keep the rest searchable", true)
            .hint("Reads the redacted pages back with OCR"),
    ];
    static COMPARE: &[Field] = &[Field::choice(
        "mode",
        "Compare",
        &[
            ("auto", "Automatic"),
            ("text", "Words"),
            ("visual", "Appearance"),
        ],
        "auto",
    )
    .hint("Automatic: words where both pages have text, appearance for scans and drawings")];
    static SIGN: &[Field] = &[
        Field::text("certificate", "Certificate", "")
            .hint("Part of its name (empty: your only signing certificate). Its legal effect depends on your certificate and the law where it's used."),
        Field::text("reason", "Reason", "").hint("e.g. Approved"),
        Field::text("location", "Location", ""),
        Field::choice(
            "box",
            "Signature box",
            &[
                ("last", "Last page, bottom right"),
                ("first", "First page, bottom right"),
                ("none", "None (invisible)"),
            ],
            "last",
        ),
        Field::text("place", "Box position", "")
            .hint("Drag the box on the preview, or page: left, top in mm (e.g. 2: 120, 240)"),
    ];
    match tool {
        Tool::PdfCompare => COMPARE,
        Tool::PdfSign => SIGN,
        Tool::PdfSearchable => SEARCHABLE,
        Tool::PdfFill => FILL,
        Tool::PdfFlatten => FLATTEN,
        Tool::PdfRedact => REDACT,
        Tool::PdfCompress => COMPRESS,
        Tool::PdfNumbers => NUMBERS,
        Tool::PdfHeaderFooter => HEADER,
        Tool::PdfBates => BATES,
        Tool::PdfWatermark => WATERMARK,
        Tool::PdfProtect => PROTECT,
        Tool::PdfUnlock => UNLOCK,
        Tool::PdfTrim => TRIM,
        Tool::PdfCrop => CROP,
        Tool::PdfResize => RESIZE,
        Tool::PdfNup => NUP,
        Tool::PdfBooklet => BOOKLET,
        Tool::PdfSplitEvery => EVERY,
        Tool::PdfSplitRanges => RANGES,
        Tool::PdfSplitSize => SIZE,
        Tool::PdfInfo => INFO,
        _ => &[],
    }
}

/// Tools that should show their options before running (from the wheel they open the
/// Board's form).
pub fn asks(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::PdfWatermark
            | Tool::PdfProtect
            | Tool::PdfUnlock
            | Tool::PdfHeaderFooter
            | Tool::PdfInfo
            | Tool::PdfCrop
            | Tool::PdfSplitEvery
            | Tool::PdfSplitRanges
            | Tool::PdfBates
            | Tool::PdfRedact
    )
}

fn need(tool: Tool) -> Option<Need> {
    Some(match tool {
        Tool::PdfCompress
        | Tool::PdfNumbers
        | Tool::PdfHeaderFooter
        | Tool::PdfBates
        | Tool::PdfWatermark
        | Tool::PdfTrim
        | Tool::PdfCrop
        | Tool::PdfResize
        | Tool::PdfClean
        | Tool::PdfInfo
        | Tool::PdfBookmarks
        | Tool::PdfAttach
        | Tool::PdfSearchable
        | Tool::PdfRedact
        | Tool::PdfGray
        | Tool::PdfFlatten
        | Tool::PdfSign => Need::Edit,
        Tool::PdfFormFields | Tool::PdfCompare => Need::Copy,
        Tool::PdfFill => Need::Edit,
        Tool::PdfImages | Tool::PdfAttachments => Need::Copy,
        Tool::PdfNup
        | Tool::PdfBooklet
        | Tool::PdfSplitEvery
        | Tool::PdfSplitRanges
        | Tool::PdfSplitChapters
        | Tool::PdfSplitSize
        | Tool::PdfRepair => Need::Everything,
        _ => return None,
    })
}

fn engine(e: EngineError) -> ToolError {
    ToolError::Engine(e)
}

fn paper(v: &Values<'_>) -> (f32, f32) {
    assemble::paper(v.str("paper")).unwrap_or(assemble::A4)
}

/// 0-based page selection from the `pages` option, against the file's page count.
fn pages_of(v: &Values<'_>, file: &Path) -> Result<Option<Vec<usize>>, ToolError> {
    let Some(spec) = v.opt("pages") else {
        return Ok(None);
    };
    let n = assemble::page_count(file).map_err(engine)?;
    crate::pages::parse(spec, n)
        .map(Some)
        .map_err(ToolError::Invalid)
}

fn today() -> String {
    // Days since 1970 → civil date (Howard Hinnant's algorithm), no time zone needed for a
    // date stamp.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Points per millimetre.
const MM: f32 = 72.0 / 25.4;

/// The text lines Numbers, Header and footer, and Bates stamp, and the pages they go on.
fn number_lines(
    tool: Tool,
    v: &Values<'_>,
    file: &Path,
) -> Result<(Vec<stamp::Line>, Option<Vec<usize>>), ToolError> {
    match tool {
        Tool::PdfNumbers => {
            let template = match v.str("style") {
                "n/total" => "{n} / {total}",
                "n" => "{n}",
                "page" => "Page {n}",
                _ => "Page {n} of {total}",
            };
            let line = stamp::Line {
                template: template.into(),
                spot: stamp::Spot::parse(v.str("position")).unwrap_or_default(),
                size: v.num("size").unwrap_or(10.0) as f32,
                start: v.num("start").unwrap_or(1.0) as usize,
                ..stamp::Line::default()
            };
            Ok((vec![line], pages_of(v, file)?))
        }
        Tool::PdfHeaderFooter => {
            let (h, f) = (v.opt("header"), v.opt("footer"));
            if h.is_none() && f.is_none() {
                return Err(ToolError::Invalid("type a header or a footer".into()));
            }
            let align = v.str("align");
            let spot = |top: bool| match (top, align) {
                (true, "left") => stamp::Spot::TopLeft,
                (true, "right") => stamp::Spot::TopRight,
                (true, _) => stamp::Spot::TopCenter,
                (false, "left") => stamp::Spot::BottomLeft,
                (false, "right") => stamp::Spot::BottomRight,
                (false, _) => stamp::Spot::BottomCenter,
            };
            let base = stamp::Line {
                size: v.num("size").unwrap_or(9.0) as f32,
                file: name_of(file),
                date: today(),
                ..stamp::Line::default()
            };
            let mut lines = Vec::new();
            for (text, top) in [(h, true), (f, false)] {
                if let Some(t) = text {
                    lines.push(stamp::Line {
                        template: t.into(),
                        spot: spot(top),
                        ..base.clone()
                    });
                }
            }
            Ok((lines, pages_of(v, file)?))
        }
        _ => {
            let prefix = v.str("prefix").replace('{', "(").replace('}', ")");
            let line = stamp::Line {
                template: format!("{prefix}{{n}}"),
                spot: stamp::Spot::parse(v.str("position")).unwrap_or(stamp::Spot::BottomRight),
                start: v.num("start").unwrap_or(1.0) as usize,
                digits: v.num("digits").unwrap_or(6.0) as usize,
                ..stamp::Line::default()
            };
            Ok((vec![line], None))
        }
    }
}

/// The watermark's place from its options: centre ("50, 20" in % from the top-left),
/// width in % of the page, angle.
fn watermark_place(v: &Values<'_>) -> Result<stamp::Placement, ToolError> {
    let mut p = stamp::Placement::default();
    if let Some(at) = v.opt("place") {
        let xy: Vec<f32> = at
            .split([',', ' ', '%'])
            .filter(|t| !t.is_empty())
            .map(|t| t.parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|_| ToolError::Invalid("Centre: write left, top in %, e.g. 50, 20".into()))?;
        match xy[..] {
            [x, y] if (0.0..=100.0).contains(&x) && (0.0..=100.0).contains(&y) => {
                p.center = [x / 100.0, y / 100.0];
            }
            _ => {
                return Err(ToolError::Invalid(
                    "Centre: two numbers from 0 to 100, e.g. 50, 20".into(),
                ));
            }
        }
    }
    p.width = v.num("size").map(|w| w as f32 / 100.0);
    p.angle = match v.str("angle") {
        "diagonal" | "" => None,
        a => a.parse::<f32>().ok(),
    };
    Ok(p)
}

/// Pages and their crop in mm [left, right, top, bottom].
pub type CustomCrops = Vec<(Vec<usize>, [f32; 4])>;

/// Per-page crops: `"3: 0, 0, 20, 20; 5-7: 5, 5, 5, 5"` → (pages, [left, right, top,
/// bottom] in mm). Later entries win over earlier ones and over the shared margins.
pub fn custom_crops(spec: &str, count: usize) -> Result<CustomCrops, ToolError> {
    let mut out = Vec::new();
    for part in spec
        .split([';', '\n'])
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let bad = || ToolError::Invalid(format!("“{part}”: write pages: left, right, top, bottom"));
        let (pages, nums) = part.split_once(':').ok_or_else(bad)?;
        let pages = crate::pages::parse(pages, count).map_err(ToolError::Invalid)?;
        let m: Vec<f32> = nums
            .split([',', ' '])
            .filter(|t| !t.is_empty())
            .map(|t| t.trim().parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|_| bad())?;
        let [l, r, t, b] = m[..] else {
            return Err(bad());
        };
        if [l, r, t, b]
            .iter()
            .any(|x| !x.is_finite() || *x < 0.0 || *x > 1000.0)
        {
            return Err(bad());
        }
        out.push((pages, [l, r, t, b]));
    }
    Ok(out)
}

/// Text for [`custom_crops`]: one entry per page, in mm with one decimal at most.
pub fn format_custom_crops(crops: &[(usize, [f32; 4])]) -> String {
    let num = |x: f32| {
        let r = (x * 10.0).round() / 10.0;
        if r.fract() == 0.0 {
            format!("{}", r as i64)
        } else {
            format!("{r:.1}")
        }
    };
    crops
        .iter()
        .map(|(p, m)| format!("{}: {}", p + 1, m.map(num).join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The visible signature box, width × height in points.
pub const SIGN_BOX: [f32; 2] = [220.0, 64.0];

/// Where the signature box goes: `"2: 120, 240"` → (page 1, [left, top] in mm from the
/// page's top-left corner as shown).
pub fn sign_place(spec: &str, count: usize) -> Result<(usize, [f32; 2]), ToolError> {
    let bad = || ToolError::Invalid("box position: write page: left, top (mm)".into());
    let (page, at) = spec.split_once(':').ok_or_else(bad)?;
    let page: usize = page.trim().parse().map_err(|_| bad())?;
    if page == 0 || page > count {
        return Err(ToolError::Invalid(format!(
            "box position: page {page} doesn't exist (there are {count})"
        )));
    }
    let xy: Vec<f32> = at
        .split([',', ' '])
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| bad())?;
    match xy[..] {
        [x, y] if x.is_finite() && y.is_finite() && x >= 0.0 && y >= 0.0 => Ok((page - 1, [x, y])),
        _ => Err(bad()),
    }
}

/// A tool's result for one page, made in a temporary folder for the Board's preview.
pub struct Preview {
    /// The PDF to show (the original when the tool leaves this page alone).
    pub pdf: PathBuf,
    /// Its page that matches the page asked for.
    pub page: usize,
    /// Its page count.
    pub pages: usize,
    _dir: Option<tempfile::TempDir>,
}

/// Tools the Board can preview on a page.
pub fn previewable(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::PdfWatermark
            | Tool::PdfCrop
            | Tool::PdfTrim
            | Tool::PdfResize
            | Tool::PdfGray
            | Tool::PdfRedact
            | Tool::PdfCompress
            | Tool::PdfNumbers
            | Tool::PdfHeaderFooter
            | Tool::PdfBates
            | Tool::PdfNup
            | Tool::PdfBooklet
    )
}

/// Tools whose pages don't match the input's one to one (the preview steps through
/// the output's sheets instead).
pub fn relayouts(tool: Tool) -> bool {
    matches!(tool, Tool::PdfNup | Tool::PdfBooklet)
}

/// Run `tool` for a preview of page `page` (0-based) of `file`. Tools that change pages
/// one at a time run on a copy of that page alone (fast on big files); numbering and
/// re-layout tools run on a copy of the whole file. Nothing is written next to `file`.
pub fn preview(
    tk: &Toolkit,
    tool: Tool,
    file: &Path,
    args: &ToolArgs,
    page: usize,
    cancel: &CancelToken,
) -> Result<Preview, ToolError> {
    if !previewable(tool) {
        return Err(ToolError::Invalid("this tool has no preview".into()));
    }
    let v = Values::new(fields(tool), args).map_err(ToolError::Invalid)?;
    if let Some(n) = need(tool) {
        secure::permits(file, n).map_err(engine)?;
    }
    let count = assemble::page_count(file).map_err(engine)?;
    let page = page.min(count.saturating_sub(1));
    let local = !matches!(tool, Tool::PdfNup | Tool::PdfBooklet);
    let unchanged = || Preview {
        pdf: file.to_path_buf(),
        page,
        pages: count,
        _dir: None,
    };
    let dir = tempfile::Builder::new()
        .prefix("swirvo-preview-")
        .tempdir()?;
    let src = dir
        .path()
        .join(file.file_name().unwrap_or("page.pdf".as_ref()));
    let mut a = args.clone();
    if matches!(
        tool,
        Tool::PdfNumbers | Tool::PdfHeaderFooter | Tool::PdfBates
    ) {
        // Number this page alone with the number and total it gets in the whole run, so a
        // 500-page file costs one page.
        let (mut lines, sel) = number_lines(tool, &v, file)?;
        let sel = sel.unwrap_or_else(|| (0..count).collect());
        let Some(k) = sel.iter().position(|&p| p == page) else {
            return Ok(unchanged());
        };
        for l in &mut lines {
            l.total = Some(l.start.saturating_sub(1) + sel.len());
            l.start += k;
        }
        assemble::keep_page(file, page, &src).map_err(engine)?;
        let out = dir.path().join("preview.pdf");
        stamp::add_lines(&src, &out, &lines, None, cancel).map_err(engine)?;
        return Ok(Preview {
            pdf: out,
            page: 0,
            pages: 1,
            _dir: Some(dir),
        });
    }
    if local {
        if let Some(sel) = pages_of(&v, file)?
            && !sel.contains(&page)
        {
            return Ok(unchanged());
        }
        if tool == Tool::PdfCrop {
            // This page's own crop, if it has one, replaces the shared margins.
            let mut m = ["left", "right", "top", "bottom"].map(|k| v.num(k).unwrap_or(10.0) as f32);
            for (pages, c) in custom_crops(v.str("custom"), count)? {
                if pages.contains(&page) {
                    m = c;
                }
            }
            for (k, x) in ["left", "right", "top", "bottom"].iter().zip(m) {
                a.insert(k.to_string(), x.to_string());
            }
            a.remove("custom");
            if m.iter().all(|&x| x <= 0.0) {
                return Ok(unchanged());
            }
        }
        a.remove("pages");
        if tool == Tool::PdfRedact {
            a.insert("searchable".into(), "false".into());
        }
        assemble::keep_page(file, page, &src).map_err(engine)?;
    } else {
        std::fs::copy(file, &src)?;
    }
    cancel.check().map_err(engine)?;
    let v = Values::new(fields(tool), &a).map_err(ToolError::Invalid)?;
    let (outs, _, _) = one(tk, tool, &src, &v, cancel)?;
    let pdf = outs
        .into_iter()
        .next()
        .ok_or_else(|| ToolError::Invalid("the tool made nothing".into()))?;
    let pages = assemble::page_count(&pdf).map_err(engine)?;
    let page = if local {
        0
    } else {
        page.min(pages.saturating_sub(1))
    };
    Ok(Preview {
        pdf,
        page,
        pages,
        _dir: Some(dir),
    })
}

/// Run a PDF tool. Per-file tools keep going after a failure and report it.
pub fn run(
    tk: &Toolkit,
    tool: Tool,
    files: &[PathBuf],
    args: &ToolArgs,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let v = Values::new(fields(tool), args).map_err(ToolError::Invalid)?;
    let mut r = ToolResult::default();
    let mut note: Option<String> = None;
    if tool == Tool::PdfFill {
        return fill(tk, files, &v, cancel);
    }
    if tool == Tool::PdfCompare {
        let mode = match v.str("mode") {
            "text" => compare::Mode::Text,
            "visual" => compare::Mode::Visual,
            _ => compare::Mode::Auto,
        };
        return compare_two(tk, files, mode, cancel);
    }
    if tool == Tool::PdfVerify {
        return verify(files);
    }
    if tool == Tool::PdfAttach {
        let (pdfs, others): (Vec<&PathBuf>, Vec<&PathBuf>) = files.iter().partition(|f| {
            tk.registry
                .from_path(f)
                .is_some_and(|id| tk.registry.key(id) == "pdf")
        });
        let [pdf] = pdfs.as_slice() else {
            return Err(ToolError::Invalid(
                "drop one PDF together with the files to attach".into(),
            ));
        };
        if others.is_empty() {
            return Err(ToolError::Invalid("add the files to attach".into()));
        }
        secure::permits(pdf, Need::Edit).map_err(engine)?;
        let others: Vec<PathBuf> = others.into_iter().cloned().collect();
        let stem = tk.registry.stem(pdf);
        let (out, n) = emit(
            &parent_of(pdf),
            &format!("{stem} (with attachments)"),
            "pdf",
            |tmp| clean::attach(pdf, &others, tmp).map(|_| ()),
        )?;
        r.message = format!("Attached {} files → {}", others.len(), name_of(&out));
        r.outputs.push(out);
        note = n;
        return Ok(finish(r, note));
    }
    // Several PDFs at once; redaction and OCR render pages at high resolution, so fewer.
    let cap = match tool {
        Tool::PdfRedact | Tool::PdfSearchable | Tool::PdfCompress => 3,
        Tool::PdfSign => 1,
        _ => 6,
    };
    let done = par_map(files, workers(files.len(), cap), cancel, |f| {
        one(tk, tool, f, &v, cancel)
    });
    cancel.check().map_err(engine)?;
    for (f, res) in files.iter().zip(done) {
        match res {
            Some(Ok((outs, msg, n))) => {
                r.outputs.extend(outs);
                r.message = msg;
                note = note.or(n);
            }
            Some(Err(e)) => r.failures.push((f.clone(), e.to_string())),
            None => {}
        }
    }
    if files.len() > 1 && !r.outputs.is_empty() {
        let (label, _) = tool.label();
        r.message = format!(
            "{label}: {} of {} PDFs done",
            files.len() - r.failures.len(),
            files.len()
        );
    }
    Ok(finish(r, note))
}

/// The signatures in each PDF, one line each, on the clipboard.
fn verify(files: &[PathBuf]) -> Result<ToolResult, ToolError> {
    let mut lines = Vec::new();
    let mut r = ToolResult::default();
    let (mut total, mut bad, mut unchecked) = (0, 0, 0);
    for f in files {
        match certsign::verify_pdf(f) {
            Ok(found) => {
                total += found.len();
                bad += found
                    .iter()
                    .filter(|(s, v)| !s.range_ok || v.as_ref().is_some_and(|v| !v.intact))
                    .count();
                unchecked += found.iter().filter(|(_, v)| v.is_none()).count();
                let rep = certsign::report(&found);
                if files.len() > 1 {
                    lines.push(format!("{}:", name_of(f)));
                }
                if rep.is_empty() {
                    lines.push("no signatures".into());
                }
                lines.extend(rep);
            }
            Err(e) => r.failures.push((f.clone(), e.to_string())),
        }
    }
    r.message = match (files.len(), total, bad) {
        (_, 0, _) => "No signatures".to_string(),
        (1, _, _) if lines.len() == 1 => lines[0].clone(),
        (_, n, 0) if unchecked == n => format!(
            "{} found; checking them needs Windows (list on the clipboard)",
            count(n, "signature")
        ),
        (_, n, 0) if unchecked > 0 => format!(
            "{} of {} intact, {unchecked} couldn't be checked (details on the clipboard)",
            n - unchecked,
            count(n, "signature")
        ),
        (_, n, 0) => format!(
            "{} intact (details on the clipboard)",
            count(n, "signature")
        ),
        (_, n, b) => format!(
            "{b} of {} INVALID (details on the clipboard)",
            count(n, "signature")
        ),
    };
    if total > 0 {
        r.clipboard = Some(lines.join("\r\n"));
    }
    Ok(r)
}

/// Two PDFs: the older one (by modification time) is the original.
fn compare_two(
    tk: &Toolkit,
    files: &[PathBuf],
    mode: compare::Mode,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let [a, b] = files else {
        return Err(ToolError::Invalid("compare takes two PDFs".into()));
    };
    for f in [a, b] {
        secure::permits(f, Need::Copy).map_err(engine)?;
    }
    let age = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let (old, new) = match (age(a), age(b)) {
        (Some(x), Some(y)) if y < x => (b, a),
        _ => (a, b),
    };
    let (so, sn) = (tk.registry.stem(old), tk.registry.stem(new));
    let mut rep = compare::Compared::default();
    let (out, note) = emit(
        &parent_of(new),
        &format!("{so} vs {sn} (changes)"),
        "pdf",
        |tmp| {
            rep = compare::compare_with(old, new, tmp, mode, cancel)?;
            Ok(())
        },
    )?;
    let mut what = Vec::new();
    if rep.added + rep.removed > 0 {
        what.push(format!(
            "{} added, {} removed",
            count(rep.added, "word"),
            rep.removed
        ));
    }
    if rep.regions > 0 {
        what.push(format!("{} changed", count(rep.regions, "area")));
    }
    let r = ToolResult {
        message: format!(
            "{} on {} (old left, new right) → {}",
            what.join(", "),
            count(rep.changed_pages, "page"),
            name_of(&out)
        ),
        outputs: vec![out],
        ..ToolResult::default()
    };
    Ok(finish(r, note))
}

/// A PDF form plus a data file: one filled PDF per row.
fn fill(
    tk: &Toolkit,
    files: &[PathBuf],
    v: &Values<'_>,
    cancel: &CancelToken,
) -> Result<ToolResult, ToolError> {
    let key = |f: &PathBuf| tk.registry.from_path(f).map(|id| tk.registry.key(id));
    let pdf = files
        .iter()
        .find(|f| key(f) == Some("pdf"))
        .ok_or_else(|| ToolError::Invalid("drop the PDF form with its data file".into()))?;
    let data = files
        .iter()
        .find(|f| key(f).is_some_and(|k| crate::doc_ops::DATA.contains(&k)))
        .ok_or_else(|| {
            ToolError::Invalid(
                "drop a spreadsheet or CSV with a column per field (Form fields makes one)".into(),
            )
        })?;
    secure::permits(pdf, Need::Edit).map_err(engine)?;
    let rows = swirvo_engines::docxtools::data_rows(data).map_err(engine)?;
    let stem = tk.registry.stem(pdf);
    let flat = v.flag("flatten");
    let (dir, made, note) = into_folder(pdf, &format!("{stem} (filled)"), |d| {
        forms::fill_rows(pdf, &rows, d, &stem, v.opt("name_field"), flat, cancel)
    })?;
    let r = ToolResult {
        message: format!("Filled {} forms → {}", made.len(), name_of(&dir)),
        outputs: vec![dir],
        ..ToolResult::default()
    };
    Ok(finish(r, note))
}

fn finish(mut r: ToolResult, note: Option<String>) -> ToolResult {
    if let Some(n) = note {
        r.message = format!("{} ({n})", r.message);
    }
    r
}

type One = (Vec<PathBuf>, String, Option<String>);

/// One output file `stem (suffix).pdf` written by `f`.
fn single(
    file: &Path,
    stem: &str,
    suffix: &str,
    f: impl FnOnce(&Path) -> Result<(), EngineError>,
) -> Result<(PathBuf, Option<String>), ToolError> {
    emit(&parent_of(file), &format!("{stem} ({suffix})"), "pdf", f)
}

fn one(
    tk: &Toolkit,
    tool: Tool,
    file: &Path,
    v: &Values<'_>,
    cancel: &CancelToken,
) -> Result<One, ToolError> {
    if let Some(n) = need(tool) {
        secure::permits(file, n).map_err(engine)?;
    }
    let stem = tk.registry.stem(file);
    let done = |out: PathBuf, msg: String, n: Option<String>| Ok((vec![out], msg, n));
    match tool {
        Tool::PdfSign => {
            let sizes = assemble::page_sizes(file).map_err(engine)?;
            let page = match v.str("box") {
                "none" => None,
                "first" => Some(0),
                _ => Some(sizes.len().saturating_sub(1)),
            };
            let place = match v.opt("place") {
                Some(p) => Some(sign_place(p, sizes.len())?),
                None => None,
            };
            let visible = match (page, place) {
                (None, _) => None,
                // Where it was put: the box's top-left corner, kept on the page.
                (Some(_), Some((p, [x, y]))) => {
                    let (w, h) = sizes[p];
                    let [bw, bh] = SIGN_BOX;
                    let x = (x * MM).clamp(0.0, (w - bw).max(0.0));
                    let top = (y * MM).clamp(0.0, (h - bh).max(0.0));
                    Some(Visible {
                        page: p,
                        rect: [x, h - top - bh, x + bw, h - top],
                    })
                }
                // 220 × 64 pt in the bottom-right corner, half an inch from the edges.
                (Some(p), None) => sizes.get(p).map(|&(w, _)| Visible {
                    page: p,
                    rect: [(w - 256.0).max(0.0), 36.0, (w - 36.0).max(220.0), 100.0],
                }),
            };
            let opts = certsign::SignOptions {
                certificate: v.opt("certificate").map(str::to_string),
                reason: v.opt("reason").map(str::to_string),
                location: v.opt("location").map(str::to_string),
                visible,
            };
            let mut used = None;
            let (out, n) = single(file, &stem, "signed", |tmp| {
                used = Some(certsign::sign_pdf(file, tmp, &opts, cancel)?);
                Ok(())
            })?;
            let who = used.map(|c| c.subject).unwrap_or_default();
            done(
                out.clone(),
                format!("Signed as {who} → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfSearchable => {
            let mut rep = searchable::Searchable::default();
            let (out, n) = single(file, &stem, "searchable", |tmp| {
                rep = searchable::make_searchable(file, tmp, v.flag("all"), cancel)?;
                Ok(())
            })?;
            let mut msg = format!(
                "Read {} on {} → {}",
                count(rep.words, "word"),
                count(rep.pages, "page"),
                name_of(&out)
            );
            if rep.skipped > 0 {
                msg.push_str(&format!(
                    " · {} already had text",
                    count(rep.skipped, "page")
                ));
            }
            done(out, msg, n)
        }
        Tool::PdfFormFields => {
            let mut count = 0;
            let (out, n) = emit(
                &parent_of(file),
                &format!("{stem} (fields)"),
                "csv",
                |tmp| {
                    count = forms::export_csv(file, tmp)?;
                    Ok(())
                },
            )?;
            done(
                out.clone(),
                format!(
                    "{count} fields → {} · add a row per copy, then drop it with the PDF on Fill form",
                    name_of(&out)
                ),
                n,
            )
        }
        Tool::PdfFlatten => {
            let mut count = 0;
            let all = v.str("what") == "all";
            let (out, n) = single(file, &stem, "flattened", |tmp| {
                count = forms::flatten_file(file, tmp, all)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!("Flattened {count} fields → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfGray => {
            let mut rep = gray::Grayed::default();
            let (out, n) = single(file, &stem, "grayscale", |tmp| {
                rep = gray::grayscale(file, tmp, cancel)?;
                Ok(())
            })?;
            let mut msg = format!("Grayscale → {}", name_of(&out));
            if rep.kept > 0 {
                msg.push_str(&format!(
                    " · {} pictures or gradients kept their colour",
                    rep.kept
                ));
            }
            done(out, msg, n)
        }
        Tool::PdfRedact => {
            let find = redact::Find {
                terms: v
                    .str("find")
                    .split([',', ';', '\n'])
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
                emails: v.flag("emails"),
                phones: v.flag("phones"),
                match_case: v.flag("match_case"),
            };
            let mut rep = redact::Redacted::default();
            let (out, n) = single(file, &stem, "redacted", |tmp| {
                rep = redact::redact(file, tmp, &find, &[], v.flag("searchable"), cancel)?;
                Ok(())
            })?;
            let mut msg = format!(
                "Removed {} on {} → {}",
                count(rep.hits, "match").replace("matchs", "matches"),
                count(rep.pages, "page"),
                name_of(&out)
            );
            if !rep.unplaced.is_empty() {
                let list: Vec<String> = rep.unplaced.iter().map(usize::to_string).collect();
                msg.push_str(&format!(
                    " · check page {}: a match there couldn't be located, so the page was flattened without a box",
                    list.join(", ")
                ));
            }
            done(out, msg, n)
        }
        Tool::PdfCompress => {
            let level = compress::Level::parse(v.str("level")).unwrap_or_default();
            let target = v.num("target_mb").map(|mb| (mb * 1_048_576.0) as u64);
            let mut report = compress::Report::default();
            let mut fits = true;
            let (out, n) = single(file, &stem, "compressed", |tmp| {
                match target {
                    Some(t) => {
                        let (rep, ok) = compress::compress_to(file, tmp, t, cancel)?;
                        report = rep;
                        fits = ok;
                    }
                    None => report = compress::compress(file, tmp, level, cancel)?,
                }
                if report.after as f64 > report.before as f64 * 0.98 && target.is_none() {
                    return Err(EngineError::Invalid(
                        "it's already compact; a smaller copy would look worse".into(),
                    ));
                }
                Ok(())
            })?;
            let pct = 100.0 - report.after as f64 * 100.0 / report.before.max(1) as f64;
            let mut msg = format!(
                "{} → {} (−{pct:.0}%) · {}",
                human(report.before),
                human(report.after),
                name_of(&out)
            );
            if !fits {
                msg.push_str(" · still over the limit; try splitting it");
            }
            done(out, msg, n)
        }
        Tool::PdfNumbers | Tool::PdfHeaderFooter | Tool::PdfBates => {
            let (lines, pages) = number_lines(tool, v, file)?;
            let suffix = match tool {
                Tool::PdfNumbers => "numbered",
                Tool::PdfBates => "Bates",
                _ => "stamped",
            };
            let mut count = 0;
            let (out, n) = single(file, &stem, suffix, |tmp| {
                count = stamp::add_lines(file, tmp, &lines, pages.as_deref(), cancel)?;
                Ok(())
            })?;
            let msg = match tool {
                Tool::PdfNumbers => format!("Numbered {count} pages → {}", name_of(&out)),
                Tool::PdfBates => {
                    let l = &lines[0];
                    let prefix = l.template.trim_end_matches("{n}");
                    let last = l.start + count.saturating_sub(1);
                    let d = l.digits;
                    format!(
                        "{prefix}{:0d$} – {prefix}{last:0d$} → {}",
                        l.start,
                        name_of(&out)
                    )
                }
                _ => format!("Added header/footer → {}", name_of(&out)),
            };
            done(out.clone(), msg, n)
        }
        Tool::PdfWatermark => {
            let opacity = v.num("strength").unwrap_or(0.25) as f32;
            let pages = pages_of(v, file)?;
            let place = watermark_place(v)?;
            let color = match v.str("color") {
                "gray" => [110, 110, 110],
                "blue" => [30, 70, 200],
                "black" => [0, 0, 0],
                _ => [200, 30, 30],
            };
            let (out, n) = match (v.opt("text"), v.opt("image")) {
                (_, Some(img)) => {
                    let bytes = std::fs::read(img)?;
                    single(file, &stem, "watermarked", |tmp| {
                        stamp::watermark_image(
                            file,
                            tmp,
                            bytes,
                            &place,
                            opacity,
                            pages.as_deref(),
                            cancel,
                        )
                        .map(|_| ())
                    })?
                }
                (Some(text), None) => single(file, &stem, "watermarked", |tmp| {
                    stamp::watermark_text(
                        file,
                        tmp,
                        text,
                        opacity,
                        color,
                        &place,
                        pages.as_deref(),
                        cancel,
                    )
                    .map(|_| ())
                })?,
                (None, None) => {
                    return Err(ToolError::Invalid(
                        "type the watermark text, or pick a picture".into(),
                    ));
                }
            };
            done(out.clone(), format!("Watermarked → {}", name_of(&out)), n)
        }
        Tool::PdfProtect => {
            let allow = secure::Allow {
                print: v.flag("print"),
                copy: v.flag("copy"),
                edit: v.flag("edit"),
            };
            let pw = v.str("password");
            let (out, n) = single(file, &stem, "protected", |tmp| {
                secure::protect(file, tmp, pw, v.opt("owner"), allow)
            })?;
            let what = if pw.is_empty() {
                "Restricted"
            } else {
                "Password-protected"
            };
            done(out.clone(), format!("{what} → {}", name_of(&out)), n)
        }
        Tool::PdfUnlock => {
            let pw = v.str("password");
            let (out, n) = single(file, &stem, "unlocked", |tmp| secure::unlock(file, tmp, pw))?;
            done(out.clone(), format!("Unlocked → {}", name_of(&out)), n)
        }
        Tool::PdfTrim => {
            let pages = pages_of(v, file)?;
            let pad = v.num("pad").unwrap_or(6.0) as f32;
            let mut count = 0;
            let (out, n) = single(file, &stem, "trimmed", |tmp| {
                count = assemble::auto_crop(file, tmp, pad, pages.as_deref(), cancel)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!("Trimmed {count} pages → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfCrop => {
            let mm = |k: &str| v.num(k).unwrap_or(10.0) as f32 * MM;
            let m = [mm("left"), mm("bottom"), mm("right"), mm("top")];
            let count = assemble::page_count(file).map_err(engine)?;
            let base = pages_of(v, file)?.unwrap_or_else(|| (0..count).collect());
            let mut plan: Vec<(usize, [f32; 4])> = base.into_iter().map(|i| (i, m)).collect();
            for (pages, [l, r, t, b]) in custom_crops(v.str("custom"), count)? {
                plan.extend(pages.into_iter().map(|i| (i, [l, b, r, t].map(|x| x * MM))));
            }
            let (out, n) = single(file, &stem, "cropped", |tmp| {
                assemble::crop_each(file, tmp, &plan).map(|_| ())
            })?;
            done(out.clone(), format!("Cropped → {}", name_of(&out)), n)
        }
        Tool::PdfResize => {
            let p = paper(v);
            let label = v
                .str("paper")
                .to_ascii_uppercase()
                .replace("LETTER", "Letter")
                .replace("LEGAL", "Legal");
            let pages = pages_of(v, file)?;
            let (out, n) = single(file, &stem, &label, |tmp| {
                assemble::resize(file, tmp, p, pages.as_deref()).map(|_| ())
            })?;
            done(
                out.clone(),
                format!("Resized to {label} → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfNup => {
            let k = v.num("per_sheet").unwrap_or(2.0) as usize;
            let mut sheets = 0;
            let (out, n) = single(file, &stem, &format!("{k}-up"), |tmp| {
                sheets = assemble::n_up(file, tmp, k, paper(v), cancel)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!("{k} per sheet, {sheets} sheets → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfBooklet => {
            let mut sides = 0;
            let (out, n) = single(file, &stem, "booklet", |tmp| {
                sides = assemble::booklet(file, tmp, paper(v), cancel)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!(
                    "{sides} sides → {} (print double-sided, flip on the short edge, fold)",
                    name_of(&out)
                ),
                n,
            )
        }
        Tool::PdfSplitRanges => {
            let n = assemble::page_count(file).map_err(engine)?;
            let groups =
                crate::pages::parse_groups(v.str("ranges"), n).map_err(ToolError::Invalid)?;
            let (dir, made, n) = into_folder(file, &format!("{stem} (parts)"), |d| {
                assemble::split_groups(file, d, &stem, &groups, cancel)
            })?;
            Ok((
                vec![dir.clone()],
                format!("{} PDFs → {}", made.len(), name_of(&dir)),
                n,
            ))
        }
        Tool::PdfSplitEvery | Tool::PdfSplitChapters | Tool::PdfSplitSize => {
            let folder = match tool {
                Tool::PdfSplitChapters => format!("{stem} (chapters)"),
                _ => format!("{stem} (parts)"),
            };
            let (dir, made, n) = into_folder(file, &folder, |d| match tool {
                Tool::PdfSplitEvery => {
                    let every = v.num("every").unwrap_or(5.0) as usize;
                    assemble::split_every(file, d, &stem, every, cancel)
                }
                Tool::PdfSplitChapters => assemble::split_by_bookmarks(file, d, &stem, cancel),
                _ => {
                    let max = (v.num("mb").unwrap_or(10.0) * 1_048_576.0) as u64;
                    assemble::split_by_size(file, d, &stem, max, cancel)
                }
            })?;
            Ok((
                vec![dir.clone()],
                format!("{} files → {}", made.len(), name_of(&dir)),
                n,
            ))
        }
        Tool::PdfClean => {
            let mut removed = clean::Removed::default();
            let (out, n) = single(file, &stem, "clean", |tmp| {
                removed = clean::sanitize(file, tmp)?;
                Ok(())
            })?;
            let what = if removed.items.is_empty() {
                "nothing hidden was found".to_string()
            } else {
                format!("removed {}", removed.items.join(", "))
            };
            done(
                out.clone(),
                format!("Cleaned: {what} → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfInfo => {
            let info = clean::Info {
                title: v.opt("title").map(str::to_string),
                author: v.opt("author").map(str::to_string),
                subject: v.opt("subject").map(str::to_string),
                keywords: v.opt("keywords").map(str::to_string),
            };
            if info.title.is_none()
                && info.author.is_none()
                && info.subject.is_none()
                && info.keywords.is_none()
            {
                return Err(ToolError::Invalid("fill in at least one field".into()));
            }
            let (out, n) = single(file, &stem, "edited", |tmp| {
                clean::set_info(file, tmp, &info)
            })?;
            done(
                out.clone(),
                format!("Properties saved → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfRepair => {
            let mut how = "";
            let (out, n) = single(file, &stem, "repaired", |tmp| {
                how = clean::repair(file, tmp)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!("Repaired ({how}) → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfBookmarks => {
            let mut count = 0;
            let (out, n) = single(file, &stem, "bookmarked", |tmp| {
                count = clean::bookmarks_from_headings(file, tmp, cancel)?;
                Ok(())
            })?;
            done(
                out.clone(),
                format!("{count} bookmarks → {}", name_of(&out)),
                n,
            )
        }
        Tool::PdfImages => {
            let (dir, made, n) = into_folder(file, &format!("{stem} (pictures)"), |d| {
                clean::extract_images(file, d, &stem, cancel)
            })?;
            Ok((
                vec![dir.clone()],
                format!("{} pictures → {}", made.len(), name_of(&dir)),
                n,
            ))
        }
        Tool::PdfAttachments => {
            let (dir, made, n) = into_folder(file, &format!("{stem} (attachments)"), |d| {
                clean::extract_attachments(file, d)
            })?;
            Ok((
                vec![dir.clone()],
                format!("{} files → {}", made.len(), name_of(&dir)),
                n,
            ))
        }
        other => Err(ToolError::Invalid(format!(
            "{} isn't a PDF tool",
            other.id()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::ToolArgs;

    fn pdf(dir: &Path, name: &str, pages: usize) -> PathBuf {
        let imgs: Vec<PathBuf> = (0..pages)
            .map(|i| {
                let p = dir.join(format!("{name}{i}.png"));
                image::RgbImage::from_fn(300, 400, |x, y| {
                    image::Rgb([(x % 256) as u8, (y % 256) as u8, (i * 40) as u8])
                })
                .save(&p)
                .unwrap();
                p
            })
            .collect();
        let out = dir.join(format!("{name}.pdf"));
        swirvo_engines::pdf_tools::images_to_pdf(&imgs, &out, &CancelToken::new()).unwrap();
        for p in imgs {
            std::fs::remove_file(p).unwrap();
        }
        out
    }

    fn args(pairs: &[(&str, &str)]) -> ToolArgs {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn previews_leave_no_files_behind() {
        let tk = Toolkit::with_builtin_engines().unwrap();
        let d = tempfile::tempdir().unwrap();
        let src = pdf(d.path(), "doc", 4);
        let c = CancelToken::new();
        let p = preview(
            &tk,
            Tool::PdfWatermark,
            &src,
            &args(&[("text", "DRAFT")]),
            2,
            &c,
        )
        .unwrap();
        assert_eq!((p.page, p.pages), (0, 1));
        assert!(p.pdf.exists() && !p.pdf.starts_with(d.path()));
        let p = preview(&tk, Tool::PdfNumbers, &src, &args(&[]), 3, &c).unwrap();
        assert_eq!((p.page, p.pages), (0, 1), "page 4 numbered on its own");
        let p = preview(&tk, Tool::PdfNup, &src, &args(&[("per_sheet", "4")]), 3, &c).unwrap();
        assert_eq!((p.page, p.pages), (0, 1));
        // A page the tool skips shows as it is.
        let p = preview(&tk, Tool::PdfGray, &src, &args(&[]), 2, &c);
        assert!(p.is_ok());
        let p = preview(
            &tk,
            Tool::PdfWatermark,
            &src,
            &args(&[("pages", "1")]),
            2,
            &c,
        )
        .unwrap();
        assert_eq!(p.pdf, src);
        let p = preview(&tk, Tool::PdfCrop, &src, &args(&[("pages", "1")]), 2, &c).unwrap();
        assert_eq!(p.pdf, src);
        drop(p);
        let names: Vec<_> = std::fs::read_dir(d.path()).unwrap().collect();
        assert_eq!(names.len(), 1, "only the original is there");
    }

    #[test]
    fn signature_box_positions() {
        assert_eq!(sign_place("2: 120, 240", 3).unwrap(), (1, [120.0, 240.0]));
        assert_eq!(sign_place(" 1 :0 0", 3).unwrap(), (0, [0.0, 0.0]));
        assert!(sign_place("4: 1, 1", 3).is_err());
        assert!(sign_place("1: -1, 1", 3).is_err());
        assert!(sign_place("1 1 1", 3).is_err());
    }

    #[test]
    fn custom_crop_text_round_trips() {
        let c = custom_crops("2: 0, 0, 20, 20; 5-6: 1.5 2 3 4", 9).unwrap();
        assert_eq!(c[1].0, [4, 5]);
        assert_eq!(c[1].1, [1.5, 2.0, 3.0, 4.0]);
        assert_eq!(
            format_custom_crops(&[(1, [0.0, 0.0, 20.0, 20.04]), (4, [1.5, 2.0, 3.0, 4.0])]),
            "2: 0, 0, 20, 20; 5: 1.5, 2, 3, 4"
        );
        assert!(custom_crops("2 0 0 0 0", 9).is_err());
        assert!(custom_crops("12: 0, 0, 0, 0", 9).is_err());
    }

    #[test]
    fn tools_write_new_files_and_keep_the_original() {
        let tk = Toolkit::with_builtin_engines().unwrap();
        let d = tempfile::tempdir().unwrap();
        let src = pdf(d.path(), "doc", 4);
        let before = std::fs::read(&src).unwrap();
        let c = CancelToken::new();
        let run1 =
            |t: Tool, a: &[(&str, &str)]| run(&tk, t, std::slice::from_ref(&src), &args(a), &c);

        let r = run1(Tool::PdfNumbers, &[]).unwrap();
        assert!(r.outputs[0].ends_with("doc (numbered).pdf"), "{r:?}");
        let r = run1(Tool::PdfBooklet, &[]).unwrap();
        assert!(r.message.contains("sides"), "{r:?}");
        let r = run1(
            Tool::PdfCrop,
            &[
                ("pages", "1"),
                ("custom", "2: 0, 0, 30, 0; 3-4: 5, 5, 5, 5"),
            ],
        )
        .unwrap();
        let boxes = assemble::page_sizes(&r.outputs[0]).unwrap();
        let orig = assemble::page_sizes(&src).unwrap();
        assert!(
            (orig[1].1 - boxes[1].1 - 30.0 * MM).abs() < 0.5,
            "{boxes:?}"
        );
        assert!(
            (orig[2].0 - boxes[2].0 - 10.0 * MM).abs() < 0.5,
            "{boxes:?}"
        );
        let bad = |t, a| run1(t, a).map_or(true, |r| !r.failures.is_empty());
        assert!(bad(Tool::PdfCrop, &[("custom", "2: 1, 2")]));
        let r = run1(Tool::PdfSplitRanges, &[("ranges", "1-2; 3, 1")]).unwrap();
        let parts: Vec<_> = std::fs::read_dir(&r.outputs[0]).unwrap().collect();
        assert_eq!(parts.len(), 2, "{}", r.message);
        assert!(bad(Tool::PdfSplitRanges, &[("ranges", "1; 99")]));
        let r = run1(Tool::PdfSplitEvery, &[("every", "3")]).unwrap();
        assert!(r.message.starts_with("2 files"), "{r:?}");
        let r = run1(Tool::PdfResize, &[("paper", "letter")]).unwrap();
        assert!(r.outputs[0].ends_with("doc (Letter).pdf"), "{r:?}");
        let r = run1(Tool::PdfProtect, &[("password", "pw")]).unwrap();
        let locked = r.outputs[0].clone();
        // A locked file refuses other tools until unlocked…
        let e = run(
            &tk,
            Tool::PdfNumbers,
            std::slice::from_ref(&locked),
            &ToolArgs::new(),
            &c,
        );
        assert!(e.unwrap().failures[0].1.contains("password"));
        let r = run(
            &tk,
            Tool::PdfUnlock,
            std::slice::from_ref(&locked),
            &args(&[("password", "pw")]),
            &c,
        )
        .unwrap();
        assert!(r.outputs[0].to_string_lossy().contains("unlocked"));
        // …options are checked…
        assert!(run1(Tool::PdfNup, &[("per_sheet", "3")]).is_err());
        assert!(run1(Tool::PdfNup, &[("sheets", "2")]).is_err());
        let r = run1(Tool::PdfWatermark, &[]).unwrap();
        assert!(r.failures[0].1.contains("watermark text"), "{r:?}");
        // …and the original never changes.
        assert_eq!(std::fs::read(&src).unwrap(), before);
        // A second run never replaces the first output.
        let r = run1(Tool::PdfNumbers, &[]).unwrap();
        assert!(r.outputs[0].ends_with("doc (numbered) (2).pdf"), "{r:?}");
    }

    #[test]
    fn attach_and_extract_round_trip() {
        let tk = Toolkit::with_builtin_engines().unwrap();
        let d = tempfile::tempdir().unwrap();
        let src = pdf(d.path(), "doc", 1);
        let note = d.path().join("note.txt");
        std::fs::write(&note, "hi").unwrap();
        let c = CancelToken::new();
        let r = run(
            &tk,
            Tool::PdfAttach,
            &[src.clone(), note],
            &ToolArgs::new(),
            &c,
        )
        .unwrap();
        let with = r.outputs[0].clone();
        let r = run(&tk, Tool::PdfAttachments, &[with], &ToolArgs::new(), &c).unwrap();
        let folder = &r.outputs[0];
        assert_eq!(std::fs::read(folder.join("note.txt")).unwrap(), b"hi");
    }

    #[test]
    fn date_is_iso() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }
}
