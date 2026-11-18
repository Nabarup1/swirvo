//! Fonts (docs/12 §8): TTF/OTF ⇄ WOFF ⇄ WOFF2, all in our code.
//!
//! - **WOFF** (W3C 2012): each table zlib-compressed separately.
//! - **WOFF2** (W3C 2018): one Brotli stream. Decoding reconstructs the transformed `glyf`
//!   and `loca` tables (§5) and the transformed `hmtx` (§5.4). Encoding uses the null
//!   transform (allowed by the spec), so outlines and hinting come back byte-identical.
//! - Font collections (`.ttc`, WOFF2 collections) are not supported.

use std::io::{Read, Write};

use swirvo_core::{Capability, Engine, EngineError, StepRequest};

/// Largest decompressed font we accept.
const MAX_FONT: usize = 128 << 20;

fn bad(what: &str) -> EngineError {
    EngineError::Decode(format!("this font is damaged ({what})"))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub tag: [u8; 4],
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sfnt {
    pub flavor: u32,
    pub tables: Vec<Table>,
}

impl Sfnt {
    pub fn get(&self, tag: &[u8; 4]) -> Option<&Table> {
        self.tables.iter().find(|t| &t.tag == tag)
    }
    /// True for CFF-flavored OpenType ("OTTO").
    pub fn is_cff(&self) -> bool {
        self.flavor == u32::from_be_bytes(*b"OTTO")
    }
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, i: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], EngineError> {
        let e = self
            .i
            .checked_add(n)
            .filter(|&e| e <= self.b.len())
            .ok_or_else(|| bad("truncated"))?;
        let s = &self.b[self.i..e];
        self.i = e;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, EngineError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, EngineError> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn i16(&mut self) -> Result<i16, EngineError> {
        Ok(self.u16()? as i16)
    }
    fn u32(&mut self) -> Result<u32, EngineError> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn tag(&mut self) -> Result<[u8; 4], EngineError> {
        let s = self.take(4)?;
        Ok([s[0], s[1], s[2], s[3]])
    }
    /// WOFF2 UIntBase128: up to 5 bytes, 7 bits each, no leading zeros, fits in u32.
    fn base128(&mut self) -> Result<u32, EngineError> {
        let mut v: u32 = 0;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 0 && b == 0x80 {
                return Err(bad("UIntBase128 leading zero"));
            }
            if v & 0xFE00_0000 != 0 {
                return Err(bad("UIntBase128 overflow"));
            }
            v = (v << 7) | (b & 0x7F) as u32;
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(bad("UIntBase128 too long"))
    }
    /// WOFF2 255UInt16.
    fn u255(&mut self) -> Result<u16, EngineError> {
        let code = self.u8()?;
        Ok(match code {
            253 => self.u16()?,
            255 => self.u8()? as u16 + 253,
            254 => self.u8()? as u16 + 506,
            c => c as u16,
        })
    }
}

fn pad4(n: usize) -> usize {
    (n + 3) & !3
}

fn checksum(d: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let mut chunks = d.chunks(4);
    for c in &mut chunks {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        sum = sum.wrapping_add(u32::from_be_bytes(w));
    }
    sum
}

pub fn read_sfnt(b: &[u8]) -> Result<Sfnt, EngineError> {
    let mut r = Rd::new(b);
    let flavor = r.u32()?;
    match &flavor.to_be_bytes() {
        b"ttcf" => {
            return Err(EngineError::Invalid(
                "font collections (.ttc) aren't supported yet".into(),
            ));
        }
        [0, 1, 0, 0] | b"OTTO" | b"true" => {}
        _ => return Err(EngineError::Decode("not a TrueType/OpenType font".into())),
    }
    let n = r.u16()? as usize;
    r.take(6)?;
    let mut tables = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = r.tag()?;
        let _sum = r.u32()?;
        let off = r.u32()? as usize;
        let len = r.u32()? as usize;
        let data = b
            .get(off..off.checked_add(len).ok_or_else(|| bad("table bounds"))?)
            .ok_or_else(|| bad("table outside the file"))?;
        tables.push(Table {
            tag,
            data: data.to_vec(),
        });
    }
    Ok(Sfnt { flavor, tables })
}

/// Serialize as a TrueType/OpenType file: tables sorted by tag, 4-byte aligned, table
/// checksums and head.checkSumAdjustment recomputed.
pub fn write_sfnt(s: &Sfnt) -> Vec<u8> {
    let mut tables: Vec<&Table> = s.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    let n = tables.len();
    let mut out = Vec::new();
    out.extend_from_slice(&s.flavor.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    let es = if n == 0 {
        0
    } else {
        usize::BITS - 1 - n.leading_zeros()
    };
    let sr = 16u16 << es;
    out.extend_from_slice(&sr.to_be_bytes());
    out.extend_from_slice(&(es as u16).to_be_bytes());
    out.extend_from_slice(&((n as u16) * 16 - sr).to_be_bytes());
    let mut off = 12 + 16 * n;
    let mut head_at = None;
    let mut body = Vec::new();
    for t in &tables {
        let mut data = t.data.clone();
        if &t.tag == b"head" && data.len() >= 12 {
            data[8..12].fill(0);
            head_at = Some(off);
        }
        out.extend_from_slice(&t.tag);
        out.extend_from_slice(&checksum(&data).to_be_bytes());
        out.extend_from_slice(&(off as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        off += pad4(data.len());
        body.extend_from_slice(&data);
        body.resize(pad4(body.len()), 0);
    }
    out.extend_from_slice(&body);
    if let Some(h) = head_at {
        let adj = 0xB1B0_AFBAu32.wrapping_sub(checksum(&out));
        out[h + 8..h + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

// ---------- WOFF 1 ----------

pub fn read_woff(b: &[u8]) -> Result<Sfnt, EngineError> {
    let mut r = Rd::new(b);
    if &r.tag()? != b"wOFF" {
        return Err(EngineError::Decode("not a WOFF font".into()));
    }
    let flavor = r.u32()?;
    let _len = r.u32()?;
    let n = r.u16()? as usize;
    r.take(2 + 4 + 4 + 20)?;
    let mut tables = Vec::with_capacity(n);
    let mut total = 0usize;
    for _ in 0..n {
        let tag = r.tag()?;
        let off = r.u32()? as usize;
        let comp = r.u32()? as usize;
        let orig = r.u32()? as usize;
        let _sum = r.u32()?;
        total = total.saturating_add(orig);
        if total > MAX_FONT {
            return Err(EngineError::Invalid("font too large".into()));
        }
        let src = b
            .get(off..off.checked_add(comp).ok_or_else(|| bad("table bounds"))?)
            .ok_or_else(|| bad("table outside the file"))?;
        let data = if comp < orig {
            let mut d = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(src)
                .take(orig as u64)
                .read_to_end(&mut d)
                .map_err(|_| bad("zlib"))?;
            if d.len() != orig {
                return Err(bad("table size"));
            }
            d
        } else if comp == orig {
            src.to_vec()
        } else {
            return Err(bad("table size"));
        };
        tables.push(Table { tag, data });
    }
    Ok(Sfnt { flavor, tables })
}

pub fn write_woff(s: &Sfnt) -> Result<Vec<u8>, EngineError> {
    let mut tables: Vec<&Table> = s.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    let sfnt = write_sfnt(s);
    let n = tables.len();
    let mut dir = Vec::new();
    let mut body = Vec::new();
    let base = 44 + 20 * n;
    for t in &tables {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(&t.data)?;
        let packed = z.finish()?;
        let data = if packed.len() < t.data.len() {
            packed
        } else {
            t.data.clone()
        };
        // The checksum the sfnt directory has (head with its adjustment zeroed).
        let mut sumdata = t.data.clone();
        if &t.tag == b"head" && sumdata.len() >= 12 {
            sumdata[8..12].fill(0);
        }
        dir.extend_from_slice(&t.tag);
        dir.extend_from_slice(&((base + body.len()) as u32).to_be_bytes());
        dir.extend_from_slice(&(data.len() as u32).to_be_bytes());
        dir.extend_from_slice(&(t.data.len() as u32).to_be_bytes());
        dir.extend_from_slice(&checksum(&sumdata).to_be_bytes());
        body.extend_from_slice(&data);
        body.resize(pad4(body.len()), 0);
    }
    let total = base + body.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"wOFF");
    out.extend_from_slice(&s.flavor.to_be_bytes());
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt.len() as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&body);
    Ok(out)
}

// ---------- WOFF 2 ----------

const KNOWN_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

struct Entry {
    tag: [u8; 4],
    version: u8,
    orig: usize,
    /// Bytes in the decompressed stream.
    stored: usize,
}

impl Entry {
    fn transformed(&self) -> bool {
        match &self.tag {
            b"glyf" | b"loca" => self.version != 3,
            _ => self.version != 0,
        }
    }
}

/// Simple glyph point.
struct Pt {
    x: i32,
    y: i32,
    on: bool,
}

/// Decode a WOFF2 triplet for `flag` (low 7 bits) from `g`.
fn triplet(flag: u8, g: &mut Rd<'_>) -> Result<(i32, i32), EngineError> {
    let sign = |f: u8, v: i32| if f & 1 != 0 { v } else { -v };
    let f = flag & 0x7F;
    Ok(if f < 10 {
        (0, sign(f, (((f & 14) as i32) << 7) + g.u8()? as i32))
    } else if f < 20 {
        (sign(f, ((((f - 10) & 14) as i32) << 7) + g.u8()? as i32), 0)
    } else if f < 84 {
        let b0 = (f - 20) as i32;
        let b1 = g.u8()? as i32;
        (
            sign(f, 1 + (b0 & 0x30) + (b1 >> 4)),
            sign(f >> 1, 1 + ((b0 & 0x0C) << 2) + (b1 & 0x0F)),
        )
    } else if f < 120 {
        let b0 = (f - 84) as i32;
        let (a, b) = (g.u8()? as i32, g.u8()? as i32);
        (
            sign(f, 1 + ((b0 / 12) << 8) + a),
            sign(f >> 1, 1 + (((b0 % 12) >> 2) << 8) + b),
        )
    } else if f < 124 {
        let (a, b, c) = (g.u8()? as i32, g.u8()? as i32, g.u8()? as i32);
        (
            sign(f, (a << 4) + (b >> 4)),
            sign(f >> 1, ((b & 0x0F) << 8) + c),
        )
    } else {
        let (a, b, c, d) = (
            g.u8()? as i32,
            g.u8()? as i32,
            g.u8()? as i32,
            g.u8()? as i32,
        );
        (sign(f, (a << 8) + b), sign(f >> 1, (c << 8) + d))
    })
}

/// Encode a simple glyph in the standard `glyf` format.
fn encode_simple(
    out: &mut Vec<u8>,
    ends: &[u16],
    pts: &[Pt],
    instr: &[u8],
    bbox: [i16; 4],
    overlap: bool,
) {
    out.extend_from_slice(&(ends.len() as i16).to_be_bytes());
    for v in bbox {
        out.extend_from_slice(&v.to_be_bytes());
    }
    for e in ends {
        out.extend_from_slice(&e.to_be_bytes());
    }
    out.extend_from_slice(&(instr.len() as u16).to_be_bytes());
    out.extend_from_slice(instr);
    let mut flags = Vec::with_capacity(pts.len());
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    let (mut px, mut py) = (0i32, 0i32);
    for (i, p) in pts.iter().enumerate() {
        let (dx, dy) = (p.x - px, p.y - py);
        (px, py) = (p.x, p.y);
        let mut f = u8::from(p.on);
        if i == 0 && overlap {
            f |= 0x40;
        }
        if dx == 0 {
            f |= 0x10;
        } else if dx.abs() < 256 {
            f |= 0x02 | if dx > 0 { 0x10 } else { 0 };
            xs.push(dx.unsigned_abs() as u8);
        } else {
            xs.extend_from_slice(&(dx as i16).to_be_bytes());
        }
        if dy == 0 {
            f |= 0x20;
        } else if dy.abs() < 256 {
            f |= 0x04 | if dy > 0 { 0x20 } else { 0 };
            ys.push(dy.unsigned_abs() as u8);
        } else {
            ys.extend_from_slice(&(dy as i16).to_be_bytes());
        }
        flags.push(f);
    }
    // Run-length the flags (REPEAT_FLAG).
    let mut i = 0;
    while i < flags.len() {
        let f = flags[i];
        let mut run = 1;
        while i + run < flags.len() && flags[i + run] == f && run < 256 {
            run += 1;
        }
        if run > 2 {
            out.push(f | 0x08);
            out.push((run - 1) as u8);
        } else {
            out.extend(std::iter::repeat_n(f, run));
        }
        i += run;
    }
    out.extend_from_slice(&xs);
    out.extend_from_slice(&ys);
}

/// Rebuilt `glyf` + `loca`, whether `loca` is the long format, and each glyph's xMin.
type Untransformed = (Vec<u8>, Vec<u8>, bool, Vec<i16>);

/// Rebuild `glyf` and `loca` from the WOFF2 transformed glyf table (spec §5.1–5.3).
fn untransform_glyf(t: &[u8]) -> Result<Untransformed, EngineError> {
    let mut h = Rd::new(t);
    h.u16()?;
    let options = h.u16()?;
    let num_glyphs = h.u16()? as usize;
    let index_format = h.u16()?;
    let mut sizes = [0usize; 7];
    for s in &mut sizes {
        *s = h.u32()? as usize;
    }
    let mut take = |n: usize| -> Result<Rd<'_>, EngineError> { Ok(Rd::new(h.take(n)?)) };
    let mut ncont = take(sizes[0])?;
    let mut npts = take(sizes[1])?;
    let mut flags = take(sizes[2])?;
    let mut glyphs = take(sizes[3])?;
    let mut comps = take(sizes[4])?;
    let bbox_all = take(sizes[5])?;
    let mut instrs = take(sizes[6])?;
    let overlap_map = if options & 1 != 0 {
        Some(take(num_glyphs.div_ceil(8))?.b)
    } else {
        None
    };
    let bitmap_len = ((num_glyphs + 31) >> 5) << 2;
    let bbox_bits = bbox_all
        .b
        .get(..bitmap_len)
        .ok_or_else(|| bad("bbox bitmap"))?;
    let mut bbox = Rd::new(&bbox_all.b[bitmap_len..]);
    let has_bbox = |i: usize| bbox_bits[i >> 3] & (0x80 >> (i & 7)) != 0;

    let mut glyf = Vec::new();
    let mut offsets = Vec::with_capacity(num_glyphs + 1);
    let mut xmins = vec![0i16; num_glyphs];
    for (gi, xmin) in xmins.iter_mut().enumerate() {
        offsets.push(glyf.len());
        let nc = ncont.i16()?;
        if nc == 0 {
            if has_bbox(gi) {
                return Err(bad("bbox on an empty glyph"));
            }
            continue;
        }
        if nc > 0 {
            let mut ends = Vec::with_capacity(nc as usize);
            let mut total: usize = 0;
            for _ in 0..nc {
                total += npts.u255()? as usize;
                if total == 0 || total > 0xFFFF {
                    return Err(bad("point count"));
                }
                ends.push((total - 1) as u16);
            }
            let mut pts = Vec::with_capacity(total);
            let (mut x, mut y) = (0i32, 0i32);
            for _ in 0..total {
                let f = flags.u8()?;
                let (dx, dy) = triplet(f, &mut glyphs)?;
                x += dx;
                y += dy;
                pts.push(Pt {
                    x,
                    y,
                    on: f & 0x80 == 0,
                });
            }
            let ilen = glyphs.u255()? as usize;
            let instr = instrs.take(ilen)?;
            let bb = if has_bbox(gi) {
                [bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?]
            } else {
                let mut b = [i16::MAX, i16::MAX, i16::MIN, i16::MIN];
                for p in &pts {
                    b[0] = b[0].min(p.x as i16);
                    b[1] = b[1].min(p.y as i16);
                    b[2] = b[2].max(p.x as i16);
                    b[3] = b[3].max(p.y as i16);
                }
                b
            };
            *xmin = bb[0];
            let overlap = overlap_map.is_some_and(|m| m[gi >> 3] & (0x80 >> (gi & 7)) != 0);
            encode_simple(&mut glyf, &ends, &pts, instr, bb, overlap);
        } else {
            if !has_bbox(gi) {
                return Err(bad("composite glyph without bbox"));
            }
            let bb = [bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?];
            *xmin = bb[0];
            glyf.extend_from_slice(&(-1i16).to_be_bytes());
            for v in bb {
                glyf.extend_from_slice(&v.to_be_bytes());
            }
            let mut have_instr = false;
            loop {
                let f = comps.u16()?;
                let idx = comps.u16()?;
                let args = if f & 0x0001 != 0 { 4 } else { 2 };
                let xf = if f & 0x0008 != 0 {
                    2
                } else if f & 0x0040 != 0 {
                    4
                } else if f & 0x0080 != 0 {
                    8
                } else {
                    0
                };
                glyf.extend_from_slice(&f.to_be_bytes());
                glyf.extend_from_slice(&idx.to_be_bytes());
                glyf.extend_from_slice(comps.take(args + xf)?);
                have_instr |= f & 0x0100 != 0;
                if f & 0x0020 == 0 {
                    break;
                }
            }
            if have_instr {
                let ilen = glyphs.u255()? as usize;
                glyf.extend_from_slice(&(ilen as u16).to_be_bytes());
                glyf.extend_from_slice(instrs.take(ilen)?);
            }
        }
        glyf.resize(pad4(glyf.len()), 0);
        if glyf.len() > MAX_FONT {
            return Err(EngineError::Invalid("font too large".into()));
        }
    }
    offsets.push(glyf.len());
    let long = index_format != 0 || glyf.len() / 2 > 0xFFFF;
    let mut loca = Vec::with_capacity(offsets.len() * 4);
    for o in offsets {
        if long {
            loca.extend_from_slice(&(o as u32).to_be_bytes());
        } else {
            loca.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    }
    Ok((glyf, loca, long, xmins))
}

/// Rebuild `hmtx` from its WOFF2 transform (spec §5.4).
fn untransform_hmtx(
    t: &[u8],
    num_glyphs: usize,
    num_hmetrics: usize,
    xmins: &[i16],
) -> Result<Vec<u8>, EngineError> {
    if num_hmetrics == 0 || num_hmetrics > num_glyphs {
        return Err(bad("hhea"));
    }
    let mut r = Rd::new(t);
    let flags = r.u8()?;
    let mut adv = Vec::with_capacity(num_hmetrics);
    for _ in 0..num_hmetrics {
        adv.push(r.u16()?);
    }
    let mut lsb = Vec::with_capacity(num_glyphs);
    for i in 0..num_hmetrics {
        lsb.push(if flags & 1 == 0 {
            r.i16()?
        } else {
            xmins.get(i).copied().unwrap_or(0)
        });
    }
    for i in num_hmetrics..num_glyphs {
        lsb.push(if flags & 2 == 0 {
            r.i16()?
        } else {
            xmins.get(i).copied().unwrap_or(0)
        });
    }
    let mut out = Vec::with_capacity(num_hmetrics * 4 + (num_glyphs - num_hmetrics) * 2);
    for i in 0..num_glyphs {
        if i < num_hmetrics {
            out.extend_from_slice(&adv[i].to_be_bytes());
        }
        out.extend_from_slice(&lsb[i].to_be_bytes());
    }
    Ok(out)
}

pub fn read_woff2(b: &[u8]) -> Result<Sfnt, EngineError> {
    let mut r = Rd::new(b);
    if &r.tag()? != b"wOF2" {
        return Err(EngineError::Decode("not a WOFF2 font".into()));
    }
    let flavor = r.u32()?;
    if &flavor.to_be_bytes() == b"ttcf" {
        return Err(EngineError::Invalid(
            "WOFF2 font collections aren't supported yet".into(),
        ));
    }
    let _len = r.u32()?;
    let n = r.u16()? as usize;
    r.u16()?;
    let _total_sfnt = r.u32()?;
    let compressed = r.u32()? as usize;
    r.take(4 + 20)?;
    let mut entries = Vec::with_capacity(n);
    for _ in 0..n {
        let flags = r.u8()?;
        let tag = match flags & 0x3F {
            63 => r.tag()?,
            i => **KNOWN_TAGS.get(i as usize).ok_or_else(|| bad("tag index"))?,
        };
        let version = flags >> 6;
        let orig = r.base128()? as usize;
        let mut e = Entry {
            tag,
            version,
            orig,
            stored: orig,
        };
        if e.transformed() {
            e.stored = r.base128()? as usize;
        }
        entries.push(e);
    }
    let src = r.take(compressed)?;
    let mut data = Vec::new();
    brotli::Decompressor::new(src, 4096)
        .take(MAX_FONT as u64 + 1)
        .read_to_end(&mut data)
        .map_err(|_| bad("brotli"))?;
    if data.len() > MAX_FONT {
        return Err(EngineError::Invalid("font too large".into()));
    }
    let mut stream = Rd::new(&data);
    let mut raw: Vec<(&Entry, &[u8])> = Vec::with_capacity(n);
    for e in &entries {
        raw.push((e, stream.take(e.stored)?));
    }
    let find = |tag: &[u8; 4]| raw.iter().find(|(e, _)| &e.tag == tag);
    let mut tables = Vec::with_capacity(n);
    let mut xmins: Option<Vec<i16>> = None;
    let mut long_loca = None;
    if let Some((e, t)) = find(b"glyf")
        && e.transformed()
    {
        let (glyf, loca, long, xm) = untransform_glyf(t)?;
        tables.push(Table {
            tag: *b"glyf",
            data: glyf,
        });
        tables.push(Table {
            tag: *b"loca",
            data: loca,
        });
        xmins = Some(xm);
        long_loca = Some(long);
    }
    for (e, t) in &raw {
        let done = tables.iter().any(|x| x.tag == e.tag);
        if done {
            continue;
        }
        if &e.tag == b"hmtx" && e.transformed() {
            let maxp = find(b"maxp").map(|x| x.1).ok_or_else(|| bad("no maxp"))?;
            let hhea = find(b"hhea").map(|x| x.1).ok_or_else(|| bad("no hhea"))?;
            let ng =
                u16::from_be_bytes([*maxp.get(4).ok_or_else(|| bad("maxp"))?, maxp[5]]) as usize;
            let nh =
                u16::from_be_bytes([*hhea.get(34).ok_or_else(|| bad("hhea"))?, hhea[35]]) as usize;
            let xm = xmins
                .as_deref()
                .ok_or_else(|| bad("hmtx transform needs glyf"))?;
            tables.push(Table {
                tag: e.tag,
                data: untransform_hmtx(t, ng, nh, xm)?,
            });
        } else if e.transformed() {
            return Err(bad("unknown table transform"));
        } else {
            let mut d = t.to_vec();
            if &e.tag == b"head"
                && let Some(long) = long_loca
                && d.len() >= 52
            {
                d[50..52].copy_from_slice(&(i16::from(long)).to_be_bytes());
            }
            if d.len() != e.orig {
                return Err(bad("table length"));
            }
            tables.push(Table {
                tag: e.tag,
                data: d,
            });
        }
    }
    Ok(Sfnt { flavor, tables })
}

/// WOFF2 with Brotli at `quality` (0–11; 11 is the smallest and slowest).
pub fn write_woff2(s: &Sfnt) -> Result<Vec<u8>, EngineError> {
    write_woff2_q(s, 11)
}

pub fn write_woff2_q(s: &Sfnt, quality: u32) -> Result<Vec<u8>, EngineError> {
    let mut tables: Vec<&Table> = s.tables.iter().collect();
    tables.sort_by_key(|t| t.tag);
    let n = tables.len();
    let mut dir = Vec::new();
    let mut stream = Vec::new();
    for t in &tables {
        let idx = KNOWN_TAGS.iter().position(|k| **k == t.tag);
        // Null transform: version 3 for glyf/loca, 0 for everything else.
        let version: u8 = if matches!(&t.tag, b"glyf" | b"loca") {
            3
        } else {
            0
        };
        match idx {
            Some(i) => dir.push((i as u8) | (version << 6)),
            None => {
                dir.push(63 | (version << 6));
                dir.extend_from_slice(&t.tag);
            }
        }
        // UIntBase128 of the length.
        let mut v = t.data.len() as u32;
        let mut bytes = vec![(v & 0x7F) as u8];
        v >>= 7;
        while v > 0 {
            bytes.push((v & 0x7F) as u8 | 0x80);
            v >>= 7;
        }
        bytes.reverse();
        dir.extend_from_slice(&bytes);
        stream.extend_from_slice(&t.data);
    }
    let mut packed = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut packed, 4096, quality.min(11), 22);
        w.write_all(&stream)?;
        w.flush()?;
    }
    let sfnt_size = 12 + 16 * n + tables.iter().map(|t| pad4(t.data.len())).sum::<usize>();
    let header = 48;
    let total = pad4(header + dir.len() + packed.len());
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"wOF2");
    out.extend_from_slice(&s.flavor.to_be_bytes());
    out.extend_from_slice(&(total as u32).to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(sfnt_size as u32).to_be_bytes());
    out.extend_from_slice(&(packed.len() as u32).to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&[0; 20]);
    out.extend_from_slice(&dir);
    out.extend_from_slice(&packed);
    out.resize(total, 0);
    Ok(out)
}

/// Read any supported font container.
pub fn read_any(b: &[u8]) -> Result<Sfnt, EngineError> {
    match b.get(..4) {
        Some(b"wOFF") => read_woff(b),
        Some(b"wOF2") => read_woff2(b),
        _ => read_sfnt(b),
    }
}

pub struct FontEngine;

impl Engine for FontEngine {
    fn id(&self) -> &'static str {
        "font"
    }

    fn capabilities(&self) -> Vec<Capability> {
        let mut v = Vec::new();
        for (from, tos) in [
            ("ttf", &["woff", "woff2"][..]),
            ("otf", &["woff", "woff2"]),
            ("woff", &["ttf", "otf", "woff2"]),
            ("woff2", &["ttf", "otf", "woff"]),
        ] {
            for to in tos {
                v.push(
                    Capability::new("font", from, to)
                        .loss(0)
                        .time(20, 200)
                        .note("same outlines and hinting, repackaged"),
                );
            }
        }
        v
    }

    fn run(&self, req: &StepRequest<'_>) -> Result<(), EngineError> {
        let bytes = std::fs::read(req.input)?;
        if bytes.len() > MAX_FONT {
            return Err(EngineError::Invalid("font too large".into()));
        }
        let s = read_any(&bytes)?;
        req.cancel.check()?;
        let out = match req.to {
            "woff" => write_woff(&s)?,
            // Balanced trades ~2 % of size for a 5–10× faster compressor.
            "woff2" => write_woff2_q(
                &s,
                if req.preset == swirvo_core::Preset::Balanced {
                    9
                } else {
                    11
                },
            )?,
            _ => write_sfnt(&s),
        };
        std::fs::write(req.output, out)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Option<Vec<u8>> {
        [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/mnt/c/Windows/Fonts/arial.ttf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok())
    }

    fn same_tables(a: &Sfnt, b: &Sfnt) {
        let mut x: Vec<_> = a.tables.iter().map(|t| (t.tag, t.data.len())).collect();
        let mut y: Vec<_> = b.tables.iter().map(|t| (t.tag, t.data.len())).collect();
        x.sort();
        y.sort();
        assert_eq!(x, y);
        for t in &a.tables {
            let u = b.get(&t.tag).unwrap();
            if &t.tag == b"head" {
                assert_eq!(t.data[..8], u.data[..8]);
                assert_eq!(t.data[12..], u.data[12..]);
            } else {
                assert!(t.data == u.data, "{}", String::from_utf8_lossy(&t.tag));
            }
        }
    }

    #[test]
    fn round_trips_woff_and_woff2() {
        let Some(ttf) = sample() else { return };
        let s = read_sfnt(&ttf).unwrap();
        let w1 = write_woff(&s).unwrap();
        assert!(w1.len() < ttf.len());
        same_tables(&s, &read_woff(&w1).unwrap());
        let w2 = write_woff2(&s).unwrap();
        assert!(w2.len() < w1.len(), "{} vs {}", w2.len(), w1.len());
        let back = read_woff2(&w2).unwrap();
        same_tables(&s, &back);
        let out = write_sfnt(&back);
        // The result is a valid font with the same glyphs and a correct whole-file checksum.
        let f = ab_glyph::FontRef::try_from_slice(&out).unwrap();
        use ab_glyph::Font;
        assert!(f.glyph_count() > 100);
        assert_eq!(checksum(&out), 0xB1B0_AFBA);
    }

    #[test]
    fn base128_and_255uint16() {
        assert_eq!(Rd::new(&[0x3F]).base128().unwrap(), 63);
        assert_eq!(Rd::new(&[0x81, 0x00]).base128().unwrap(), 128);
        assert!(Rd::new(&[0x80, 0x01]).base128().is_err());
        assert!(Rd::new(&[0xFF, 0xFF, 0xFF, 0xFF, 0x7F]).base128().is_err());
        assert_eq!(Rd::new(&[252]).u255().unwrap(), 252);
        assert_eq!(Rd::new(&[255, 0]).u255().unwrap(), 253);
        assert_eq!(Rd::new(&[254, 0]).u255().unwrap(), 506);
        assert_eq!(Rd::new(&[253, 0x12, 0x34]).u255().unwrap(), 0x1234);
    }

    #[test]
    fn triplets_follow_the_spec_table() {
        // flag 0: dy = −(0 + b), flag 1: +b; flag 10/11: dx; 84…: two bytes.
        assert_eq!(triplet(0, &mut Rd::new(&[5])).unwrap(), (0, -5));
        assert_eq!(triplet(1, &mut Rd::new(&[5])).unwrap(), (0, 5));
        assert_eq!(triplet(11, &mut Rd::new(&[7])).unwrap(), (7, 0));
        assert_eq!(triplet(20, &mut Rd::new(&[0x00])).unwrap(), (-1, -1));
        assert_eq!(triplet(23, &mut Rd::new(&[0x21])).unwrap(), (3, 2));
        assert_eq!(
            triplet(124, &mut Rd::new(&[1, 0, 0, 2])).unwrap(),
            (-256, -2)
        );
        assert_eq!(triplet(127, &mut Rd::new(&[1, 0, 0, 2])).unwrap(), (256, 2));
    }
}
