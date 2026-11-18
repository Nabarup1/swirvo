//! Win32 helpers shared by the agent overlay and the app windows (Board, page picker).

use std::ffi::c_void;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS,
    DeleteDC, DeleteObject, HBITMAP, HDC, HGDIOBJ, SelectObject,
};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, ReleaseStgMedium};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

/// A top-down 32-bpp DIB selected into a memory DC.
pub(crate) struct Surface {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> Option<Surface> {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        // SAFETY: plain GDI calls; `bits` receives the DIB's pixel pointer, valid while `bmp` lives.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmp = CreateDIBSection(Some(dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
            if bits.is_null() {
                let _ = DeleteObject(bmp.into());
                let _ = DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp.into());
            Some(Surface {
                dc,
                bmp,
                old,
                bits: bits.cast(),
                w,
                h,
            })
        }
    }

    pub fn pixels(&mut self) -> &mut [u8] {
        // SAFETY: the DIB holds exactly w*h*4 bytes and is only touched from this thread.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: undo `new` in reverse order.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

pub(crate) fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

pub(crate) fn set_clipboard(hwnd: HWND, text: &str) -> windows_core::Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: standard clipboard protocol; on success the system owns the HGLOBAL.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2)?;
        let dst = GlobalLock(mem).cast::<u16>();
        if dst.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err(windows_core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
        let _ = GlobalUnlock(mem);
        OpenClipboard(Some(hwnd))?;
        let r = EmptyClipboard()
            .and_then(|()| SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(mem.0))));
        let _ = CloseClipboard();
        if r.is_err() {
            let _ = GlobalFree(Some(mem));
        }
        r.map(|_| ())
    }
}

/// File paths from a drag's `CF_HDROP`, or empty when the drag carries no files.
pub(crate) fn dropped_files(obj: &IDataObject) -> Vec<PathBuf> {
    let fmt = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    // SAFETY: GetData hands us a STGMEDIUM we release; the HDROP lives inside it until then.
    unsafe {
        let Ok(mut medium) = obj.GetData(&fmt) else {
            return Vec::new();
        };
        let files = hdrop_files(HDROP(medium.u.hGlobal.0));
        ReleaseStgMedium(&mut medium);
        files
    }
}

/// Files paths from an HDROP handle (clipboard or drag).
unsafe fn hdrop_files(hdrop: HDROP) -> Vec<PathBuf> {
    // SAFETY: the caller guarantees `hdrop` is valid for the duration of the call.
    unsafe {
        let n = DragQueryFileW(hdrop, u32::MAX, None);
        let mut files = Vec::with_capacity(n as usize);
        for i in 0..n {
            let len = DragQueryFileW(hdrop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
            files.push(PathBuf::from(std::ffi::OsString::from_wide(
                &buf[..got.min(len)],
            )));
        }
        files
    }
}

/// What's on the clipboard for a paste: copied files (Explorer's Ctrl+C) or text.
pub(crate) enum Paste {
    Files(Vec<PathBuf>),
    Text(String),
    Nothing,
}

pub(crate) fn paste(hwnd: HWND) -> Paste {
    // SAFETY: standard clipboard protocol; handles are only read while the clipboard is open.
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return Paste::Nothing;
        }
        let out = if IsClipboardFormatAvailable(CF_HDROP.0 as u32).is_ok() {
            match GetClipboardData(CF_HDROP.0 as u32) {
                Ok(h) => Paste::Files(hdrop_files(HDROP(h.0))),
                Err(_) => Paste::Nothing,
            }
        } else if IsClipboardFormatAvailable(CF_UNICODETEXT.0 as u32).is_ok() {
            match GetClipboardData(CF_UNICODETEXT.0 as u32) {
                Ok(h) => {
                    let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0)).cast::<u16>();
                    if p.is_null() {
                        Paste::Nothing
                    } else {
                        let mut n = 0;
                        while *p.add(n) != 0 && n < 1 << 20 {
                            n += 1;
                        }
                        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                        let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0));
                        Paste::Text(s)
                    }
                }
                Err(_) => Paste::Nothing,
            }
        } else {
            Paste::Nothing
        };
        let _ = CloseClipboard();
        out
    }
}
