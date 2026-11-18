//! Optical character recognition through what the OS has (docs/12 §2): the built-in
//! Windows.Media.Ocr engine on Windows, `tesseract` (if installed) on Linux. 0 bytes in
//! our download. Used by image → TXT, by "Copy text" on images, and by the PDF reader
//! when a PDF has no text layer (scans).

use std::path::Path;
use std::sync::OnceLock;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

/// Raster formats OCR accepts directly.
pub const INPUTS: &[&str] = &["png", "jpg", "webp", "bmp", "tiff", "gif"];

/// Is an OCR engine available? Cheap: no engine is started.
pub fn available() -> bool {
    static AVAIL: OnceLock<bool> = OnceLock::new();
    *AVAIL.get_or_init(|| {
        if std::env::var_os("SWIRVO_NO_OCR").is_some() {
            return false;
        }
        #[cfg(windows)]
        {
            true
        }
        #[cfg(not(windows))]
        {
            crate::external::which(&["tesseract"], &[]).is_some()
        }
    })
}

/// Recognize text in `img`. Lines are separated by `\n`, blocks by a blank line.
pub fn recognize(img: &image::RgbaImage) -> Result<String, EngineError> {
    if !available() {
        return Err(EngineError::Invalid("no OCR engine on this PC".into()));
    }
    #[cfg(windows)]
    {
        crate::os::winocr::recognize(img)
    }
    #[cfg(not(windows))]
    {
        tesseract(img)
    }
}

#[cfg(not(windows))]
fn tesseract(img: &image::RgbaImage) -> Result<String, EngineError> {
    let exe = crate::external::which(&["tesseract"], &[])
        .ok_or_else(|| EngineError::Invalid("tesseract isn't installed".into()))?;
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("page.png");
    image::DynamicImage::ImageRgba8(img.clone())
        .to_rgb8()
        .save(&input)
        .map_err(EngineError::encode)?;
    let base = dir.path().join("out");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(&input).arg(&base);
    crate::external::run(cmd, &swirvo_core::CancelToken::new(), "tesseract")?;
    let text = std::fs::read_to_string(base.with_extension("txt"))?;
    Ok(text.replace('\u{c}', "\n"))
}

/// A recognized word and its box, in the image's pixels (top-left origin).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One line of text, words left to right.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub words: Vec<Word>,
}

/// Recognize words with their positions (for a searchable PDF's hidden text layer).
pub fn recognize_lines(img: &image::RgbaImage) -> Result<Vec<Line>, EngineError> {
    if !available() {
        return Err(EngineError::Invalid("no OCR engine on this PC".into()));
    }
    #[cfg(windows)]
    {
        crate::os::winocr::recognize_lines(img)
    }
    #[cfg(not(windows))]
    {
        tesseract_lines(img)
    }
}

#[cfg(not(windows))]
fn tesseract_lines(img: &image::RgbaImage) -> Result<Vec<Line>, EngineError> {
    let exe = crate::external::which(&["tesseract"], &[])
        .ok_or_else(|| EngineError::Invalid("tesseract isn't installed".into()))?;
    let dir = tempfile::tempdir()?;
    let input = dir.path().join("page.png");
    image::DynamicImage::ImageRgba8(img.clone())
        .to_rgb8()
        .save(&input)
        .map_err(EngineError::encode)?;
    let base = dir.path().join("out");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(&input).arg(&base).arg("tsv");
    crate::external::run(cmd, &swirvo_core::CancelToken::new(), "tesseract")?;
    let tsv = std::fs::read_to_string(base.with_extension("tsv"))?;
    Ok(parse_tsv(&tsv))
}

/// Tesseract's TSV: level page block par line word left top width height conf text.
#[cfg(any(test, not(windows)))]
fn parse_tsv(tsv: &str) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    let mut key = None;
    for row in tsv.lines().skip(1) {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let text = c[11].trim();
        if text.is_empty() {
            continue;
        }
        let n = |i: usize| c[i].parse::<f32>().unwrap_or(0.0);
        let k = (c[1], c[2], c[3], c[4]);
        if key != Some(k) {
            out.push(Line::default());
            key = Some(k);
        }
        if let Some(l) = out.last_mut() {
            l.words.push(Word {
                text: text.to_string(),
                x: n(6),
                y: n(7),
                w: n(8),
                h: n(9),
            });
        }
    }
    out
}

/// Recognize the text in an image file.
pub fn recognize_file(path: &Path, key: &str) -> Result<String, EngineError> {
    let img = crate::image::load(path, key)?;
    recognize(&img.to_rgba8())
}

/// Image → TXT via OCR. The planner chains it (PNG → TXT → DOCX, …).
pub struct OcrEngine;

impl Engine for OcrEngine {
    fn id(&self) -> &'static str {
        "ocr"
    }

    fn capabilities(&self) -> Vec<Capability> {
        INPUTS
            .iter()
            .map(|f| {
                Capability::new("ocr", f, "txt")
                    .loss(35)
                    .time(400, 300)
                    .note("recognized text (OCR); layout isn't kept")
            })
            .collect()
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let text = recognize_file(req.input, req.from)?;
        req.cancel.check()?;
        if text.trim().is_empty() {
            return Err(EngineError::Invalid("no text found in this image".into()));
        }
        std::fs::write(
            req.output,
            text.replace('\n', "\r\n").replace("\r\r\n", "\r\n"),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_tesseract_tsv() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t800\t600\t-1\t\n\
5\t1\t1\t1\t1\t1\t10\t20\t50\t12\t96\tHello\n\
5\t1\t1\t1\t1\t2\t70\t20\t60\t12\t95\tworld\n\
5\t1\t1\t1\t2\t1\t10\t40\t30\t12\t91\tBye\n";
        let l = super::parse_tsv(tsv);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].words[1].text, "world");
        assert_eq!(l[0].words[1].x, 70.0);
        assert_eq!(l[1].words[0].text, "Bye");
    }
}
