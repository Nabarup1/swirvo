//! Text input helpers: BOM stripping and legacy-encoding fallback.

use std::path::Path;

use swirvo_core::EngineError;

/// Read a text file as UTF-8, falling back to Windows-1252 (the usual culprit for
/// subtitles and CSVs exported on Windows), so the output is always valid UTF-8.
pub fn read_text(path: &Path) -> Result<String, EngineError> {
    Ok(from_bytes(&std::fs::read(path)?))
}

/// Decode text bytes: UTF-8 (BOM optional), UTF-16 with BOM, else Windows-1252.
pub fn from_bytes(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return encoding_rs::UTF_16LE.decode(rest).0.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return encoding_rs::UTF_16BE.decode(rest).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_legacy_and_bom() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        std::fs::write(&p, b"caf\xE9").unwrap();
        assert_eq!(read_text(&p).unwrap(), "café");
        std::fs::write(&p, b"\xEF\xBB\xBFhi").unwrap();
        assert_eq!(read_text(&p).unwrap(), "hi");
    }
}
