//! A small typesetter: [`Doc`] → paginated A4 PDF (krilla), measured with the same fonts it
//! embeds (subset). Lines are filled greedily by real glyph advances; headings keep with the
//! next lines; tables repeat their header row on new pages. Characters missing from the UI
//! font fall back per character to CJK, Indic and symbol fonts when the system has them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ab_glyph::{Font as _, FontArc, PxScale, ScaleFont as _};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect};
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::text::{Font, TextDirection};
use swirvo_core::EngineError;

use super::{Block, Doc, Inline, Style};

const PAGE_W: f32 = 595.28;
const PAGE_H: f32 = 841.89;
const MARGIN_X: f32 = 64.0;
const TOP: f32 = 64.0;
const BOTTOM: f32 = PAGE_H - 64.0;
const WIDTH: f32 = PAGE_W - 2.0 * MARGIN_X;
const BODY: f32 = 10.5;
const INK: (u8, u8, u8) = (26, 28, 33);
const MUTED: (u8, u8, u8) = (85, 90, 100);
const LINK: (u8, u8, u8) = (5, 99, 193);
const LINE: (u8, u8, u8) = (200, 203, 210);

// ---------------------------------------------------------------- fonts

struct Face {
    pdf: Font,
    metrics: FontArc,
}

impl Face {
    fn load(path: &Path) -> Option<Face> {
        let bytes = Arc::new(std::fs::read(path).ok()?);
        let metrics = FontArc::try_from_vec(bytes.to_vec()).ok()?;
        let pdf = Font::new(krilla::Data::from(bytes), 0)?;
        Some(Face { pdf, metrics })
    }

    fn has(&self, c: char) -> bool {
        self.metrics.glyph_id(c).0 != 0
    }

    fn width(&self, text: &str, size: f32) -> f32 {
        let f = self.metrics.as_scaled(PxScale::from(size));
        let mut w = 0.0;
        let mut prev = None;
        for c in text.chars() {
            let id = f.glyph_id(c);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id);
            prev = Some(id);
        }
        // ab_glyph scales by em height in px; PDF sizes are em sizes, so convert.
        w * self.em_ratio()
    }

    fn em_ratio(&self) -> f32 {
        let units = self.metrics.units_per_em().unwrap_or(1000.0);
        let height = self.metrics.ascent_unscaled() - self.metrics.descent_unscaled();
        if height > 0.0 { height / units } else { 1.0 }
    }
}

fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR") {
        dirs.push(PathBuf::from(w).join("Fonts"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(
            PathBuf::from(l)
                .join("Microsoft")
                .join("Windows")
                .join("Fonts"),
        );
    }
    for d in [
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/truetype/liberation2",
        "/usr/share/fonts/truetype/ubuntu",
        "/usr/share/fonts/truetype/noto",
        "/usr/share/fonts/opentype/noto",
        "/usr/share/fonts/noto",
        "/usr/share/fonts/noto-cjk",
        "/usr/share/fonts/TTF",
        "/usr/share/fonts/truetype/droid",
        "/mnt/c/Windows/Fonts",
    ] {
        dirs.push(PathBuf::from(d));
    }
    dirs
}

fn find(dirs: &[PathBuf], names: &[&str]) -> Option<Face> {
    names.iter().find_map(|n| {
        dirs.iter()
            .map(|d| d.join(n))
            .find(|p| p.is_file())
            .and_then(|p| Face::load(&p))
    })
}

/// Regular, bold, italic, bold-italic, mono, then lazily loaded fallbacks.
struct Fonts {
    faces: Vec<Face>,
    fallback_names: Vec<&'static str>,
    dirs: Vec<PathBuf>,
}

const REGULAR: usize = 0;
const BOLD: usize = 1;
const ITALIC: usize = 2;
const BOLD_ITALIC: usize = 3;
const MONO: usize = 4;

impl Fonts {
    fn load() -> Result<Fonts, EngineError> {
        let dirs = font_dirs();
        let regular = find(
            &dirs,
            &[
                "segoeui.ttf",
                "arial.ttf",
                "DejaVuSans.ttf",
                "LiberationSans-Regular.ttf",
                "Ubuntu-R.ttf",
                "NotoSans-Regular.ttf",
            ],
        )
        .ok_or_else(|| EngineError::Encode("no system font found to typeset the PDF".into()))?;
        let pick = |names: &[&str]| find(&dirs, names);
        let bold = pick(&[
            "segoeuib.ttf",
            "arialbd.ttf",
            "DejaVuSans-Bold.ttf",
            "LiberationSans-Bold.ttf",
            "Ubuntu-B.ttf",
            "NotoSans-Bold.ttf",
        ]);
        let italic = pick(&[
            "segoeuii.ttf",
            "ariali.ttf",
            "DejaVuSans-Oblique.ttf",
            "LiberationSans-Italic.ttf",
            "Ubuntu-RI.ttf",
            "NotoSans-Italic.ttf",
        ]);
        let bold_italic = pick(&[
            "segoeuiz.ttf",
            "arialbi.ttf",
            "DejaVuSans-BoldOblique.ttf",
            "LiberationSans-BoldItalic.ttf",
            "Ubuntu-BI.ttf",
            "NotoSans-BoldItalic.ttf",
        ]);
        let mono = pick(&[
            "consola.ttf",
            "cour.ttf",
            "DejaVuSansMono.ttf",
            "LiberationMono-Regular.ttf",
            "UbuntuMono-R.ttf",
            "NotoSansMono-Regular.ttf",
        ]);
        let clone = |f: &Face| Face {
            pdf: f.pdf.clone(),
            metrics: f.metrics.clone(),
        };
        let bold = bold.unwrap_or_else(|| clone(&regular));
        let italic = italic.unwrap_or_else(|| clone(&regular));
        let bold_italic = bold_italic.unwrap_or_else(|| clone(&bold));
        let mono = mono.unwrap_or_else(|| clone(&regular));
        Ok(Fonts {
            faces: vec![regular, bold, italic, bold_italic, mono],
            fallback_names: vec![
                "seguisym.ttf",
                "msyh.ttc",
                "YuGothM.ttc",
                "malgun.ttf",
                "Nirmala.ttf",
                "ebrima.ttf",
                "NotoSansCJK-Regular.ttc",
                "DroidSansFallbackFull.ttf",
                "DejaVuSans.ttf",
                "NotoSansSymbols2-Regular.ttf",
            ],
            dirs,
        })
    }

    fn style_face(s: &Style) -> usize {
        match (s.code, s.bold, s.italic) {
            (true, _, _) => MONO,
            (_, true, true) => BOLD_ITALIC,
            (_, true, false) => BOLD,
            (_, false, true) => ITALIC,
            _ => REGULAR,
        }
    }

    /// Face for `c`: the styled face if it has the glyph, else the first fallback that does.
    fn face_for(&mut self, c: char, base: usize) -> usize {
        if c.is_whitespace() || self.faces[base].has(c) {
            return base;
        }
        if let Some(i) = (MONO + 1..self.faces.len()).find(|&i| self.faces[i].has(c)) {
            return i;
        }
        while !self.fallback_names.is_empty() {
            let name = self.fallback_names.remove(0);
            if let Some(f) = find(&self.dirs, &[name]) {
                let has = f.has(c);
                self.faces.push(f);
                if has {
                    return self.faces.len() - 1;
                }
            }
        }
        base
    }
}

// ---------------------------------------------------------------- layout

#[derive(Clone)]
struct Run {
    face: usize,
    size: f32,
    text: String,
    width: f32,
    color: (u8, u8, u8),
    underline: bool,
}

/// A laid-out line: runs with x offsets relative to the line start.
#[derive(Clone, Default)]
struct LaidLine {
    runs: Vec<(f32, Run)>,
}

enum Op {
    /// A picture with its top-left corner at (x, y).
    Image {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        img: krilla::image::Image,
    },
    Text {
        face: usize,
        size: f32,
        x: f32,
        y: f32,
        text: String,
        color: (u8, u8, u8),
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: (u8, u8, u8),
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        width: f32,
        color: (u8, u8, u8),
    },
}

struct Typesetter {
    fonts: Fonts,
    pages: Vec<Vec<Op>>,
    y: f32,
    /// Each picture embedded once, however often it appears.
    images: std::collections::HashMap<*const Vec<u8>, Option<krilla::image::Image>>,
}

#[derive(Clone, Copy)]
struct Look {
    size: f32,
    leading: f32,
    bold: bool,
    italic: bool,
    color: (u8, u8, u8),
}

impl Look {
    const fn body() -> Look {
        Look {
            size: BODY,
            leading: BODY * 1.45,
            bold: false,
            italic: false,
            color: INK,
        }
    }
}

impl Typesetter {
    fn page(&mut self) -> &mut Vec<Op> {
        if self.pages.is_empty() {
            self.pages.push(Vec::new());
        }
        let n = self.pages.len() - 1;
        &mut self.pages[n]
    }

    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = TOP;
    }

    fn page_is_empty(&self) -> bool {
        self.pages.last().is_none_or(Vec::is_empty)
    }

    /// Make room for `h` points, starting a new page if needed.
    fn ensure(&mut self, h: f32) {
        if self.y + h > BOTTOM && !self.page_is_empty() {
            self.new_page();
        }
    }

    fn gap(&mut self, h: f32) {
        if !self.page_is_empty() {
            self.y += h;
        }
    }

    /// Break styled text into lines no wider than `width`.
    fn wrap(&mut self, inl: &[Inline], look: Look, width: f32) -> Vec<LaidLine> {
        // Tokens: (text, style, is_space, is_newline) split at spaces and newlines.
        let mut lines: Vec<LaidLine> = vec![LaidLine::default()];
        let mut x = 0.0f32;
        for i in inl {
            let style = Style {
                bold: i.style.bold || look.bold,
                italic: i.style.italic || look.italic,
                ..i.style.clone()
            };
            let base = Fonts::style_face(&style);
            let size = if style.code {
                look.size * 0.9
            } else {
                look.size
            };
            let color = if style.link.is_some() {
                LINK
            } else {
                look.color
            };
            let underline = style.link.is_some();
            let mut word = String::new();
            let mut tokens: Vec<String> = Vec::new();
            for c in i.text.chars() {
                if c == ' ' || c == '\n' || c == '\t' {
                    if !word.is_empty() {
                        tokens.push(std::mem::take(&mut word));
                    }
                    tokens.push(if c == '\t' {
                        "    ".to_string()
                    } else {
                        c.to_string()
                    });
                } else {
                    word.push(c);
                }
            }
            if !word.is_empty() {
                tokens.push(word);
            }
            for tok in tokens {
                if tok == "\n" {
                    lines.push(LaidLine::default());
                    x = 0.0;
                    continue;
                }
                let is_space = tok.chars().all(|c| c == ' ');
                if is_space && x == 0.0 {
                    continue; // no leading spaces on a wrapped line
                }
                let runs = self.shape(&tok, base, size, color, underline);
                let w: f32 = runs.iter().map(|r| r.width).sum();
                if x + w > width && x > 0.0 && !is_space {
                    lines.push(LaidLine::default());
                    x = 0.0;
                }
                if w > width && !is_space {
                    // A single word wider than the column: break it by characters.
                    for c in tok.chars() {
                        let r = self.shape(&c.to_string(), base, size, color, underline);
                        let cw: f32 = r.iter().map(|r| r.width).sum();
                        if x + cw > width && x > 0.0 {
                            lines.push(LaidLine::default());
                            x = 0.0;
                        }
                        for run in r {
                            let w = run.width;
                            if let Some(l) = lines.last_mut() {
                                l.runs.push((x, run));
                            }
                            x += w;
                        }
                    }
                    continue;
                }
                for run in runs {
                    let w = run.width;
                    if let Some(l) = lines.last_mut() {
                        l.runs.push((x, run));
                    }
                    x += w;
                }
            }
        }
        // Trailing spaces don't count; then merge neighbours that share a look into one
        // text run, so the PDF has real word spacing (better copy/paste and smaller files).
        for l in &mut lines {
            while l.runs.last().is_some_and(|(_, r)| r.text.trim().is_empty()) {
                l.runs.pop();
            }
            let mut merged: Vec<(f32, Run)> = Vec::with_capacity(l.runs.len());
            for (x, r) in l.runs.drain(..) {
                match merged.last_mut() {
                    Some((_, m))
                        if m.face == r.face
                            && m.size == r.size
                            && m.color == r.color
                            && m.underline == r.underline =>
                    {
                        m.text.push_str(&r.text);
                        m.width += r.width;
                    }
                    _ => merged.push((x, r)),
                }
            }
            l.runs = merged;
        }
        lines
    }

    /// Split `text` into runs by font coverage.
    fn shape(
        &mut self,
        text: &str,
        base: usize,
        size: f32,
        color: (u8, u8, u8),
        underline: bool,
    ) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        for c in text.chars() {
            let face = self.fonts.face_for(c, base);
            match runs.last_mut() {
                Some(r) if r.face == face => r.text.push(c),
                _ => runs.push(Run {
                    face,
                    size,
                    text: c.to_string(),
                    width: 0.0,
                    color,
                    underline,
                }),
            }
        }
        for r in &mut runs {
            r.width = self.fonts.faces[r.face].width(&r.text, size);
        }
        runs
    }

    fn place(&mut self, lines: &[LaidLine], x0: f32, look: Look, marker: Option<&str>, bar: bool) {
        for (k, line) in lines.iter().enumerate() {
            self.ensure(look.leading);
            let baseline = self.y + look.size * 1.02;
            if k == 0
                && let Some(m) = marker
            {
                let runs = self.shape(m, REGULAR, look.size, look.color, false);
                let w: f32 = runs.iter().map(|r| r.width).sum();
                let mut x = x0 - w - 6.0;
                for r in runs {
                    let rw = r.width;
                    self.page().push(Op::Text {
                        face: r.face,
                        size: r.size,
                        x,
                        y: baseline,
                        text: r.text,
                        color: r.color,
                    });
                    x += rw;
                }
            }
            if bar {
                let y = self.y;
                self.page().push(Op::Rect {
                    x: x0 - 12.0,
                    y,
                    w: 2.0,
                    h: look.leading,
                    color: LINE,
                });
            }
            for (dx, r) in &line.runs {
                let x = x0 + dx;
                if r.underline {
                    let y = baseline + 1.5;
                    self.page().push(Op::Line {
                        x1: x,
                        y1: y,
                        x2: x + r.width,
                        y2: y,
                        width: 0.5,
                        color: r.color,
                    });
                }
                self.page().push(Op::Text {
                    face: r.face,
                    size: r.size,
                    x,
                    y: baseline,
                    text: r.text.clone(),
                    color: r.color,
                });
            }
            self.y += look.leading;
        }
    }

    fn blocks(&mut self, blocks: &[Block], x0: f32, width: f32, quote: bool, depth: usize) {
        for b in blocks {
            let mut look = Look::body();
            if quote {
                look.color = MUTED;
                look.italic = true;
            }
            match b {
                Block::Heading(l, i) => {
                    let (size, before) = match l {
                        1 => (20.0, 18.0),
                        2 => (16.0, 15.0),
                        3 => (13.5, 12.0),
                        _ => (11.5, 10.0),
                    };
                    let look = Look {
                        size,
                        leading: size * 1.3,
                        bold: true,
                        ..look
                    };
                    self.gap(before);
                    let lines = self.wrap(i, look, width);
                    // Keep the heading with at least two lines of what follows.
                    self.ensure(lines.len() as f32 * look.leading + 2.0 * Look::body().leading);
                    self.place(&lines, x0, look, None, false);
                    self.y += 4.0;
                }
                Block::Para(i) => {
                    let lines = self.wrap(i, look, width);
                    self.place(&lines, x0, look, None, quote);
                    self.y += 6.0;
                }
                Block::List { ordered, items } => {
                    let indent = 18.0;
                    for (n, item) in items.iter().enumerate() {
                        let marker = if *ordered {
                            format!("{}.", n + 1)
                        } else {
                            ["•", "◦", "▪"][depth % 3].to_string()
                        };
                        let (cx, cw) = (x0 + indent, (width - indent).max(60.0));
                        for (k, inner) in item.iter().enumerate() {
                            match inner {
                                Block::Para(i) => {
                                    let lines = self.wrap(i, look, cw);
                                    self.place(
                                        &lines,
                                        cx,
                                        look,
                                        (k == 0).then_some(marker.as_str()),
                                        quote,
                                    );
                                    self.y += 2.0;
                                }
                                other => self.blocks(
                                    std::slice::from_ref(other),
                                    cx,
                                    cw,
                                    quote,
                                    depth + 1,
                                ),
                            }
                        }
                    }
                    self.y += 4.0;
                }
                Block::Quote(inner) => self.blocks(inner, x0 + 14.0, width - 14.0, true, depth),
                Block::Code(c) => {
                    let look = Look {
                        size: 9.0,
                        leading: 12.5,
                        ..Look::body()
                    };
                    let style = Style {
                        code: true,
                        ..Default::default()
                    };
                    self.gap(2.0);
                    for src in c.split('\n') {
                        let inl = [Inline {
                            text: src.replace('\t', "    "),
                            style: style.clone(),
                        }];
                        let lines = self.wrap(&inl, Look { size: 10.0, ..look }, width - 12.0);
                        for line in lines {
                            self.ensure(look.leading);
                            let y = self.y;
                            self.page().push(Op::Rect {
                                x: x0 - 4.0,
                                y,
                                w: width + 8.0,
                                h: look.leading,
                                color: (244, 244, 246),
                            });
                            self.place(std::slice::from_ref(&line), x0 + 4.0, look, None, false);
                        }
                    }
                    self.y += 8.0;
                }
                Block::Table {
                    rows,
                    header,
                    pictures,
                } => self.table(rows, *header, pictures, x0, width),
                Block::Rule => {
                    self.ensure(14.0);
                    let y = self.y + 6.0;
                    self.page().push(Op::Line {
                        x1: x0,
                        y1: y,
                        x2: x0 + width,
                        y2: y,
                        width: 0.75,
                        color: LINE,
                    });
                    self.y += 14.0;
                }
                Block::Image(p) => self.picture(p, x0, width),
                Block::PageBreak => {
                    if !self.page_is_empty() {
                        self.new_page();
                    }
                }
            }
        }
    }

    /// A picture centred in the column, as wide as placed in the source but no wider than
    /// the column or taller than a page.
    fn picture(&mut self, p: &super::Picture, x0: f32, width: f32) {
        let Some((img, placed)) = self.image(p, width, BOTTOM - TOP) else {
            return;
        };
        self.gap(4.0);
        self.ensure(placed.h);
        let (x, y) = (x0 + (width - placed.w) / 2.0, self.y);
        self.page().push(Op::Image {
            x,
            y,
            w: placed.w,
            h: placed.h,
            img,
        });
        self.y += placed.h + 8.0;
    }

    /// A picture's image (embedded once per document) and its size within `max_w` ×
    /// `max_h` points.
    fn image(
        &mut self,
        p: &super::Picture,
        max_w: f32,
        max_h: f32,
    ) -> Option<(krilla::image::Image, super::picture::Placed)> {
        let mut placed = super::picture::place(p, max_w, max_h, false)?;
        let img = self
            .images
            .entry(std::sync::Arc::as_ptr(&p.bytes))
            .or_insert_with(|| {
                let data: krilla::Data = std::mem::take(&mut placed.bytes).into();
                match placed.ext {
                    "jpg" => krilla::image::Image::from_jpeg(data, true),
                    "gif" => krilla::image::Image::from_gif(data, true),
                    _ => krilla::image::Image::from_png(data, true),
                }
                .ok()
            })
            .clone()?;
        Some((img, placed))
    }

    fn table(
        &mut self,
        rows: &[Vec<String>],
        header: bool,
        pictures: &[super::CellPicture],
        x0: f32,
        width: f32,
    ) {
        let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        // Column widths proportional to content length (capped), at least 36 pt each.
        let want: Vec<f32> = (0..cols)
            .map(|c| {
                rows.iter()
                    .map(|r| r.get(c).map_or(0, |s| s.chars().count()))
                    .max()
                    .unwrap_or(0)
                    .clamp(3, 40) as f32
            })
            .collect();
        let total: f32 = want.iter().sum();
        let mut w: Vec<f32> = want.iter().map(|x| (x / total * width).max(36.0)).collect();
        let sum: f32 = w.iter().sum();
        for x in &mut w {
            *x *= width / sum;
        }
        let look = Look {
            size: 9.0,
            leading: 12.0,
            ..Look::body()
        };
        let pad = 4.0;
        self.gap(4.0);
        // Pictures under each cell's text: (image, width, height), fitted to the cell and
        // no taller than a third of the page so a row still fits on one.
        type CellPics = Vec<Vec<(krilla::image::Image, f32, f32)>>;
        let max_pic_h = (BOTTOM - TOP) / 3.0;
        let lay = |t: &mut Typesetter, r: usize| -> (Vec<Vec<LaidLine>>, CellPics, f32) {
            let bold = header && r == 0;
            let cells: Vec<Vec<LaidLine>> = (0..cols)
                .map(|c| {
                    let text = rows[r].get(c).cloned().unwrap_or_default();
                    let inl = [Inline {
                        text,
                        style: Style {
                            bold,
                            ..Default::default()
                        },
                    }];
                    t.wrap(&inl, look, w[c] - 2.0 * pad)
                })
                .collect();
            let pics: CellPics = (0..cols)
                .map(|c| {
                    super::cell_pictures(pictures, r, c)
                        .filter_map(|p| {
                            let (img, pl) = t.image(p, w[c] - 2.0 * pad, max_pic_h)?;
                            Some((img, pl.w, pl.h))
                        })
                        .collect()
                })
                .collect();
            let h = cells
                .iter()
                .zip(&pics)
                .map(|(l, p)| {
                    // As drawn: the lines, a gap, then each picture and a gap after it.
                    let text = match (l.len(), p.is_empty()) {
                        (0, true) => look.leading,
                        (0, false) => 0.0,
                        (n, true) => n as f32 * look.leading,
                        (n, false) => n as f32 * look.leading + pad,
                    };
                    text + p.iter().map(|x| x.2 + pad).sum::<f32>()
                })
                .fold(look.leading, f32::max)
                + 2.0 * pad;
            (cells, pics, h)
        };
        let draw_row =
            |t: &mut Typesetter, cells: &[Vec<LaidLine>], pics: &CellPics, h: f32, head: bool| {
                let y = t.y;
                if head {
                    t.page().push(Op::Rect {
                        x: x0,
                        y,
                        w: width,
                        h,
                        color: (240, 241, 244),
                    });
                }
                let mut x = x0;
                for (c, lines) in cells.iter().enumerate() {
                    t.page().push(Op::Line {
                        x1: x,
                        y1: y,
                        x2: x,
                        y2: y + h,
                        width: 0.5,
                        color: LINE,
                    });
                    for (k, line) in lines.iter().enumerate() {
                        let baseline = y + pad + k as f32 * look.leading + look.size * 1.02;
                        for (dx, r) in &line.runs {
                            t.page().push(Op::Text {
                                face: r.face,
                                size: r.size,
                                x: x + pad + dx,
                                y: baseline,
                                text: r.text.clone(),
                                color: r.color,
                            });
                        }
                    }
                    let mut py = y + pad + lines.len() as f32 * look.leading;
                    if !lines.is_empty() {
                        py += pad;
                    }
                    for (img, pw, ph) in &pics[c] {
                        t.page().push(Op::Image {
                            x: x + pad,
                            y: py,
                            w: *pw,
                            h: *ph,
                            img: img.clone(),
                        });
                        py += ph + pad;
                    }
                    x += w[c];
                }
                t.page().push(Op::Line {
                    x1: x0 + width,
                    y1: y,
                    x2: x0 + width,
                    y2: y + h,
                    width: 0.5,
                    color: LINE,
                });
                t.page().push(Op::Line {
                    x1: x0,
                    y1: y,
                    x2: x0 + width,
                    y2: y,
                    width: 0.5,
                    color: LINE,
                });
                t.page().push(Op::Line {
                    x1: x0,
                    y1: y + h,
                    x2: x0 + width,
                    y2: y + h,
                    width: 0.5,
                    color: LINE,
                });
                t.y += h;
            };
        let head = if header { Some(lay(self, 0)) } else { None };
        let start = usize::from(header);
        if let Some((cells, pics, h)) = &head {
            self.ensure(h + look.leading + 2.0 * pad);
            draw_row(self, cells, pics, *h, true);
        }
        for r in start..rows.len() {
            let (cells, pics, h) = lay(self, r);
            if self.y + h > BOTTOM && !self.page_is_empty() {
                self.new_page();
                if let Some((hc, hp, hh)) = &head {
                    draw_row(self, hc, hp, *hh, true);
                }
            }
            draw_row(self, &cells, &pics, h, false);
        }
        self.y += 10.0;
    }
}

fn paint(c: (u8, u8, u8)) -> krilla::paint::Paint {
    rgb::Color::new(c.0, c.1, c.2).into()
}

pub fn write(doc: &Doc, fallback: &str, out: &Path) -> Result<(), EngineError> {
    let mut t = Typesetter {
        fonts: Fonts::load()?,
        pages: Vec::new(),
        y: TOP,
        images: Default::default(),
    };
    t.blocks(&doc.blocks, MARGIN_X, WIDTH, false, 0);
    if t.pages.is_empty() {
        t.pages.push(Vec::new());
    }
    let Typesetter { fonts, pages, .. } = t;

    let mut pdf = krilla::Document::new();
    pdf.set_metadata(
        krilla::metadata::Metadata::new()
            .title(doc.title_or(fallback))
            .creator("Swirvo".to_string()),
    );
    let settings = PageSettings::from_wh(PAGE_W, PAGE_H)
        .ok_or_else(|| EngineError::Encode("bad page size".into()))?;
    for ops in pages {
        let mut page = pdf.start_page_with(settings.clone());
        let mut s = page.surface();
        for op in ops {
            match op {
                Op::Image { x, y, w, h, img } => {
                    if let Some(sz) = krilla::geom::Size::from_wh(w.max(0.01), h.max(0.01)) {
                        s.push_transform(&krilla::geom::Transform::from_translate(x, y));
                        s.draw_image(img, sz);
                        s.pop();
                    }
                }
                Op::Text {
                    face,
                    size,
                    x,
                    y,
                    text,
                    color,
                } => {
                    s.set_fill(Some(Fill {
                        paint: paint(color),
                        ..Default::default()
                    }));
                    s.draw_text(
                        Point::from_xy(x, y),
                        fonts.faces[face].pdf.clone(),
                        size,
                        &text,
                        false,
                        TextDirection::Auto,
                    );
                }
                Op::Rect { x, y, w, h, color } => {
                    if let Some(r) = Rect::from_xywh(x, y, w, h) {
                        let mut pb = PathBuilder::new();
                        pb.push_rect(r);
                        if let Some(p) = pb.finish() {
                            s.set_fill(Some(Fill {
                                paint: paint(color),
                                ..Default::default()
                            }));
                            s.set_stroke(None);
                            s.draw_path(&p);
                        }
                    }
                }
                Op::Line {
                    x1,
                    y1,
                    x2,
                    y2,
                    width,
                    color,
                } => {
                    let mut pb = PathBuilder::new();
                    pb.move_to(x1, y1);
                    pb.line_to(x2, y2);
                    if let Some(p) = pb.finish() {
                        s.set_fill(None);
                        s.set_stroke(Some(Stroke {
                            paint: paint(color),
                            width,
                            ..Default::default()
                        }));
                        s.draw_path(&p);
                        s.set_stroke(None);
                    }
                }
            }
        }
        s.finish();
        page.finish();
    }
    let bytes = pdf
        .finish()
        .map_err(|e| EngineError::Encode(format!("PDF: {e:?}")))?;
    std::fs::write(out, bytes)?;
    Ok(())
}
