# 05 · Conversion engines and techniques

## 1. Hub-and-spoke intermediate representations

Writing a converter for every pair means N × M converters. Instead, each category has an **intermediate representation (IR)**. Decoders produce it and encoders consume it, so the work is N + M. Compilers (LLVM IR), Pandoc (its AST) and FFmpeg (AVFrame) all use the same trick.

| Category | IR | Decoders → IR → Encoders |
|---|---|---|
| Images | `Raster` (u8/u16/f32, linear or sRGB, alpha, ICC profile, orientation, frames) | libvips loaders, libheif, LibRaw, resvg → libvips savers, jpegli, libwebp, libavif, libjxl, oxipng |
| Audio/video | FFmpeg `AVFrame`/`AVPacket` streams | Demuxers and decoders → filter graph → encoders and muxers |
| Structured documents | Pandoc AST (or our `DocIR` for PDF reconstruction) | MD, HTML, DOCX, ODT, EPUB, RTF readers → writers |
| Office layout fidelity | LibreOffice's internal model | Office/ODF/iWork import → PDF and Office export |
| Tables and data | Apache Arrow `RecordBatch` stream | CSV, XLSX, ODS, JSON, Parquet readers → writers |
| Archives | Entry stream (header + byte stream) | libarchive readers → zip/7z/tar writers |
| Subtitles | `SubtitleIR` (cues, styled spans, regions) | SRT, VTT, ASS, TTML, SBV parsers → writers |

## 2. Fast paths: lossless, no re-encode

The planner chooses these automatically, because their edges have zero quality loss and tiny time cost.

| From → To | Technique | Speed vs. re-encoding | Quality |
|---|---|---|---|
| MOV/MKV (H.264 or HEVC + AAC) → MP4 | Remux (`-c copy`, `+faststart`) | 50–200× | Identical |
| MP4 → MKV; WebM (VP9/AV1 + Opus) → MKV | Remux | 50–200× | Identical |
| Video (AAC track) → M4A | Stream copy of the audio track | ~100× | Identical |
| JPEG → JPEG XL | Lossless JPEG recompression; reversible, ~20 % smaller | — | Identical |
| JPEG rotate, flip, crop | DCT-domain transform (libjpeg-turbo `transupp`), crop at MCU boundaries | ~10× | Identical |
| PNG → WebP / JXL lossless | Lossless encode | — | Identical |
| PDF merge, split, reorder, rotate | Object-graph copy (qpdf / lopdf) | — | Identical |
| WAV/AIFF → FLAC/ALAC | Lossless | — | Identical |
| ZIP ⇄ TAR.ZST etc. | Entries streamed between formats, no temporary extraction | ~2× | Identical |
| Video trim | Smart cut ([04](04-algorithms.md) §18) | 10–100× | Identical except the two edge GOPs |
| GIF → MP4/WebM | Re-encode | — | Better, and 5–10× smaller |

## 3. Images

**Pipeline:** sniff → decode (tiles in parallel for HEIC/AVIF/JXL/TIFF) → apply EXIF orientation → color management (lcms2: convert to sRGB for web formats, keep the ICC profile for TIFF/PNG/JXL/AVIF) → alpha policy (flatten onto white or a chosen color for JPEG, with a ⚠ in the wheel) → resize (Lanczos3 in linear light, with an optional sharpen afterwards) → encode → optimize → verify.

**Content-aware defaults** (class from [04](04-algorithms.md) §10):

| Target | Photo | Graphic / screenshot |
|---|---|---|
| JPG | jpegli, q 85, 4:2:0, progressive | q 92, **4:4:4** (sharp text) |
| WebP | lossy q 80, method 4 | **lossless** (often smaller than lossy for UIs) |
| AVIF | libaom/SVT, cq ≈ 26, speed 6, 10-bit when the source is HDR | lossless or cq 18, 4:4:4 |
| JXL | distance 1.0, effort 7 | lossless (modular) |
| PNG | — | oxipng level 2, strip safe chunks |
| HEIC | libheif + kvazaar (BSD) or the system encoder, q 80 | q 90 |
| GIF | cgif + quantizr (MIT), Floyd-Steinberg or ordered dithering | same |
| ICO | 16, 24, 32, 48, 64, 128, 256 px; PNG-compressed 256 | same |

**HDR:** keep HDR (PQ/HLG, 10–12 bit) when the target supports it (AVIF, HEIC, JXL); otherwise tone-map (BT.2390) to SDR and say so. **Animated:** GIF, APNG and animated WebP keep frames and timing; animated → static uses the first frame and tells the user. **Multi-page:** TIFF and PDF → images produce `name-p01.png…` in a subfolder when there are more than 5 pages. **RAW:** LibRaw with camera white balance and AHD demosaic. *Edit in a RAW editor* is out of scope.

## 4. Audio and video (FFmpeg)

**Graph construction:** probe the streams → choose a route (remux if the codecs fit the target container; otherwise transcode only the streams that need it) → build `-filter_complex` (scale, fps, tonemap, loudnorm…) → choose an encoder by capability probe → run with `-progress pipe:1 -nostats` and parse `out_time_us` for progress.

**Encoder choice (LGPL build plus OS and hardware encoders, so no GPL is needed):**

| Target | Preferred | Fallbacks |
|---|---|---|
| H.264 | NVENC / Quick Sync / AMF (hardware) | **`h264_mf`** (Windows Media Foundation; the codec licence comes with Windows), OpenH264 (Cisco binary), x264 (GPL pack, opt-in) |
| HEVC | NVENC / Quick Sync / AMF | `hevc_mf` (needs the HEVC extension), x265 (GPL pack) |
| AV1 | NVENC/QSV/AMF on newer GPUs (not the GTX 1650 Ti or UHD 630) | SVT-AV1, chunked in parallel ([04](04-algorithms.md) §8) |
| VP9 | libvpx (`-row-mt 1`, `-cpu-used 4`) | — |
| AAC | FFmpeg native AAC (≥ 128 kb/s is good), or `aac_mf` on Windows | — |
| Opus / Vorbis / FLAC / MP3 | libopus / libvorbis / native FLAC / LAME (VBR V2) | — |

**Hardware pipeline:** keep frames on the GPU from decode to encode (`-hwaccel cuda|qsv|d3d11va|vaapi`, with the output format kept on the device, then `scale_cuda`, `scale_qsv` or `scale_vaapi`). If the hardware fails, a circuit breaker falls back to software. Capabilities are **probed by test-encoding one frame per encoder** at first run and after driver changes. Session limits are discovered, never hardcoded.

**Recipes inside the engine:** two-pass `loudnorm` (EBU R128) · `palettegen`/`paletteuse` for GIF (stats_mode=diff, fps 12–15, 480–720 px wide) · subtitles burned in with libass · `vidstab` two-pass stabilization · `zscale` + `tonemap`, or libplacebo, for HDR → SDR · RNNoise (`arnndn`) for denoising · `atempo` plus `setpts` for speed changes.

## 5. Documents

There are four routes; the planner picks by edge cost.

1. **Structural (Pandoc AST):** MD, HTML, EPUB, DOCX, ODT, RTF, FB2, LaTeX output. High fidelity for text structure (headings, lists, tables, footnotes), weaker on page layout.
2. **Layout fidelity (LibreOffice headless):** DOCX, PPTX, XLSX, DOC, PPT, XLS, ODF and Apple iWork → PDF, and Office ⇄ ODF.
   - A **warm pool** of 1–2 `soffice` processes, each with its own `-env:UserInstallation` profile, driven over UNO. Kept warm while document jobs are queued and shut down after 2 minutes idle.
   - **Fonts matter most on Linux:** bundle metric-compatible fonts (Carlito for Calibri, Caladea for Cambria, the Liberation family for Arial/Times/Courier) so layout doesn't shift.
   - Macros disabled; no network; per-document timeout (60 s + 2 s per page); crash → restart that instance.
3. **Platform webview printing:** HTML, Markdown and EPUB → PDF through **WebView2 `PrintToPdfAsync`** on Windows and WebKitGTK print operations on Linux. That is a full browser layout engine with no Chromium to bundle. It runs in a hidden Hub window, with network requests disabled.
4. **PDF reconstruction (our engine, `pdflayout`):** PDF → DOCX rebuilds each page (fonts, columns, tables, pictures, boxes, headers and footers) as described in [13](13-pdf-reconstruction.md); PDF → Markdown, TXT, HTML, ODT and EPUB use the same analysis. XLSX and PPTX are planned. PDF → images renders with hayro (pure Rust) at the chosen DPI, with pixel caps for oversized pages.

## 6. Spreadsheets and data

- **Reading:** `calamine` streams XLSX, XLS and ODS. **Writing:** `rust_xlsxwriter` in constant-memory mode for large sheets.
- **CSV dialect sniffing:** test candidate delimiters (`,` `;` `\t` `|`) and quote characters and pick the one with the most consistent per-row field counts. Encoding is detected with `chardetng`, so legacy Excel CSVs in Windows-1252 import correctly.
- **Type inference for Parquet:** sample rows (the first 10k plus a reservoir sample) and promote along the lattice null < bool < int64 < float64 < string, detecting dates and timestamps against common patterns. The result streams through Arrow and is written with ZSTD compression.
- **JSON ⇄ YAML ⇄ TOML:** `serde`-based, with `toml_edit` when TOML comments should survive. **XML ⇄ JSON** uses `@attr` and `#text` conventions, so round-trips are predictable.

## 7. Archives

libarchive reads ZIP, 7z, RAR4/RAR5, TAR (all compressions), ISO and CAB. Writing uses `zip` (deflate through zlib-rs, multi-threaded across entries), libarchive's 7z writer (LZMA2), and `tar` + `zstd`/`xz2`. Safety ([03](03-system-architecture.md) §14): zip-slip checks, symlink policy, expansion-ratio and entry-count limits. ZIP file names are decoded as CP437 unless the UTF-8 flag is set (with an override for Shift-JIS or GBK archives). Timestamps and Unix permissions are preserved.

## 8. Subtitles, fonts, text and local AI

- **Subtitles:** our own parsers into `SubtitleIR`. Timing can be shifted, scaled, or fixed for 23.976 ↔ 25 fps. ASS styles map to WebVTT cue settings where possible. Embedded tracks are extracted by stream copy.
- **Fonts:** TTF/OTF → WOFF2 (the woff2 library: Brotli plus the glyf/loca transform) and back; WOFF ⇄ TTF. No outline conversion between TrueType and CFF.
- **Text:** `encoding_rs` for streaming transcoding, `chardetng` for detection, CRLF ⇄ LF, BOM add/remove. Binary files are refused.
- **Local AI** runs on ONNX Runtime (`ort` 2.x, ORT 1.31) with execution providers chosen per machine: **Windows ML** where available (Microsoft's ONNX Runtime-based stack, which acquires vendor providers itself; verify its packaging requirements), otherwise DirectML or CUDA on Windows, CUDA, ROCm or OpenVINO on Linux, and CPU everywhere. Each model ships in FP16 and INT8 variants, chosen from free VRAM.

| Feature | Engine / model | License | Size |
|---|---|---|---|
| Speech → text, subtitles, bleep timestamps | whisper.cpp + Whisper (base, small, large-v3-turbo) | MIT / MIT | 142 MB – 1.6 GB |
| OCR | Windows.Media.Ocr (built in) · Tesseract 5 · RapidOCR (PP-OCR ONNX) | OS / Apache-2.0 / Apache-2.0 | 0 / 15–40 MB / ~15 MB |
| Background removal | BiRefNet (lite) | MIT | ~170 MB |
| Upscaling ×2/×4 | Real-ESRGAN (x4plus, anime variants) | BSD-3-Clause | 17–65 MB |
| Face detection for redaction | YuNet | MIT | < 1 MB |

Avoid: RMBG-1.4/2.0 (non-commercial), InsightFace/SCRFD weights (non-commercial), and any CC BY-NC model.

## 9. Engine matrix and licensing

| Engine | Purpose | License | How we ship it |
|---|---|---|---|
| libvips | Image core | LGPL-2.1+ | Dynamic library, core |
| libjpeg-turbo / jpegli | JPEG | BSD-style / BSD-3 | Core |
| libwebp, libavif, dav1d, libaom | WebP, AVIF | BSD | Core |
| SVT-AV1 | AV1 encoding | BSD-3-Clause-Clear + AOM patent licence | Media pack |
| libjxl | JPEG XL | BSD-3 | Core |
| libheif + libde265 + kvazaar | HEIC decode/encode | LGPL-3 / LGPL-3 / BSD-3 | Core (dynamic); prefer the OS codec on Windows (§10) |
| LibRaw | RAW | LGPL-2.1 / CDDL-1.0 | Core |
| resvg | SVG | Apache-2.0 / MIT | Core (Rust) |
| lcms2 | Color management | MIT | Core |
| cgif + **quantizr** | GIF encode, palettes | MIT / MIT | Core (instead of libimagequant, which is GPL-3) |
| oxipng | PNG optimization | MIT | Core (Rust) |
| hayro | PDF parse, render, text and layout extraction | Apache-2.0 / MIT (bundles Foxit fonts and Adobe CMaps, both BSD-3: notices required) | Core (Rust; replaced the planned PDFium) |
| qpdf / lopdf | PDF structure operations | Apache-2.0 / MIT | Core |
| FFmpeg (LGPL configuration) | Audio and video | LGPL-2.1+ | Media pack, separate executables |
| LAME, libopus, libvorbis, libvpx, OpenH264 | Codecs | LGPL / BSD / BSD / BSD / BSD-2 | Inside the Media pack |
| nv-codec-headers, libvpl, AMF headers | Hardware encoding | MIT | Inside the Media pack |
| libass, libvmaf, zimg, libplacebo | Subtitles, quality metrics, scaling, tone mapping | ISC / BSD-2-Patent / WTFPL / LGPL-2.1 | Inside the Media pack |
| LibreOffice | Office layout | MPL-2.0 | Office pack, separate processes |
| Pandoc | Structured documents | **GPL-2.0+** | Docs pack, separate executable (with the source offer) |
| Calibre `ebook-convert` | MOBI/AZW3 | **GPL-3.0** | Optional Ebook pack, separate process |
| libarchive | Archives | BSD-2 | Core |
| Arrow / Parquet, calamine, rust_xlsxwriter | Data | Apache-2.0 / MIT / MIT-Apache | Core (Rust) |
| encoding_rs, chardetng | Text | MIT / Apache-2.0 | Core (Rust) |
| rxing | QR codes and barcodes | Apache-2.0 | Core (Rust) |
| ONNX Runtime, whisper.cpp | AI runtimes | MIT / MIT | AI packs |
| Tauri, React, Fluent System Icons, Inter | UI | MIT-Apache / MIT / MIT / OFL-1.1 | Hub |

**Never use:** anything under AGPL (MuPDF/PyMuPDF, Ghostscript, iText); `fdk-aac` (not redistributable); libimagequant linked in (GPL-3) unless the app itself is GPL; unrar code for *creating* RAR files (its licence forbids it; libarchive reading is fine).

## 10. Patents (get legal review before a commercial launch)

H.264, HEVC, AAC and HEIF/HEVC can carry patent obligations in some jurisdictions. Strategy:

1. Default to royalty-free codecs wherever the user doesn't care (AV1, VP9, Opus, FLAC, WebP, AVIF, JXL).
2. On Windows, use **OS-licensed codecs**: Media Foundation (`h264_mf`, `aac_mf`) and WIC for HEIC decoding when the user has the HEIF/HEVC extensions. Hardware encoders are licensed with the GPU.
3. Use OpenH264 through Cisco's binary distribution as a software H.264 fallback.
4. On Linux, prefer the distribution's FFmpeg and codecs (or the Flatpak `ffmpeg-full` extension), so codec distribution stays with the distributor.
5. Keep GPL encoders (x264/x265) in an opt-in pack downloaded by the user.

## 11. Compliance workflow

- `cargo deny check licenses bans advisories sources` in CI, with an allow-list of licenses and a ban list (AGPL, GPL in linked crates).
- `scripts/third-party-notices.py` writes `THIRD-PARTY-NOTICES.txt` for every crate compiled into the shipped binaries, including licences of assets bundled *inside* crates (cargo-deny can't see those). The Windows build puts it next to the exe; the tray menu opens it.
- SBOM: `cargo cyclonedx` for Rust and `syft` for native libraries, attached to every release.
- LGPL: dynamic linking, relinking instructions, a source offer and the exact build scripts for every LGPL library we ship. GPL tools: the source offer for the exact binary in the pack.
- Native dependencies are reproducible: a vcpkg manifest with a pinned baseline (Windows) and pinned source tarballs with checksums (Linux), built in CI.

## 12. Validating outputs (per category, in the golden corpus)

| Category | Checks |
|---|---|
| Images | Decodes; dimensions; orientation applied; ICC present when expected; SSIMULACRA2 ≥ threshold against the reference |
| Video/audio | `ffprobe` streams, duration ±1 frame, A/V sync; VMAF on samples for presets; loudness within ±0.5 LU of the target |
| PDF | Page count; text extractable when expected; redaction leaves zero matches; renders without errors |
| Documents | PDF page count within ±1 of the reference; no missing-font substitutions logged |
| Data | Row and column counts; schema; round-trip equality where the formats are lossless |
| Archives | Entry list, sizes and CRCs match; no path escapes |
