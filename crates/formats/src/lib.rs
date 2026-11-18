//! Format registry and detection.
//!
//! * The registry is built once (a few µs) and gives O(1) lookups by key and extension,
//!   including compound extensions such as `.tar.gz`.
//! * Detection combines the extension guess (instant, what the wheel shows first) with a
//!   content sniff of the first 4 KiB (docs/04 §1).

mod detect;
mod table;

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use swirvo_core::{Category, FormatId};

pub use detect::{Confidence, Detection, detect_bytes, detect_path, sniff};
pub use table::{FORMATS, FormatDef};

/// Lookup tables over [`FORMATS`].
pub struct Registry {
    defs: &'static [FormatDef],
    by_key: HashMap<&'static str, FormatId>,
    by_ext: HashMap<&'static str, FormatId>,
    /// Compound extensions ("tar.gz"), longest first, for suffix matching.
    compound: Vec<(&'static str, FormatId)>,
}

/// The process-wide registry.
pub fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| Registry::new(FORMATS))
}

impl Registry {
    fn new(defs: &'static [FormatDef]) -> Self {
        assert!(
            defs.len() <= 256,
            "reachability bitsets hold at most 256 formats"
        );
        let mut by_key = HashMap::with_capacity(defs.len());
        let mut by_ext = HashMap::with_capacity(defs.len() * 2);
        let mut compound = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            let id = FormatId(i as u16);
            let dup = by_key.insert(d.key, id);
            assert!(dup.is_none(), "duplicate format key {}", d.key);
            for &e in d.exts {
                let dup = by_ext.insert(e, id);
                assert!(dup.is_none(), "extension .{e} claimed twice");
                if e.contains('.') {
                    compound.push((e, id));
                }
            }
        }
        compound.sort_by_key(|(e, _)| std::cmp::Reverse(e.len()));
        Registry {
            defs,
            by_key,
            by_ext,
            compound,
        }
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }
    pub fn get(&self, id: FormatId) -> &'static FormatDef {
        &self.defs[id.index()]
    }
    pub fn key(&self, id: FormatId) -> &'static str {
        self.defs[id.index()].key
    }
    /// Look up by key, or by any extension, case-insensitively ("JPG", "jpeg", "tgz").
    pub fn find(&self, name: &str) -> Option<FormatId> {
        let n = name.trim_start_matches('.').to_ascii_lowercase();
        self.by_key
            .get(n.as_str())
            .or_else(|| self.by_ext.get(n.as_str()))
            .copied()
    }
    pub fn id(&self, key: &str) -> Option<FormatId> {
        self.by_key.get(key).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = (FormatId, &'static FormatDef)> + '_ {
        self.defs
            .iter()
            .enumerate()
            .map(|(i, d)| (FormatId(i as u16), d))
    }
    pub fn in_category(
        &self,
        c: Category,
    ) -> impl Iterator<Item = (FormatId, &'static FormatDef)> + '_ {
        self.iter().filter(move |(_, d)| d.category == c)
    }

    /// Extension-based guess for a path; handles compound extensions.
    pub fn from_path(&self, path: &Path) -> Option<FormatId> {
        let name = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        for (ext, id) in &self.compound {
            if name.len() > ext.len() + 1
                && name.ends_with(ext)
                && name.as_bytes()[name.len() - ext.len() - 1] == b'.'
            {
                return Some(*id);
            }
        }
        let ext = Path::new(&name).extension()?.to_str()?;
        self.by_ext.get(ext).copied()
    }

    /// File name without its (possibly compound) format extension: "a.tar.gz" → "a".
    pub fn stem(&self, path: &Path) -> String {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        for (ext, _) in &self.compound {
            if lower.len() > ext.len() + 1
                && lower.ends_with(ext)
                && lower.as_bytes()[lower.len() - ext.len() - 1] == b'.'
            {
                return name[..name.len() - ext.len() - 1].to_string();
            }
        }
        match name.rfind('.') {
            Some(i) if i > 0 => name[..i].to_string(),
            _ => name,
        }
    }

    /// Extension used for outputs of a format.
    pub fn primary_ext(&self, id: FormatId) -> &'static str {
        self.defs[id.index()].exts[0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        let r = registry();
        assert_eq!(r.key(r.find("JPEG").unwrap()), "jpg");
        assert_eq!(r.key(r.find(".tgz").unwrap()), "tar.gz");
        assert_eq!(
            r.key(r.from_path(Path::new("/x/Backup.TAR.GZ")).unwrap()),
            "tar.gz"
        );
        assert_eq!(r.key(r.from_path(Path::new("photo.heic")).unwrap()), "heic");
        assert!(r.from_path(Path::new("README")).is_none());
        // A dotfile named ".tar.gz" is a gzip of ".tar", not a compressed tarball named "".
        assert_eq!(r.key(r.from_path(Path::new(".tar.gz")).unwrap()), "gz");
    }

    #[test]
    fn stems() {
        let r = registry();
        assert_eq!(r.stem(Path::new("a.b.tar.zst")), "a.b");
        assert_eq!(r.stem(Path::new("IMG_2041.HEIC")), "IMG_2041");
        assert_eq!(r.stem(Path::new(".bashrc")), ".bashrc");
    }
}
