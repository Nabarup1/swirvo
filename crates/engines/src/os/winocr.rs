//! Windows.Media.Ocr: the OCR engine built into Windows 10/11. It uses the OCR language
//! packs installed with the user's languages (Settings → Time & language → Language).

use swirvo_core::EngineError;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

fn err(what: &str) -> impl Fn(windows_core::Error) -> EngineError + '_ {
    move |e| EngineError::Invalid(format!("{what}: {e}"))
}

pub fn recognize(img: &image::RgbaImage) -> Result<String, EngineError> {
    with_com(|| {
        let (result, _) = run(img)?;
        let lines = result.Lines().map_err(err("OCR"))?;
        let mut out = String::new();
        for i in 0..lines.Size().unwrap_or(0) {
            if let Ok(line) = lines.GetAt(i)
                && let Ok(t) = line.Text()
            {
                out.push_str(&t.to_string_lossy());
                out.push('\n');
            }
        }
        Ok(out)
    })
}

/// Lines of words with their boxes, in `img`'s pixels.
pub fn recognize_lines(img: &image::RgbaImage) -> Result<Vec<crate::ocr::Line>, EngineError> {
    with_com(|| {
        let (result, k) = run(img)?;
        let lines = result.Lines().map_err(err("OCR"))?;
        let mut out = Vec::new();
        for i in 0..lines.Size().unwrap_or(0) {
            let Ok(line) = lines.GetAt(i) else { continue };
            let Ok(words) = line.Words() else { continue };
            let mut l = crate::ocr::Line::default();
            for j in 0..words.Size().unwrap_or(0) {
                let Ok(w) = words.GetAt(j) else { continue };
                let (Ok(t), Ok(r)) = (w.Text(), w.BoundingRect()) else {
                    continue;
                };
                l.words.push(crate::ocr::Word {
                    text: t.to_string_lossy(),
                    x: r.X / k,
                    y: r.Y / k,
                    w: r.Width / k,
                    h: r.Height / k,
                });
            }
            if !l.words.is_empty() {
                out.push(l);
            }
        }
        Ok(out)
    })
}

fn with_com<T>(f: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
    // SAFETY: per-thread COM/WinRT init, balanced below when it succeeded.
    let inited = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let r = f();
    if inited {
        // SAFETY: balances the successful init above.
        unsafe { CoUninitialize() };
    }
    r
}

/// Run the engine; also returns the scale applied to the image.
fn run(img: &image::RgbaImage) -> Result<(windows::Media::Ocr::OcrResult, f32), EngineError> {
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        EngineError::Invalid(
            "Windows has no OCR language installed (Settings → Time & language → Language → add a language with \"Optical character recognition\")".into(),
        )
    })?;
    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000).max(1000);
    // OCR wants text at least ~20 px tall: upscale small images, downscale huge ones.
    let (w, h) = img.dimensions();
    let longest = w.max(h).max(1);
    let k = if longest < 1000 {
        (2000.0 / longest as f32).min(3.0)
    } else if longest > max {
        max as f32 / longest as f32
    } else {
        1.0
    };
    let scaled = (k - 1.0).abs() > 0.01;
    let img = if scaled {
        image::imageops::resize(
            img,
            ((w as f32 * k) as u32).max(1),
            ((h as f32 * k) as u32).max(1),
            image::imageops::FilterType::CatmullRom,
        )
    } else {
        img.clone()
    };
    let (w, h) = img.dimensions();
    // Flatten onto white and swap to BGRA (opaque, so premultiplied == straight).
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in img.pixels() {
        let a = p[3] as u32;
        let mix = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
        bgra.extend_from_slice(&[mix(p[2]), mix(p[1]), mix(p[0]), 255]);
    }
    let writer = DataWriter::new().map_err(err("OCR buffer"))?;
    writer.WriteBytes(&bgra).map_err(err("OCR buffer"))?;
    let buffer = writer.DetachBuffer().map_err(err("OCR buffer"))?;
    let bmp =
        SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, w as i32, h as i32)
            .map_err(err("OCR bitmap"))?;
    let result = engine
        .RecognizeAsync(&bmp)
        .and_then(|op| op.join())
        .map_err(err("OCR"))?;
    Ok((result, if scaled { k } else { 1.0 }))
}
