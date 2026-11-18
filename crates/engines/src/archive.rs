//! Archive ⇄ archive by streaming entries: each entry is read from the source and written to
//! the target in one pass, with no temporary extraction (docs/05 §7).
//!
//! Safety: absolute paths, `..` components and symlinks are never written (zip-slip), and
//! timestamps and Unix permissions are carried across formats.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path};
use std::time::{Duration, SystemTime};

use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use swirvo_core::{CancelToken, Capability, Engine, EngineError, FastPath, Preset, StepRequest};
use zip::write::SimpleFileOptions;

pub struct ArchiveEngine;

#[cfg(feature = "zstd")]
const FORMATS: &[&str] = &["zip", "tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst"];
#[cfg(not(feature = "zstd"))]
const FORMATS: &[&str] = &["zip", "tar", "tar.gz", "tar.bz2", "tar.xz"];

impl Engine for ArchiveEngine {
    fn id(&self) -> &'static str {
        "archive"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for &from in FORMATS {
            for &to in FORMATS {
                if from != to {
                    caps.push(
                        Capability::new("archive", from, to)
                            .time(
                                10,
                                if to == "zip" || to == "tar.gz" {
                                    30
                                } else {
                                    12
                                },
                            )
                            .fast(FastPath::ObjectCopy)
                            .note("entries streamed, no temporary extraction"),
                    );
                }
            }
        }
        caps
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let mut sink = Sink::create(req.output, req.to, req.preset)?;
        for_each_entry(req.input, req.from, &mut |meta, data| {
            req.cancel.check()?;
            sink.add(&meta, data)
        })?;
        sink.finish()
    }
}

/// Archive formats this build can read and write.
pub fn formats() -> &'static [&'static str] {
    FORMATS
}

/// Read every entry to the end (CRC and stream integrity are checked by the decoders).
/// Returns (files, uncompressed bytes).
pub fn test(archive: &Path, from: &str, cancel: &CancelToken) -> Result<(usize, u64), EngineError> {
    let (mut files, mut total) = (0usize, 0u64);
    for_each_entry(archive, from, &mut |meta, data| {
        cancel.check()?;
        if !meta.dir {
            total += io::copy(data, &mut io::sink())?;
            files += 1;
        }
        Ok(())
    })?;
    Ok((files, total))
}

/// Zip-bomb limits for [`extract`].
pub const MAX_ENTRIES: usize = 200_000;
pub const MAX_TOTAL_BYTES: u64 = 64 << 30;

/// Characters Windows can't store in a name; ':' would also open an NTFS alternate stream.
fn portable_component(c: &str) -> bool {
    !c.is_empty()
        && !c
            .chars()
            .any(|ch| ch < ' ' || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\'))
        && !c.ends_with(['.', ' '])
}

/// Extract every safe entry of `archive` (format key `from`) under `dest`, an empty folder
/// the caller just created. Files are opened with `create_new`, so nothing that exists is
/// ever replaced. Symlinks, `..` and absolute paths are skipped (see [`safe_path`]).
/// Returns (files written, bytes written).
pub fn extract(
    archive: &Path,
    from: &str,
    dest: &Path,
    cancel: &CancelToken,
) -> Result<(usize, u64), EngineError> {
    let (mut files, mut total, mut seen) = (0usize, 0u64, 0usize);
    for_each_entry(archive, from, &mut |meta, data| {
        cancel.check()?;
        seen += 1;
        if seen > MAX_ENTRIES {
            return Err(EngineError::Invalid(format!(
                "archive has more than {MAX_ENTRIES} entries"
            )));
        }
        if cfg!(windows) && !meta.path.split('/').all(portable_component) {
            return Ok(()); // can't be represented safely on this file system
        }
        let target = meta
            .path
            .split('/')
            .fold(dest.to_path_buf(), |p, c| p.join(c));
        if meta.dir {
            fs::create_dir_all(&target)?;
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)?;
        let mut out = BufWriter::new(file);
        let n = io::copy(&mut data.take(MAX_TOTAL_BYTES - total + 1), &mut out)?;
        total += n;
        if total > MAX_TOTAL_BYTES {
            drop(out);
            let _ = fs::remove_file(&target);
            return Err(EngineError::Invalid(
                "archive expands past 64 GiB; stopped".into(),
            ));
        }
        let file = out.into_inner().map_err(|e| e.into_error())?;
        if meta.mtime > 0 {
            let _ = file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(meta.mtime));
        }
        files += 1;
        Ok(())
    })?;
    Ok((files, total))
}

#[derive(Debug, Clone)]
pub struct Meta {
    /// '/'-separated, relative, validated.
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub mode: u32,
    /// Unix seconds.
    pub mtime: u64,
}

/// Relative, `..`-free path with '/' separators, or None if unsafe.
fn safe_path(p: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

type EntryFn<'a> = dyn FnMut(Meta, &mut dyn Read) -> Result<(), EngineError> + 'a;

fn for_each_entry(path: &Path, from: &str, f: &mut EntryFn<'_>) -> Result<(), EngineError> {
    match from {
        "zip" => {
            let mut zip = zip::ZipArchive::new(BufReader::new(File::open(path)?))
                .map_err(EngineError::decode)?;
            for i in 0..zip.len() {
                let mut e = zip.by_index(i).map_err(EngineError::decode)?;
                if e.is_symlink() {
                    continue;
                }
                let Some(p) = e.enclosed_name().and_then(|p| safe_path(&p)) else {
                    continue;
                };
                let meta = Meta {
                    path: p,
                    dir: e.is_dir(),
                    size: e.size(),
                    mode: e
                        .unix_mode()
                        .unwrap_or(if e.is_dir() { 0o755 } else { 0o644 })
                        & 0o7777,
                    mtime: e.last_modified().map_or(0, zip_to_unix),
                };
                f(meta, &mut e)?;
            }
        }
        "tar" | "tar.gz" | "tar.bz2" | "tar.xz" | "tar.zst" if FORMATS.contains(&from) => {
            let file = BufReader::new(File::open(path)?);
            let reader: Box<dyn Read> = match from {
                "tar.gz" => Box::new(GzDecoder::new(file)),
                "tar.bz2" => Box::new(bzip2::read::MultiBzDecoder::new(file)),
                "tar.xz" => Box::new(lzma_rust2::XzReader::new(file, true)),
                #[cfg(feature = "zstd")]
                "tar.zst" => Box::new(zstd::Decoder::with_buffer(file)?),
                _ => Box::new(file),
            };
            let mut ar = tar::Archive::new(reader);
            for e in ar.entries().map_err(EngineError::decode)? {
                let mut e = e.map_err(EngineError::decode)?;
                let ty = e.header().entry_type();
                if !(ty.is_file() || ty.is_dir()) {
                    continue; // symlinks, hard links, devices
                }
                let Some(p) = e.path().ok().and_then(|p| safe_path(&p)) else {
                    continue;
                };
                let h = e.header();
                let meta = Meta {
                    path: p,
                    dir: ty.is_dir(),
                    size: h.size().unwrap_or(0),
                    mode: h.mode().unwrap_or(0o644) & 0o7777,
                    mtime: h.mtime().unwrap_or(0),
                };
                f(meta, &mut e)?;
            }
        }
        other => {
            return Err(EngineError::Invalid(format!(
                "archive engine can't read {other}"
            )));
        }
    }
    Ok(())
}

/// A writer that must be finalized (compression trailers).
trait Finish: Write {
    fn finish_box(self: Box<Self>) -> io::Result<()>;
}
impl Finish for BufWriter<File> {
    fn finish_box(mut self: Box<Self>) -> io::Result<()> {
        self.flush()
    }
}
impl Finish for GzEncoder<BufWriter<File>> {
    fn finish_box(self: Box<Self>) -> io::Result<()> {
        (*self).finish()?.flush()
    }
}
impl Finish for bzip2::write::BzEncoder<BufWriter<File>> {
    fn finish_box(self: Box<Self>) -> io::Result<()> {
        (*self).finish()?.flush()
    }
}
impl Finish for lzma_rust2::XzWriter<BufWriter<File>> {
    fn finish_box(self: Box<Self>) -> io::Result<()> {
        (*self).finish()?.flush()
    }
}
#[cfg(feature = "zstd")]
impl Finish for zstd::Encoder<'static, BufWriter<File>> {
    fn finish_box(self: Box<Self>) -> io::Result<()> {
        (*self).finish()?.flush()
    }
}

enum Sink {
    Zip(Box<zip::ZipWriter<BufWriter<File>>>, SimpleFileOptions),
    Tar(tar::Builder<Box<dyn Finish>>),
}

impl Sink {
    fn create(path: &Path, to: &str, preset: Preset) -> Result<Sink, EngineError> {
        let out = BufWriter::new(File::create(path)?);
        Ok(match to {
            "zip" => {
                let level = match preset {
                    Preset::Fastest => 1,
                    Preset::Balanced => 6,
                    Preset::Best | Preset::Smallest => 9,
                };
                let opts = SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated)
                    .compression_level(Some(level))
                    .large_file(false);
                Sink::Zip(Box::new(zip::ZipWriter::new(out)), opts)
            }
            "tar" | "tar.gz" | "tar.bz2" | "tar.xz" | "tar.zst" if FORMATS.contains(&to) => {
                let w: Box<dyn Finish> = match to {
                    "tar.gz" => Box::new(GzEncoder::new(
                        out,
                        match preset {
                            Preset::Fastest => Compression::fast(),
                            Preset::Balanced => Compression::default(),
                            _ => Compression::best(),
                        },
                    )),
                    "tar.bz2" => Box::new(bzip2::write::BzEncoder::new(
                        out,
                        match preset {
                            Preset::Fastest => bzip2::Compression::fast(),
                            _ => bzip2::Compression::best(),
                        },
                    )),
                    "tar.xz" => Box::new(
                        lzma_rust2::XzWriter::new(
                            out,
                            lzma_rust2::XzOptions::with_preset(match preset {
                                Preset::Fastest => 1,
                                Preset::Balanced => 6,
                                _ => 9,
                            }),
                        )
                        .map_err(EngineError::encode)?,
                    ),
                    #[cfg(feature = "zstd")]
                    "tar.zst" => {
                        let level = match preset {
                            Preset::Fastest => 3,
                            Preset::Balanced => 9,
                            _ => 19,
                        };
                        let mut enc = zstd::Encoder::new(out, level)?;
                        // Multithreaded zstd isn't enabled in this build; long-distance matching helps big archives.
                        enc.long_distance_matching(true)?;
                        Box::new(enc)
                    }
                    _ => Box::new(out),
                };
                let mut b = tar::Builder::new(w);
                b.mode(tar::HeaderMode::Complete);
                Sink::Tar(b)
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "archive engine can't write {other}"
                )));
            }
        })
    }

    fn add(&mut self, m: &Meta, data: &mut dyn Read) -> Result<(), EngineError> {
        match self {
            Sink::Zip(z, base) => {
                let mut o = base
                    .unix_permissions(m.mode)
                    .large_file(m.size >= u32::MAX as u64);
                if let Some(t) = unix_to_zip(m.mtime) {
                    o = o.last_modified_time(t);
                }
                if m.dir {
                    z.add_directory(m.path.as_str(), o)
                        .map_err(EngineError::encode)?;
                } else {
                    z.start_file(m.path.as_str(), o)
                        .map_err(EngineError::encode)?;
                    io::copy(data, z)?;
                }
            }
            Sink::Tar(b) => {
                let mut h = tar::Header::new_gnu();
                h.set_mode(m.mode);
                h.set_mtime(m.mtime);
                if m.dir {
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                    b.append_data(&mut h, format!("{}/", m.path), io::empty())?;
                } else {
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_size(m.size);
                    b.append_data(&mut h, &m.path, data)?;
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<(), EngineError> {
        match self {
            Sink::Zip(z, _) => {
                z.finish().map_err(EngineError::encode)?.flush()?;
            }
            Sink::Tar(b) => b.into_inner()?.finish_box()?,
        }
        Ok(())
    }
}

// --- civil date conversions (Howard Hinnant's algorithms), for ZIP's DOS timestamps ---

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn zip_to_unix(t: zip::DateTime) -> u64 {
    let days = days_from_civil(t.year() as i64, t.month() as u32, t.day() as u32);
    (days * 86_400 + t.hour() as i64 * 3600 + t.minute() as i64 * 60 + t.second() as i64).max(0)
        as u64
}

fn unix_to_zip(secs: u64) -> Option<zip::DateTime> {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    zip::DateTime::from_date_and_time(
        u16::try_from(y).ok()?,
        m as u8,
        d as u8,
        (rem / 3600) as u8,
        (rem / 60 % 60) as u8,
        (rem % 60) as u8,
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(days_from_civil(2026, 11, 7)), (2026, 11, 7));
        assert_eq!(civil_from_days(days_from_civil(2000, 2, 29)), (2000, 2, 29));
    }

    #[test]
    fn extract_never_replaces_and_skips_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let o = SimpleFileOptions::default();
            z.start_file("docs/readme.txt", o).unwrap();
            z.write_all(b"hello").unwrap();
            z.start_file("../evil.txt", o).unwrap();
            z.write_all(b"nope").unwrap();
            z.finish().unwrap();
        }
        let dest = dir.path().join("out");
        fs::create_dir(&dest).unwrap();
        let cancel = CancelToken::new();
        assert_eq!(extract(&zip_path, "zip", &dest, &cancel).unwrap(), (1, 5));
        assert_eq!(fs::read(dest.join("docs/readme.txt")).unwrap(), b"hello");
        assert!(!dir.path().join("evil.txt").exists());
        // A second run into the same folder must fail rather than overwrite.
        fs::write(dest.join("docs/readme.txt"), b"mine").unwrap();
        assert!(extract(&zip_path, "zip", &dest, &cancel).is_err());
        assert_eq!(fs::read(dest.join("docs/readme.txt")).unwrap(), b"mine");
        assert!(
            portable_component("a.txt")
                && !portable_component("a.txt:ads")
                && !portable_component("dot.")
        );
    }

    #[test]
    fn tar_bz2_and_xz_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            z.start_file("x/readme.txt", SimpleFileOptions::default())
                .unwrap();
            z.write_all(&b"sample ".repeat(1000)).unwrap();
            z.finish().unwrap();
        }
        let cancel = CancelToken::new();
        for to in ["tar.bz2", "tar.xz"] {
            let out = dir.path().join(format!("a.{to}"));
            ArchiveEngine
                .run(&StepRequest {
                    input: &zip_path,
                    output: &out,
                    from: "zip",
                    to,
                    preset: Preset::Balanced,
                    pdf: Default::default(),
                    cancel: &cancel,
                })
                .unwrap();
            assert_eq!(test(&out, to, &cancel).unwrap(), (1, 7000), "{to}");
            assert!(
                std::fs::metadata(&out).unwrap().len() < 2000,
                "{to} should compress"
            );
        }
    }

    #[test]
    fn rejects_traversal() {
        assert_eq!(
            safe_path(Path::new("a/./b.txt")).as_deref(),
            Some("a/b.txt")
        );
        assert!(safe_path(Path::new("../etc/passwd")).is_none());
        assert!(safe_path(Path::new("/abs")).is_none());
    }

    #[cfg(feature = "zstd")]
    #[test]
    fn zip_to_tar_zst_to_zip_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let o = SimpleFileOptions::default().last_modified_time(
                zip::DateTime::from_date_and_time(2024, 5, 6, 7, 8, 10).unwrap(),
            );
            z.add_directory("docs/", o).unwrap();
            z.start_file("docs/readme.txt", o).unwrap();
            z.write_all(b"hello swirvo").unwrap();
            z.start_file("../evil.txt", o).unwrap();
            z.write_all(b"nope").unwrap();
            z.finish().unwrap();
        }
        let cancel = CancelToken::new();
        let run = |input: &Path, output: &Path, from: &str, to: &str| {
            ArchiveEngine
                .run(&StepRequest {
                    input,
                    output,
                    from,
                    to,
                    preset: Preset::Balanced,
                    pdf: Default::default(),
                    cancel: &cancel,
                })
                .unwrap()
        };
        let tzst = dir.path().join("a.tar.zst");
        let back = dir.path().join("b.zip");
        run(&zip_path, &tzst, "zip", "tar.zst");
        run(&tzst, &back, "tar.zst", "zip");

        let mut z = zip::ZipArchive::new(File::open(&back).unwrap()).unwrap();
        let names: Vec<String> = (0..z.len())
            .map(|i| z.by_index(i).unwrap().name().unwrap().to_string())
            .collect();
        assert_eq!(
            names,
            vec!["docs/", "docs/readme.txt"],
            "traversal entry must be dropped"
        );
        let mut f = z.by_name("docs/readme.txt").unwrap();
        let t = f.last_modified().unwrap();
        assert_eq!(
            (t.year(), t.month(), t.day(), t.hour(), t.minute()),
            (2024, 5, 6, 7, 8)
        );
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "hello swirvo");
    }
}
