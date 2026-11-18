//! Engines the operating system already ships (docs/12 §8): 0 bytes in our download.
//!
//! - `systar`: bsdtar/libarchive (`C:\Windows\System32\tar.exe` since Windows 10 1803;
//!   `bsdtar` on Linux) for 7Z, RAR, ISO and CAB.
//! - `wic` (Windows): the Windows Imaging Component, with whatever codecs this PC has
//!   (HEIF, AV1, Raw Image Extensions…).
//! - `mediafoundation` (Windows): video to MP4 (H.264/AAC), audio to MP3/M4A/AAC/WAV,
//!   video to GIF, with the codecs Windows ships.
//! - `msoffice` (Windows): Word, Excel and PowerPoint, when installed, export their files
//!   to PDF and open DOC/XLS/PPT, through COM automation.
//! - `certsign` (Windows): signs and verifies PDF signatures with the user's certificates
//!   (CryptoAPI and the Windows certificate store); a tool, not a conversion.
//! - OCR lives in `crate::ocr` (Windows.Media.Ocr or tesseract).

use swirvo_core::Engine;

pub mod certsign;
#[cfg(windows)]
pub(crate) mod mediafoundation;
#[cfg(windows)]
pub(crate) mod msoffice;
pub mod systar;
#[cfg(windows)]
pub(crate) mod wic;
#[cfg(windows)]
pub(crate) mod winocr;

pub fn discover() -> Vec<Box<dyn Engine>> {
    discover_with(crate::Packs::default())
}

/// Is Word, Excel or PowerPoint installed? (For the Settings window.)
pub fn office_installed() -> bool {
    #[cfg(windows)]
    {
        msoffice::MsOfficeEngine::discover().is_some()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// [`discover`], leaving out Microsoft Office when `packs.office` is off.
pub fn discover_with(packs: crate::Packs) -> Vec<Box<dyn Engine>> {
    let _ = packs;
    let mut v: Vec<Box<dyn Engine>> = Vec::new();
    if let Some(e) = systar::SysTar::find() {
        v.push(Box::new(e));
    }
    #[cfg(windows)]
    if let Some(e) = wic::WicEngine::discover() {
        v.push(Box::new(e));
    }
    #[cfg(windows)]
    if let Some(e) = mediafoundation::MfEngine::discover() {
        v.push(Box::new(e));
    }
    #[cfg(windows)]
    if packs.office
        && let Some(e) = msoffice::MsOfficeEngine::discover()
    {
        v.push(Box::new(e));
    }
    if crate::ocr::available() {
        v.push(Box::new(crate::ocr::OcrEngine));
    }
    v
}

#[cfg(all(test, windows))]
mod tests {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

    /// Finding the OS engines runs on the app's UI thread before drag and drop is set up,
    /// which needs a single-threaded apartment: discovery must leave the thread's COM
    /// state alone (a multithreaded init left behind made the app fail to start).
    #[test]
    fn discovery_leaves_the_threads_apartment_alone() {
        std::thread::spawn(|| {
            let found = super::discover();
            assert!(!found.is_empty());
            // SAFETY: COM init on this test thread, balanced below.
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            assert!(hr.is_ok(), "the thread can still become an STA: {hr:?}");
            // SAFETY: balances the successful CoInitializeEx above.
            unsafe { CoUninitialize() };
        })
        .join()
        .unwrap();
    }
}
