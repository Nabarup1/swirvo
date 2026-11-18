//! Image decoding through WIC with whatever codecs this PC has: the HEIF, AV1 (AVIF) and
//! Raw Image Extensions from the Microsoft Store are common on Windows 11. Only formats
//! the built-in engines can't read are registered, and only if a WIC decoder claims them.

use std::os::windows::ffi::OsStrExt;

use swirvo_core::{Capability, Engine, EngineError, StepRequest};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapCodecInfo, IWICImagingFactory,
    IWICPalette, WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICComponentEnumerateDefault,
    WICDecodeMetadataCacheOnDemand, WICDecoder,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows_core::{Interface, PCWSTR};

/// Our format keys a WIC codec may cover, with the extensions that identify them.
const CANDIDATES: &[(&str, &[&str])] = &[
    ("heic", &["heic", "heif", "hif"]),
    ("avif", &["avif"]),
    ("dng", &["dng"]),
    ("cr2", &["cr2"]),
    ("cr3", &["cr3"]),
    ("nef", &["nef"]),
    ("arw", &["arw"]),
];

/// Largest image we decode (pixels).
const MAX_PIXELS: u64 = 250_000_000;

pub struct WicEngine {
    keys: Vec<&'static str>,
}

/// COM for the current thread; balanced on drop when we initialized it.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // SAFETY: per-thread COM init; S_FALSE / RPC_E_CHANGED_MODE mean COM is already up.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Com(hr.is_ok())
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        }
    }
}

fn factory() -> Result<IWICImagingFactory, EngineError> {
    // SAFETY: standard in-proc COM activation.
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| EngineError::Invalid(format!("Windows Imaging Component unavailable: {e}")))
}

/// Lower-case file extensions (without dots) of every installed WIC decoder.
fn decoder_extensions() -> Vec<String> {
    let _com = Com::init();
    let Ok(f) = factory() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    // SAFETY: enumerating codec infos; buffers sized from the reported length.
    unsafe {
        let Ok(en) =
            f.CreateComponentEnumerator(WICDecoder.0 as u32, WICComponentEnumerateDefault.0 as u32)
        else {
            return out;
        };
        loop {
            let mut item = [None];
            let mut got = 0u32;
            if en.Next(&mut item, Some(&mut got)).is_err() || got == 0 {
                break;
            }
            let Some(unk) = item[0].take() else { break };
            let Ok(info) = unk.cast::<IWICBitmapCodecInfo>() else {
                continue;
            };
            let mut len = 0u32;
            let _ = info.GetFileExtensions(&mut [], &mut len);
            if len == 0 || len > 4096 {
                continue;
            }
            let mut buf = vec![0u16; len as usize];
            if info.GetFileExtensions(&mut buf, &mut len).is_ok() {
                let s = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
                out.extend(
                    s.split(',')
                        .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                        .filter(|e| !e.is_empty()),
                );
            }
        }
    }
    out
}

impl WicEngine {
    pub fn discover() -> Option<WicEngine> {
        if std::env::var_os("SWIRVO_NO_WIC").is_some() {
            return None;
        }
        let exts = decoder_extensions();
        let keys: Vec<&'static str> = CANDIDATES
            .iter()
            .filter(|(_, e)| e.iter().any(|x| exts.iter().any(|y| y == x)))
            .map(|(k, _)| *k)
            .collect();
        (!keys.is_empty()).then_some(WicEngine { keys })
    }
}

/// Decode the first frame of `path` to straight-alpha RGBA.
pub fn decode(path: &std::path::Path) -> Result<image::RgbaImage, EngineError> {
    let _com = Com::init();
    let f = factory()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: WIC calls with a NUL-terminated path and a buffer of exactly stride × height.
    unsafe {
        let dec = f
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(|e| EngineError::Decode(format!("Windows can't open this image ({e})")))?;
        let frame = dec.GetFrame(0).map_err(EngineError::decode)?;
        let conv = f.CreateFormatConverter().map_err(EngineError::decode)?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .map_err(EngineError::decode)?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h).map_err(EngineError::decode)?;
        if w == 0 || h == 0 || w as u64 * h as u64 > MAX_PIXELS {
            return Err(EngineError::Invalid(format!(
                "{w}×{h} is too large to convert"
            )));
        }
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut buf)
            .map_err(EngineError::decode)?;
        image::RgbaImage::from_raw(w, h, buf)
            .ok_or_else(|| EngineError::Decode("bad bitmap".into()))
    }
}

impl Engine for WicEngine {
    fn id(&self) -> &'static str {
        "os-wic"
    }

    fn capabilities(&self) -> Vec<Capability> {
        self.keys
            .iter()
            .map(|k| {
                Capability::new("os-wic", k, "png")
                    .loss(0)
                    .time(120, 120)
                    .note("decoded by Windows (installed codec)")
            })
            .collect()
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let img = decode(req.input)?;
        req.cancel.check()?;
        image::DynamicImage::ImageRgba8(img)
            .save_with_format(req.output, image::ImageFormat::Png)
            .map_err(EngineError::encode)
    }
}
