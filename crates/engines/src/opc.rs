//! Office Open XML packages (DOCX, PPTX, XLSX): a ZIP of XML parts linked by
//! relationship files. Reading is capped per part and in total, so a hostile archive
//! can't make us inflate gigabytes.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use swirvo_core::EngineError;

use crate::docxlayout::xml::{self, El};

/// Per-part size cap.
const PART_LIMIT: u64 = 96 << 20;
/// Everything read from one package.
const TOTAL_LIMIT: u64 = 512 << 20;

pub struct Package<'a> {
    zip: zip::ZipArchive<Cursor<&'a [u8]>>,
    read: u64,
}

/// A part's relationships: id → (target, type, external).
#[derive(Debug, Default, Clone)]
pub struct Rels {
    map: HashMap<String, Rel>,
}

#[derive(Debug, Clone)]
pub struct Rel {
    pub target: String,
    pub kind: String,
    pub external: bool,
}

impl Rels {
    pub fn get(&self, id: &str) -> Option<&Rel> {
        self.map.get(id)
    }

    /// The internal target of relationship `id`.
    pub fn target(&self, id: &str) -> Option<&str> {
        self.map
            .get(id)
            .filter(|r| !r.external)
            .map(|r| r.target.as_str())
    }

    /// The first relationship whose type ends with `suffix` (e.g. "/slideLayout").
    pub fn of_type(&self, suffix: &str) -> Option<&Rel> {
        let mut v: Vec<(&String, &Rel)> = self
            .map
            .iter()
            .filter(|(_, r)| r.kind.ends_with(suffix))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v.first().map(|(_, r)| *r)
    }
}

pub fn dir_of(part: &str) -> &str {
    part.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Resolve a relative target against a folder (`..` and `.` handled; `/x` is absolute).
pub fn join(base: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let mut parts: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

impl<'a> Package<'a> {
    pub fn open(bytes: &'a [u8]) -> Result<Package<'a>, EngineError> {
        Ok(Package {
            zip: zip::ZipArchive::new(Cursor::new(bytes)).map_err(EngineError::decode)?,
            read: 0,
        })
    }

    pub fn bytes(&mut self, name: &str) -> Option<Vec<u8>> {
        if self.read > TOTAL_LIMIT {
            return None;
        }
        let f = self.zip.by_name(name).ok()?;
        let mut v = Vec::new();
        f.take(PART_LIMIT).read_to_end(&mut v).ok()?;
        self.read += v.len() as u64;
        Some(v)
    }

    pub fn xml(&mut self, name: &str) -> Option<El> {
        let b = self.bytes(name)?;
        let s = String::from_utf8_lossy(&b);
        xml::parse(s.trim_start_matches('\u{feff}')).ok()
    }

    pub fn rels(&mut self, part: &str) -> Rels {
        let dir = dir_of(part);
        let file = part.rsplit('/').next().unwrap_or(part);
        let path = if dir.is_empty() {
            format!("_rels/{file}.rels")
        } else {
            format!("{dir}/_rels/{file}.rels")
        };
        let mut r = Rels::default();
        if let Some(root) = self.xml(&path) {
            for rel in root.all("Relationship") {
                let (Some(id), Some(t)) = (rel.attr("Id"), rel.attr("Target")) else {
                    continue;
                };
                let external = rel.attr("TargetMode") == Some("External");
                let target = if external {
                    t.to_string()
                } else {
                    join(dir, t)
                };
                r.map.insert(
                    id.to_string(),
                    Rel {
                        target,
                        kind: rel.attr("Type").unwrap_or("").to_string(),
                        external,
                    },
                );
            }
        }
        r
    }

    /// The package's main part (the target of the root `officeDocument` relationship).
    pub fn main_part(&mut self, fallback: &str) -> String {
        self.rels("")
            .of_type("/officeDocument")
            .map(|r| r.target.clone())
            .unwrap_or_else(|| fallback.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_targets() {
        assert_eq!(join("ppt/slides", "../media/a.png"), "ppt/media/a.png");
        assert_eq!(join("ppt", "slides/s1.xml"), "ppt/slides/s1.xml");
        assert_eq!(join("ppt/slides", "/ppt/x.xml"), "ppt/x.xml");
        assert_eq!(join("", "word/document.xml"), "word/document.xml");
    }
}
