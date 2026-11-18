//! Content sniffing: a byte trie over magic signatures plus container probes
//! (ISO-BMFF brands, ZIP first entry, RIFF FourCC, EBML doctype, Ogg codec headers).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::OnceLock;

use swirvo_core::{FormatId, Traits};

use crate::registry;

/// How sure detection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Only the extension was available or usable.
    Extension,
    /// A short or ambiguous signature (BMP's "BM", MP3 frame sync, generic text).
    Weak,
    /// An unambiguous signature or container probe.
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    pub format: FormatId,
    pub confidence: Confidence,
    /// What the extension alone suggested.
    pub ext_guess: Option<FormatId>,
    /// The content disagrees with the extension (e.g. a ".jpg" that is really HEIC).
    pub mismatch: bool,
}

#[derive(Clone, Copy)]
enum Probe {
    None,
    Zip,
    IsoBmff,
    Riff,
    Ebml,
    Ogg,
    Form,
}

struct Sig {
    offset: usize,
    bytes: &'static [u8],
    key: &'static str,
    conf: Confidence,
    probe: Probe,
}

const fn s(
    offset: usize,
    bytes: &'static [u8],
    key: &'static str,
    conf: Confidence,
    probe: Probe,
) -> Sig {
    Sig {
        offset,
        bytes,
        key,
        conf,
        probe,
    }
}

use Confidence::{Strong as S, Weak as W};

static SIGS: &[Sig] = &[
    s(0, b"\xFF\xD8\xFF", "jpg", S, Probe::None),
    s(0, b"\x89PNG\r\n\x1a\n", "png", S, Probe::None),
    s(0, b"GIF87a", "gif", S, Probe::None),
    s(0, b"GIF89a", "gif", S, Probe::None),
    s(0, b"BM", "bmp", W, Probe::None),
    s(0, b"II*\0", "tiff", S, Probe::None),
    s(0, b"MM\0*", "tiff", S, Probe::None),
    s(0, b"\0\0\x01\0", "ico", W, Probe::None),
    s(0, b"qoif", "qoi", S, Probe::None),
    s(0, b"8BPS", "psd", S, Probe::None),
    s(0, b"\xFF\x0A", "jxl", S, Probe::None),
    s(0, b"\0\0\0\x0CJXL \r\n\x87\n", "jxl", S, Probe::None),
    s(0, b"P1", "pnm", W, Probe::None),
    s(0, b"P2", "pnm", W, Probe::None),
    s(0, b"P3", "pnm", W, Probe::None),
    s(0, b"P4", "pnm", W, Probe::None),
    s(0, b"P5", "pnm", W, Probe::None),
    s(0, b"P6", "pnm", W, Probe::None),
    s(0, b"P7", "pnm", W, Probe::None),
    s(0, b"RIFF", "wav", S, Probe::Riff),
    s(0, b"FORM", "aiff", S, Probe::Form),
    s(4, b"ftyp", "mp4", S, Probe::IsoBmff),
    s(0, b"\x1A\x45\xDF\xA3", "mkv", S, Probe::Ebml),
    s(0, b"OggS", "ogg", S, Probe::Ogg),
    s(0, b"fLaC", "flac", S, Probe::None),
    s(0, b"ID3", "mp3", S, Probe::None),
    s(0, b"\xFF\xFB", "mp3", W, Probe::None),
    s(0, b"\xFF\xF3", "mp3", W, Probe::None),
    s(0, b"\xFF\xF2", "mp3", W, Probe::None),
    s(0, b"\xFF\xF1", "aac", W, Probe::None),
    s(0, b"\xFF\xF9", "aac", W, Probe::None),
    s(0, b"#!AMR", "amr", S, Probe::None),
    s(0, b"\x0B\x77", "ac3", W, Probe::None),
    s(0, b"caff", "caf", S, Probe::None),
    s(0, b"FLV\x01", "flv", S, Probe::None),
    s(
        0,
        b"\x30\x26\xB2\x75\x8E\x66\xCF\x11",
        "wmv",
        S,
        Probe::None,
    ),
    s(0, b"%PDF-", "pdf", S, Probe::None),
    // EMF: record type 1 (the header) with " EMF" at byte 40.
    s(40, b" EMF", "emf", S, Probe::None),
    // WMF: the placeable header's key, or a bare memory/disk metafile header.
    s(0, b"\xD7\xCD\xC6\x9A", "wmf", S, Probe::None),
    s(0, b"\x01\x00\x09\x00\x00\x03", "wmf", W, Probe::None),
    s(0, b"\x02\x00\x09\x00\x00\x03", "wmf", W, Probe::None),
    s(0, b"{\\rtf", "rtf", S, Probe::None),
    s(
        0,
        b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1",
        "doc",
        S,
        Probe::None,
    ),
    s(0, b"PK\x03\x04", "zip", S, Probe::Zip),
    s(0, b"PK\x05\x06", "zip", S, Probe::None),
    s(0, b"7z\xBC\xAF\x27\x1C", "7z", S, Probe::None),
    s(0, b"Rar!\x1A\x07", "rar", S, Probe::None),
    s(0, b"\x1F\x8B", "gz", S, Probe::None),
    s(0, b"\xFD7zXZ\0", "xz", S, Probe::None),
    s(0, b"\x28\xB5\x2F\xFD", "zst", S, Probe::None),
    s(0, b"BZh", "bz2", W, Probe::None),
    s(0, b"MSCF", "cab", S, Probe::None),
    s(257, b"ustar", "tar", S, Probe::None),
    s(0, b"\0\x01\0\0", "ttf", W, Probe::None),
    s(0, b"OTTO", "otf", S, Probe::None),
    s(0, b"wOFF", "woff", S, Probe::None),
    s(0, b"wOF2", "woff2", S, Probe::None),
    s(0, b"PAR1", "parquet", S, Probe::None),
    s(60, b"BOOKMOBI", "mobi", S, Probe::None),
];

/// Byte trie over every offset-0 signature. Children are kept sorted and searched with
/// binary search, so a lookup costs O(L · log σ) with L ≤ 12 here.
struct Trie {
    nodes: Vec<Node>,
}

#[derive(Default)]
struct Node {
    next: Vec<(u8, u32)>,
    term: Option<u16>,
}

impl Trie {
    fn build() -> Trie {
        let mut t = Trie {
            nodes: vec![Node::default()],
        };
        for (i, sig) in SIGS.iter().enumerate().filter(|(_, s)| s.offset == 0) {
            let mut cur = 0usize;
            for &b in sig.bytes {
                cur = match t.nodes[cur].next.binary_search_by_key(&b, |&(k, _)| k) {
                    Ok(j) => t.nodes[cur].next[j].1 as usize,
                    Err(j) => {
                        let id = t.nodes.len() as u32;
                        t.nodes.push(Node::default());
                        t.nodes[cur].next.insert(j, (b, id));
                        id as usize
                    }
                };
            }
            t.nodes[cur].term.get_or_insert(i as u16);
        }
        t
    }

    /// Index of the longest signature that prefixes `data`.
    fn longest(&self, data: &[u8]) -> Option<u16> {
        let mut cur = 0usize;
        let mut best = self.nodes[0].term;
        for &b in data {
            match self.nodes[cur].next.binary_search_by_key(&b, |&(k, _)| k) {
                Ok(j) => {
                    cur = self.nodes[cur].next[j].1 as usize;
                    if self.nodes[cur].term.is_some() {
                        best = self.nodes[cur].term;
                    }
                }
                Err(_) => break,
            }
        }
        best
    }
}

fn trie() -> &'static Trie {
    static T: OnceLock<Trie> = OnceLock::new();
    T.get_or_init(Trie::build)
}

fn find(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn probe_iso_bmff(h: &[u8]) -> &'static str {
    let brand = |b: &[u8]| -> Option<&'static str> {
        Some(match b {
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"heim" | b"heis" => "heic",
            b"avif" | b"avis" => "avif",
            b"qt  " => "mov",
            b"M4A " | b"M4B " | b"M4P " => "m4a",
            b"crx " => "cr3",
            b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" => "3gp",
            b"jxl " => "jxl",
            _ => return None,
        })
    };
    if h.len() < 12 {
        return "mp4";
    }
    if let Some(k) = brand(&h[8..12]) {
        return k;
    }
    // mif1/msf1 and generic ISO brands: look at compatible brands.
    let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
    let end = size.clamp(16, h.len());
    let mut i = 16;
    let mut found = None;
    while i + 4 <= end {
        if let Some(k) = brand(&h[i..i + 4]) {
            // AVIF files also list mif1; prefer the most specific still-image brand.
            if k == "avif" {
                return "avif";
            }
            found.get_or_insert(k);
        }
        i += 4;
    }
    found.unwrap_or("mp4")
}

fn probe_zip(h: &[u8]) -> &'static str {
    // EPUB and ODF store an uncompressed "mimetype" entry first.
    if h.len() > 38 && &h[30..38] == b"mimetype" {
        let extra = u16::from_le_bytes([h[28], h[29]]) as usize;
        let start = 38 + extra;
        let body = &h[start.min(h.len())..h.len().min(start + 80)];
        if body.starts_with(b"application/epub+zip") {
            return "epub";
        }
        if body.starts_with(b"application/vnd.oasis.opendocument.text") {
            return "odt";
        }
        if body.starts_with(b"application/vnd.oasis.opendocument.spreadsheet") {
            return "ods";
        }
        if body.starts_with(b"application/vnd.oasis.opendocument.presentation") {
            return "odp";
        }
    }
    // OOXML: entry names in local headers near the start.
    if find(h, b"word/") {
        "docx"
    } else if find(h, b"xl/") {
        "xlsx"
    } else if find(h, b"ppt/") {
        "pptx"
    } else {
        "zip"
    }
}

fn probe(p: Probe, h: &[u8], default: &'static str) -> &'static str {
    match p {
        Probe::None => default,
        Probe::Zip => probe_zip(h),
        Probe::IsoBmff => probe_iso_bmff(h),
        Probe::Riff => match h.get(8..12) {
            Some(b"WAVE") => "wav",
            Some(b"AVI ") => "avi",
            Some(b"WEBP") => "webp",
            _ => default,
        },
        Probe::Ebml => {
            if find(&h[..h.len().min(64)], b"webm") {
                "webm"
            } else {
                "mkv"
            }
        }
        Probe::Ogg => {
            if find(h, b"OpusHead") {
                "opus"
            } else if find(h, b"\x80theora") {
                "ogv"
            } else {
                "ogg"
            }
        }
        Probe::Form => match h.get(8..12) {
            Some(b"AIFF") | Some(b"AIFC") => "aiff",
            _ => default,
        },
    }
}

/// Text heuristics on a UTF-8 head. Returns a format key and whether it is confident.
fn sniff_text(h: &[u8]) -> Option<(&'static str, Confidence)> {
    let h = h.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(h);
    // Allow a truncated multi-byte char at the very end of the window.
    let text = match std::str::from_utf8(h) {
        Ok(t) => t,
        Err(e) if e.valid_up_to() + 4 >= h.len() => {
            std::str::from_utf8(&h[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    if text
        .chars()
        .any(|c| c == '\0' || (c.is_control() && !matches!(c, '\n' | '\r' | '\t' | '\x0c')))
    {
        return None;
    }
    let t = text.trim_start();
    let lower: String = t.chars().take(256).collect::<String>().to_ascii_lowercase();
    if t.starts_with("WEBVTT") {
        return Some(("vtt", S));
    }
    if t.starts_with("[Script Info]") {
        return Some(("ass", S));
    }
    if lower.starts_with("<?xml") || lower.starts_with('<') {
        if lower.contains("<svg") {
            return Some(("svg", S));
        }
        if lower.starts_with("<!doctype html") || lower.starts_with("<html") {
            return Some(("html", S));
        }
        if lower.contains("<tt") && lower.contains("ttml") {
            return Some(("ttml", S));
        }
        return Some(("xml", W));
    }
    if t.starts_with('{') || t.starts_with('[') {
        let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).take(4).collect();
        if lines.len() > 1
            && lines
                .iter()
                .all(|l| l.trim_start().starts_with('{') && l.trim_end().ends_with('}'))
        {
            return Some(("ndjson", W));
        }
        return Some(("json", W));
    }
    // SRT: "1\n00:00:01,000 --> 00:00:02,000"
    let mut ls = t.lines();
    if let (Some(a), Some(b)) = (ls.next(), ls.next())
        && a.trim().chars().all(|c| c.is_ascii_digit())
        && b.contains("-->")
    {
        return Some(("srt", S));
    }
    Some(("txt", W))
}

/// Sniff a head buffer. Returns a format key and confidence.
pub fn sniff(head: &[u8]) -> Option<(&'static str, Confidence)> {
    if let Some(i) = trie().longest(head) {
        let sig = &SIGS[i as usize];
        return Some((probe(sig.probe, head, sig.key), sig.conf));
    }
    for sig in SIGS.iter().filter(|s| s.offset > 0) {
        if head.len() >= sig.offset + sig.bytes.len()
            && &head[sig.offset..sig.offset + sig.bytes.len()] == sig.bytes
        {
            return Some((probe(sig.probe, head, sig.key), sig.conf));
        }
    }
    // MPEG-TS: sync byte every 188 bytes.
    if head.len() > 376 && head[0] == 0x47 && head[188] == 0x47 && head[376] == 0x47 {
        return Some(("ts", S));
    }
    sniff_text(head)
}

/// Combine the extension guess with a content sniff (docs/04 §1).
pub fn detect_bytes(head: &[u8], ext_guess: Option<FormatId>) -> Option<Detection> {
    let reg = registry();
    let sniffed = sniff(head).and_then(|(k, c)| reg.id(k).map(|id| (id, c)));
    let ext_def = ext_guess.map(|id| reg.get(id));
    let (format, confidence) = match (sniffed, ext_guess) {
        (None, None) => return None,
        (None, Some(e)) => (e, Confidence::Extension),
        (Some((s, c)), None) => (s, c),
        (Some((s, c)), Some(e)) => {
            let sdef = reg.get(s);
            let edef = ext_def.expect("ext_guess is Some");
            let ext_compatible =
                // Generic sniffs that a more specific extension refines.
                (sdef.key == "zip" && edef.traits.has(Traits::ZIP_FAMILY))
                || (sdef.key == "doc" && matches!(edef.key, "xls" | "ppt" | "doc"))
                || (sdef.key == "wmv" && edef.key == "wma")
                || (sdef.key == "mp4" && matches!(edef.key, "m4a" | "mov" | "3gp"))
                || (sdef.key == "gz" && edef.key == "tar.gz")
                || (sdef.key == "xz" && edef.key == "tar.xz")
                || (sdef.key == "zst" && edef.key == "tar.zst")
                || (sdef.key == "bz2" && edef.key == "tar.bz2")
                || (sdef.key == "ttf" && edef.key == "otf")
                || (sdef.key == "ogg" && edef.key == "oga")
                // Weak sniffs defer to an extension in the same family, and generic
                // text defers to any text-based format.
                || (c == Confidence::Weak && (sdef.category == edef.category
                    || (sdef.key == "txt" && edef.traits.has(Traits::TEXT))
                    || (sdef.key == "xml" && edef.traits.has(Traits::TEXT))));
            if s == e || ext_compatible {
                (e, c.max(Confidence::Weak))
            } else {
                // Content wins: a ".ts" that sniffs as text is TypeScript, not MPEG-TS.
                (s, c)
            }
        }
    };
    Some(Detection {
        format,
        confidence,
        ext_guess,
        mismatch: ext_guess.is_some_and(|e| e != format),
    })
}

/// Detect a file on disk with one bounded read (4 KiB covers every probe above).
pub fn detect_path(path: &Path) -> io::Result<Option<Detection>> {
    let mut head = Vec::with_capacity(4096);
    File::open(path)?.take(4096).read_to_end(&mut head)?;
    let ext = registry().from_path(path);
    if head.is_empty() {
        return Ok(ext.map(|e| Detection {
            format: e,
            confidence: Confidence::Extension,
            ext_guess: Some(e),
            mismatch: false,
        }));
    }
    Ok(detect_bytes(&head, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(d: Option<Detection>) -> &'static str {
        registry().key(d.expect("detected").format)
    }
    fn ext(e: &str) -> Option<FormatId> {
        registry().find(e)
    }

    #[test]
    fn magic_numbers() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....").unwrap().0, "png");
        assert_eq!(sniff(b"\xFF\xD8\xFF\xE0....JFIF").unwrap().0, "jpg");
        assert_eq!(sniff(b"%PDF-1.7\n").unwrap().0, "pdf");
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ").unwrap().0, "webp");
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt ").unwrap().0, "wav");
    }

    #[test]
    fn windows_metafiles() {
        let mut emf = vec![1, 0, 0, 0, 108, 0, 0, 0];
        emf.resize(40, 0);
        emf.extend(b" EMF\0\0\x01\0");
        assert_eq!(sniff(&emf).unwrap().0, "emf");
        assert_eq!(sniff(b"\xD7\xCD\xC6\x9A\0\0\0\0").unwrap().0, "wmf");
        assert_eq!(sniff(b"\x01\x00\x09\x00\x00\x03\x22\x00").unwrap().0, "wmf");
    }

    #[test]
    fn iso_bmff_brands() {
        let heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
        assert_eq!(sniff(heic).unwrap().0, "heic");
        let avif = b"\0\0\0\x1cftypavif\0\0\0\0avifmif1miaf";
        assert_eq!(sniff(avif).unwrap().0, "avif");
        let mif1_avif = b"\0\0\0\x1cftypmif1\0\0\0\0mif1avifmiaf";
        assert_eq!(sniff(mif1_avif).unwrap().0, "avif");
        let mov = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";
        assert_eq!(sniff(mov).unwrap().0, "mov");
        let mp4 = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2";
        assert_eq!(sniff(mp4).unwrap().0, "mp4");
    }

    #[test]
    fn jpg_that_is_really_heic() {
        let heic = b"\0\0\0\x18ftypheic\0\0\0\0mif1heic";
        let d = detect_bytes(heic, ext("jpg")).unwrap();
        assert_eq!(registry().key(d.format), "heic");
        assert!(d.mismatch);
    }

    #[test]
    fn zip_family_keeps_specific_extension() {
        let mut z = b"PK\x03\x04".to_vec();
        z.resize(64, 0);
        assert_eq!(key(detect_bytes(&z, ext("key"))), "key");
        assert_eq!(key(detect_bytes(&z, None)), "zip");
        let mut epub = b"PK\x03\x04".to_vec();
        epub.resize(30, 0);
        epub.extend_from_slice(b"mimetypeapplication/epub+zip");
        assert_eq!(key(detect_bytes(&epub, None)), "epub");
    }

    #[test]
    fn text_formats() {
        assert_eq!(
            key(detect_bytes(
                b"1\n00:00:01,000 --> 00:00:02,000\nHi\n",
                None
            )),
            "srt"
        );
        assert_eq!(key(detect_bytes(b"WEBVTT\n\n", ext("txt"))), "vtt");
        assert_eq!(key(detect_bytes(b"a,b\n1,2\n", ext("csv"))), "csv");
        assert_eq!(key(detect_bytes(b"name = 'x'\n", ext("toml"))), "toml");
        assert_eq!(key(detect_bytes(b"{\"a\":1}\n{\"a\":2}\n", None)), "ndjson");
        assert_eq!(
            key(detect_bytes(
                b"<svg xmlns='http://www.w3.org/2000/svg'/>",
                None
            )),
            "svg"
        );
        assert!(!detect_bytes(b"# Title\n", ext("md")).unwrap().mismatch);
    }

    #[test]
    fn tar_and_compressed_tar() {
        let mut tar = vec![0u8; 600];
        tar[257..262].copy_from_slice(b"ustar");
        assert_eq!(key(detect_bytes(&tar, None)), "tar");
        assert_eq!(key(detect_bytes(b"\x1F\x8B\x08\0", ext("tgz"))), "tar.gz");
    }

    proptest::proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(proptest::num::u8::ANY, 0..600)) {
            let _ = detect_bytes(&bytes, ext("jpg"));
            let _ = detect_bytes(&bytes, None);
        }
    }
}
