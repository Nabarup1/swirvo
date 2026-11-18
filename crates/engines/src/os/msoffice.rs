//! Microsoft Office, when it's installed (Microsoft 365, Office 2016+): Word, Excel and
//! PowerPoint export their own files to PDF, exactly as the user would see them, and
//! open the legacy formats (DOC, XLS, PPT) our engines don't read. Driven through COM
//! automation, the interface Office documents for this; nothing of Office is shipped.
//!
//! Safety:
//! - Macros are force-disabled before any file is opened (`AutomationSecurity`), and
//!   alerts are off. Files open read-only and are never saved back.
//! - A password-protected file gets a dummy password, so Office fails instead of
//!   showing a password prompt nobody can see.
//! - Every call runs on a worker thread with a deadline. Word and Excel start a private
//!   instance for us; if it hangs (a hidden dialog) or the job is cancelled, that process
//!   (identified as the one that appeared when we started it) is ended. PowerPoint runs
//!   one instance per user, so it's never ended: a hang only abandons the job.

use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use swirvo_core::{Capability, Engine, EngineError, StepRequest};
use windows::Win32::Foundation::{CloseHandle, DISP_E_EXCEPTION};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CLSIDFromProgID, COINIT_APARTMENTTHREADED, CoCreateInstance,
    CoInitializeEx, CoUninitialize, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO, IDispatch,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::System::Variant::VARIANT;
use windows_core::{BSTR, GUID, HSTRING, PCWSTR};

const DISPID_PROPERTYPUT: i32 = -3;
const LOCALE_USER_DEFAULT: u32 = 0x0400;
/// `msoAutomationSecurityForceDisable`: no macros, whatever the file or the user's
/// settings say.
const MACROS_OFF: i32 = 3;
/// Opens a protected file without prompting (the open fails instead).
const NO_PASSWORD: &str = "\u{1}swirvo";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum App {
    Word,
    Excel,
    PowerPoint,
}

impl App {
    fn prog_id(self) -> &'static str {
        match self {
            App::Word => "Word.Application",
            App::Excel => "Excel.Application",
            App::PowerPoint => "PowerPoint.Application",
        }
    }

    fn exe(self) -> &'static str {
        match self {
            App::Word => "WINWORD.EXE",
            App::Excel => "EXCEL.EXE",
            App::PowerPoint => "POWERPNT.EXE",
        }
    }

    fn name(self) -> &'static str {
        match self {
            App::Word => "Word",
            App::Excel => "Excel",
            App::PowerPoint => "PowerPoint",
        }
    }

    /// What it opens, and the modern format it saves legacy files as.
    fn inputs(self) -> (&'static [&'static str], &'static str) {
        match self {
            App::Word => (&["docx", "doc", "odt", "rtf"], "docx"),
            App::Excel => (&["xlsx", "xls", "ods"], "xlsx"),
            App::PowerPoint => (&["pptx", "ppt", "odp"], "pptx"),
        }
    }

    fn installed(self) -> bool {
        let id = HSTRING::from(self.prog_id());
        // SAFETY: a registry lookup; nothing is started.
        unsafe { CLSIDFromProgID(PCWSTR(id.as_ptr())) }.is_ok()
    }

    fn for_input(key: &str) -> Option<App> {
        [App::Word, App::Excel, App::PowerPoint]
            .into_iter()
            .find(|a| a.inputs().0.contains(&key))
    }
}

pub struct MsOfficeEngine {
    apps: Vec<App>,
}

impl MsOfficeEngine {
    /// Present when at least one Office app is registered. Cheap: no app is started.
    pub fn discover() -> Option<MsOfficeEngine> {
        if std::env::var_os("SWIRVO_NO_MSOFFICE").is_some() {
            return None;
        }
        // On a thread of its own with COM set up and torn down there, so the caller's
        // apartment (the UI thread's, which drag and drop needs) is never touched.
        let apps: Vec<App> = std::thread::spawn(|| {
            // SAFETY: per-thread COM init, balanced below.
            let inited = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let apps = [App::Word, App::Excel, App::PowerPoint]
                .into_iter()
                .filter(|a| a.installed())
                .collect();
            if inited {
                // SAFETY: balances the CoInitializeEx above on this thread.
                unsafe { CoUninitialize() };
            }
            apps
        })
        .join()
        .unwrap_or_default();
        (!apps.is_empty()).then_some(MsOfficeEngine { apps })
    }
}

impl Engine for MsOfficeEngine {
    fn id(&self) -> &'static str {
        "ms-office"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &app in &self.apps {
            let (inputs, modern) = app.inputs();
            for &from in inputs {
                caps.push(
                    Capability::new("ms-office", from, "pdf")
                        .loss(1)
                        // A little quicker than LibreOffice's estimate, so Office wins
                        // for its own formats when both are installed.
                        .time(2200, 300)
                        .note("Microsoft Office on this PC: as Office prints it"),
                );
                if from != modern {
                    caps.push(
                        Capability::new("ms-office", from, modern)
                            .loss(1)
                            .time(2200, 300)
                            .note("Microsoft Office"),
                    );
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        // One Office conversion at a time: each starts its own Word/Excel/PowerPoint (heavy),
        // and a hung one is found by the processes that weren't there before, which a
        // second conversion running alongside would muddle.
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = ONE.lock().unwrap_or_else(|p| p.into_inner());
        req.cancel.check()?;
        let app = App::for_input(req.from)
            .filter(|a| self.apps.contains(a))
            .ok_or_else(|| EngineError::Invalid(format!("Office doesn't open {}", req.from)))?;
        let input = std::path::absolute(req.input)?;
        let output = std::path::absolute(req.output)?;
        let to = req.to.to_string();
        let mb = std::fs::metadata(&input).map_or(0, |m| m.len()) / 1_048_576;
        let deadline = Duration::from_secs(120 + 4 * mb);
        let before = pids(app.exe());
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name(format!("ms-office-{}", app.name()))
            .spawn(move || {
                let _ = tx.send(convert(app, &input, &output, &to));
            })
            .map_err(EngineError::Io)?;
        let start = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(r) => return r,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(EngineError::Encode(format!(
                        "{} stopped unexpectedly",
                        app.name()
                    )));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let cancelled = req.cancel.is_cancelled();
                    if cancelled || start.elapsed() > deadline {
                        if app != App::PowerPoint {
                            for pid in pids(app.exe()).difference(&before) {
                                end_process(*pid);
                            }
                        }
                        return Err(if cancelled {
                            EngineError::Cancelled
                        } else {
                            EngineError::Encode(format!(
                                "{} didn't finish (it may be waiting on a dialog)",
                                app.name()
                            ))
                        });
                    }
                }
            }
        }
    }
}

/// Process ids of every running `exe`.
fn pids(exe: &str) -> HashSet<u32> {
    let mut out = HashSet::new();
    // SAFETY: a process snapshot we close; the entry has its size set as required.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            let name = String::from_utf16_lossy(&e.szExeFile[..len]);
            if name.eq_ignore_ascii_case(exe) {
                out.insert(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

fn end_process(pid: u32) {
    // SAFETY: opening a process by id with terminate rights only, then closing it.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
}

/// An automation object with late-bound calls.
struct Obj(IDispatch);

fn err(app: &str, what: &str, e: windows_core::Error, info: &EXCEPINFO) -> EngineError {
    let detail = if e.code() == DISP_E_EXCEPTION && !info.bstrDescription.is_empty() {
        info.bstrDescription.to_string()
    } else {
        e.message()
    };
    EngineError::Encode(format!("{app}: {what}: {}", detail.trim()))
}

impl Obj {
    fn invoke(
        &self,
        name: &str,
        flags: DISPATCH_FLAGS,
        args: &[(&str, VARIANT)],
    ) -> Result<VARIANT, EngineError> {
        // A property put passes its value unnamed; method arguments go by name.
        let put = flags == DISPATCH_PROPERTYPUT;
        let names: Vec<HSTRING> = std::iter::once(name)
            .chain(args.iter().filter(|_| !put).map(|(n, _)| *n))
            .map(HSTRING::from)
            .collect();
        let ptrs: Vec<PCWSTR> = names.iter().map(|h| PCWSTR(h.as_ptr())).collect();
        let mut ids = vec![0i32; ptrs.len()];
        // SAFETY: arrays sized to `ptrs`; the dispatch object is live.
        unsafe {
            self.0.GetIDsOfNames(
                &GUID::zeroed(),
                ptrs.as_ptr(),
                ptrs.len() as u32,
                LOCALE_USER_DEFAULT,
                ids.as_mut_ptr(),
            )
        }
        .map_err(|e| EngineError::Encode(format!("{name}: {}", e.message())))?;
        let mut values: Vec<VARIANT> = args.iter().map(|(_, v)| v.clone()).collect();
        let mut named: Vec<i32> = if put {
            vec![DISPID_PROPERTYPUT]
        } else {
            ids[1..].to_vec()
        };
        let params = DISPPARAMS {
            rgvarg: if values.is_empty() {
                std::ptr::null_mut()
            } else {
                values.as_mut_ptr()
            },
            rgdispidNamedArgs: if named.is_empty() {
                std::ptr::null_mut()
            } else {
                named.as_mut_ptr()
            },
            cArgs: values.len() as u32,
            cNamedArgs: named.len() as u32,
        };
        let mut result = VARIANT::default();
        let mut info = EXCEPINFO::default();
        // SAFETY: `params` points into `values` and `named`, alive for the call.
        unsafe {
            self.0.Invoke(
                ids[0],
                &GUID::zeroed(),
                LOCALE_USER_DEFAULT,
                flags,
                &params,
                Some(&mut result),
                Some(&mut info),
                None,
            )
        }
        .map_err(|e| err("Office", name, e, &info))?;
        Ok(result)
    }

    fn get(&self, name: &str) -> Result<Obj, EngineError> {
        let v = self.invoke(name, DISPATCH_PROPERTYGET, &[])?;
        IDispatch::try_from(&v)
            .map(Obj)
            .map_err(|_| EngineError::Encode(format!("{name} isn't an object")))
    }

    fn count(&self, name: &str) -> Option<i32> {
        let v = self.invoke(name, DISPATCH_PROPERTYGET, &[]).ok()?;
        i32::try_from(&v).ok()
    }

    fn put(&self, name: &str, v: VARIANT) -> Result<(), EngineError> {
        self.invoke(name, DISPATCH_PROPERTYPUT, &[("", v)])
            .map(|_| ())
    }

    fn call(&self, name: &str, args: &[(&str, VARIANT)]) -> Result<VARIANT, EngineError> {
        self.invoke(name, DISPATCH_METHOD | DISPATCH_PROPERTYGET, args)
    }

    fn call_obj(&self, name: &str, args: &[(&str, VARIANT)]) -> Result<Obj, EngineError> {
        let v = self.call(name, args)?;
        IDispatch::try_from(&v)
            .map(Obj)
            .map_err(|_| EngineError::Encode(format!("{name} returned nothing")))
    }
}

fn s(p: &Path) -> VARIANT {
    VARIANT::from(BSTR::from(p.as_os_str().to_string_lossy().as_ref()))
}

/// Runs on its own thread: start the app, open read-only, export, close, quit.
fn convert(app: App, input: &Path, output: &Path, to: &str) -> Result<(), EngineError> {
    // SAFETY: per-thread COM init (Office's objects want a single-threaded apartment),
    // balanced below.
    let inited = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let r = (|| {
        let id = HSTRING::from(app.prog_id());
        // SAFETY: CLSID lookup and instance creation through the documented ProgID.
        let disp: IDispatch = unsafe {
            let clsid = CLSIDFromProgID(PCWSTR(id.as_ptr()))
                .map_err(|e| EngineError::Encode(format!("{}: {}", app.name(), e.message())))?;
            CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER).map_err(|e| {
                EngineError::Encode(format!("couldn't start {}: {}", app.name(), e.message()))
            })?
        };
        let a = Obj(disp);
        let r = match app {
            App::Word => word(&a, input, output, to),
            App::Excel => excel(&a, input, output, to),
            App::PowerPoint => powerpoint(&a, input, output, to),
        };
        // Our own Word/Excel instance always quits; PowerPoint only when it holds nothing
        // else (it may be the user's open instance).
        match app {
            App::Word => {
                let _ = a.call("Quit", &[("SaveChanges", VARIANT::from(0i32))]);
            }
            App::Excel => {
                let _ = a.call("Quit", &[]);
            }
            App::PowerPoint => {
                if a.get("Presentations").ok().and_then(|p| p.count("Count")) == Some(0) {
                    let _ = a.call("Quit", &[]);
                }
            }
        }
        r
    })();
    if inited {
        // SAFETY: balances the successful init above.
        unsafe { CoUninitialize() };
    }
    r
}

fn word(a: &Obj, input: &Path, output: &Path, to: &str) -> Result<(), EngineError> {
    a.put("Visible", false.into())?;
    a.put("DisplayAlerts", 0i32.into())?; // wdAlertsNone
    a.put("AutomationSecurity", MACROS_OFF.into())?;
    let doc = a.get("Documents")?.call_obj(
        "Open",
        &[
            ("FileName", s(input)),
            ("ConfirmConversions", false.into()),
            ("ReadOnly", true.into()),
            ("AddToRecentFiles", false.into()),
            ("PasswordDocument", NO_PASSWORD.into()),
            ("WritePasswordDocument", NO_PASSWORD.into()),
            ("Visible", false.into()),
            ("NoEncodingDialog", true.into()),
        ],
    )?;
    let r = if to == "pdf" {
        doc.call(
            "ExportAsFixedFormat",
            &[
                ("OutputFileName", s(output)),
                ("ExportFormat", 17i32.into()), // wdExportFormatPDF
                ("OpenAfterExport", false.into()),
                ("OptimizeFor", 0i32.into()), // print quality
                ("IncludeDocProps", true.into()),
                ("CreateBookmarks", 1i32.into()),  // from headings
                ("DocStructureTags", true.into()), // tagged PDF
                ("BitmapMissingFonts", true.into()),
            ],
        )
    } else {
        doc.call(
            "SaveAs2",
            &[
                ("FileName", s(output)),
                ("FileFormat", 16i32.into()), // wdFormatDocumentDefault (.docx)
                ("AddToRecentFiles", false.into()),
            ],
        )
    };
    let _ = doc.call("Close", &[("SaveChanges", 0i32.into())]);
    r.map(|_| ())
}

fn excel(a: &Obj, input: &Path, output: &Path, to: &str) -> Result<(), EngineError> {
    a.put("Visible", false.into())?;
    a.put("DisplayAlerts", false.into())?;
    a.put("AskToUpdateLinks", false.into())?;
    a.put("AutomationSecurity", MACROS_OFF.into())?;
    let _ = a.put("ScreenUpdating", false.into());
    let wb = a.get("Workbooks")?.call_obj(
        "Open",
        &[
            ("Filename", s(input)),
            ("UpdateLinks", 0i32.into()),
            ("ReadOnly", true.into()),
            ("Password", NO_PASSWORD.into()),
            ("WriteResPassword", NO_PASSWORD.into()),
            ("IgnoreReadOnlyRecommended", true.into()),
            ("Notify", false.into()),
            ("AddToMru", false.into()),
        ],
    )?;
    let r = if to == "pdf" {
        wb.call(
            "ExportAsFixedFormat",
            &[
                ("Type", 0i32.into()), // xlTypePDF
                ("Filename", s(output)),
                ("Quality", 0i32.into()),
                ("IncludeDocProperties", true.into()),
                ("IgnorePrintAreas", false.into()),
                ("OpenAfterPublish", false.into()),
            ],
        )
    } else {
        wb.call(
            "SaveAs",
            &[
                ("Filename", s(output)),
                ("FileFormat", 51i32.into()), // xlOpenXMLWorkbook (.xlsx)
                ("AddToMru", false.into()),
            ],
        )
    };
    let _ = wb.call("Close", &[("SaveChanges", false.into())]);
    r.map(|_| ())
}

fn powerpoint(a: &Obj, input: &Path, output: &Path, to: &str) -> Result<(), EngineError> {
    a.put("DisplayAlerts", 1i32.into())?; // ppAlertsNone
    a.put("AutomationSecurity", MACROS_OFF.into())?;
    // PowerPoint takes an open password as "file::password".
    let name = format!("{}::{NO_PASSWORD}", input.display());
    let pres = a.get("Presentations")?.call_obj(
        "Open",
        &[
            ("FileName", VARIANT::from(BSTR::from(name.as_str()))),
            ("ReadOnly", (-1i32).into()), // msoTrue
            ("Untitled", 0i32.into()),
            ("WithWindow", 0i32.into()), // msoFalse: no window
        ],
    )?;
    let format = if to == "pdf" { 32i32 } else { 24i32 }; // ppSaveAsPDF, ppSaveAsOpenXMLPresentation
    let r = pres.call(
        "SaveAs",
        &[("FileName", s(output)), ("FileFormat", format.into())],
    );
    let _ = pres.call("Close", &[]);
    r.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;

    /// The late-binding layer against an automation object every Windows has.
    #[test]
    fn late_bound_calls_with_named_arguments() {
        // SAFETY: test-thread COM init; the dictionary is created and used on this thread.
        let o = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let id = HSTRING::from("Scripting.Dictionary");
            let clsid = CLSIDFromProgID(PCWSTR(id.as_ptr())).unwrap();
            Obj(CoCreateInstance::<_, IDispatch>(&clsid, None, CLSCTX_INPROC_SERVER).unwrap())
        };
        o.put("CompareMode", 1i32.into()).unwrap(); // text compare: keys ignore case
        o.call("Add", &[("Key", "Sample".into()), ("Item", 5i32.into())])
            .unwrap();
        assert_eq!(o.count("Count"), Some(1));
        let v = o.call("Exists", &[("Key", "SAMPLE".into())]).unwrap();
        assert!(bool::try_from(&v).unwrap());
        let Err(e) = o.call("Add", &[("Key", "sample".into()), ("Item", 1i32.into())]) else {
            panic!("a duplicate key is an error");
        };
        assert!(e.to_string().contains("Add"), "{e}");
        assert!(o.call("NoSuchMethod", &[]).is_err());
    }

    #[test]
    fn finds_processes() {
        assert!(!pids("explorer.exe").is_empty() || !pids("svchost.exe").is_empty());
        assert!(pids("no-such-program-swirvo.exe").is_empty());
    }
}
