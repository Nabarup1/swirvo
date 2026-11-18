# 10 · Formats and conversions

Every format Swirvo recognizes, and what each one can become. The wheel only ever shows targets that are actually reachable on this PC, so this table is the ceiling, not a promise that every target appears for every file.

**Status**
- ✅ **Built in.** Works in the standalone app with nothing else installed.
- 🧩 **With an engine pack.** Lights up automatically when the tool is found on this PC: **Media** = FFmpeg (`ffmpeg.exe` on PATH or in `%LOCALAPPDATA%\Swirvo\engines\ffmpeg\`), **Office** = LibreOffice (default install location, or `soffice` on PATH).
- 🪟 **Built into the OS.** Uses an engine Windows (or Linux) already ships, so it adds 0 bytes to Swirvo: **OCR** = Windows.Media.Ocr (`tesseract` on Linux, if installed); **Codecs** = WIC image codecs (HEIF, AV1 and Raw Image Extensions, common on Windows 11); **tar** = `C:\Windows\System32\tar.exe` (libarchive; `bsdtar` on Linux). See docs/12 §2.
- 🔜 **Planned.** Recognized (detected, shown in the Board), but no engine yet.

The first Office-pack conversion takes about 20 seconds while LibreOffice builds Swirvo' private profile (`%LOCALAPPDATA%\Swirvo\engines\libreoffice-profile`); later ones take a few seconds. Office jobs run one at a time.

Multi-step routes are planned automatically (docs/04 §2). For example, SVG → JPG renders to PNG and then encodes JPG, and DOCX → EPUB goes through the document model. The planner prefers the most faithful route. DOCX → PDF uses our own layout engine (docs/16); when that fails on a file, the job falls back to LibreOffice if installed, then to the built-in typesetter (text, headings, lists and tables).

**Only sensible targets are offered** (`crates/graph/src/sensible.rs`). Chaining engines freely would also find absurd routes, such as a video's first GIF frame run through OCR into a Word file, or JSON typeset as a PDF. The rule looks only at the source and the final target; intermediate steps are free.

| Source | Offered targets |
|---|---|
| Image | Images and PDF. Photos and screenshots (JPG, PNG, WebP, BMP, TIFF, GIF, HEIC, AVIF, JXL) can also become TXT or DOCX through OCR. |
| Video | Video, audio (the soundtrack), animated GIF. Still frames are a tool, not a conversion. |
| Audio | Audio. |
| PDF | Documents, e-books, text, and its tables as XLSX or CSV. Page images are a tool (page picker). |
| Document, e-book, text | Documents, PDF, e-books, text. |
| Slides | Slides, PDF, text. |
| Spreadsheet | Spreadsheets, data, PDF, text, and DOCX/ODT/HTML/MD as a table. |
| Data (JSON, YAML, TOML, XML) | Data and spreadsheets. |
| Subtitles | Subtitles and a TXT transcript. |
| Archive, font | Same category. |

## Images

| From | To | Status | Notes |
|---|---|---|---|
| JPG, PNG, WebP, GIF, BMP, TIFF, ICO, TGA, QOI, PNM | each other, PDF | ✅ | EXIF orientation applied; JPEG → PDF embeds the JPEG unchanged; transparency is flattened onto white for JPG/PPM |
| SVG | PNG (then any raster format), PDF | ✅ | resvg; rendered at 2× for sharpness, max 8192 px |
| EMF, WMF (Windows metafiles) | PDF, SVG, PNG (then any raster format) | ✅ | our own GDI playback, drawn as vectors (text stays text); PNG rendered at 2×. Also drawn inside DOCX, ODT, RTF, PPTX and XLSX → PDF ([16](16-docx-to-pdf.md) §12) |
| JPEG XL (JXL) | PNG (then any) | ✅ | jxl-oxide decoder |
| HEIC/HEIF, AVIF | PNG (then any raster format), PDF | 🪟 Codecs | decoded by Windows when the HEIF / AV1 extension is installed |
| PSD | PNG (then any) | ✅ | flattened composite (8/16-bit; RGB, gray, CMYK, indexed); layers aren't kept |
| Camera RAW (DNG, CR2, CR3, NEF, ARW) | JPG (then any) | ✅ | the camera's embedded full-size JPEG preview, oriented; copied byte for byte when upright |
| Camera RAW | PNG with a full develop | 🪟 Codecs | Windows Raw Image Extension |
| any raster image | TXT (then DOCX, MD, PDF…) | 🪟 OCR | recognized text; layout isn't kept |
| any raster | AVIF, JXL, HEIC | 🔜 | encoders not bundled yet (HEIC through WIC is next) |
| any raster | SVG (tracing) | 🔜 | vtracer |

## Documents and PDF

The built-in document engine reads every format below into one document model (headings, paragraphs, bold/italic/code/links, lists, quotes, code blocks, tables, page breaks, pictures) and writes it back out.

**Pictures** are read from DOCX, ODT, RTF, EPUB and PDF files, from pictures embedded in HTML (`data:` URIs), and from Markdown (`data:` URIs, and files in the folder `<name> files` next to `<name>.md`, nothing else). They are written to **HTML**, embedded in the page so it stays one self-contained file; to **EPUB**, as files inside the book; to **DOCX** and **ODT**, as pictures inside the file (each stored once however often it appears); and to the typeset **PDF**. For HTML and EPUB, EMF and WMF become SVG and other bitmaps become PNG, so browsers and e-book readers can show them. DOCX and ODT keep PNG, JPEG, GIF, EMF and WMF as they are and turn other bitmaps and SVG into PNG; the typeset PDF draws metafiles and SVG as 2× pictures. Pictures keep the width they had in the source (or their own size at 96 dpi), but never get wider than the text or taller than a page. Only pictures inside the document are used: a picture that links to a file elsewhere on the PC, or a web page's `<img src="photo.jpg">`, is never loaded. **Pictures in table cells** are kept too (DOCX, ODT, HTML, EPUB and Markdown readers): each cell can hold pictures after its text, and every writer but plain text draws them inside the cell, fitted to the cell's width (in the typeset PDF also no taller than a third of a page, so a row fits on one). Where in the cell's text a picture stood isn't kept; it follows the text. **Markdown** gets each picture of up to 256 KB (as written: PNG, JPEG, GIF, WebP, or SVG for metafiles) inside the file as a `data:` URI, so the `.md` stays one file; bigger ones are left out with a note in their place (*picture left out: too large for Markdown*). Writing them as files in a `<name> files` folder would be tidier, but the engine only sees a temporary output that the job renames into place afterwards (possibly after more conversion steps), so it can't know where that folder belongs. Plain text leaves pictures out.

| From | To | Status | Notes |
|---|---|---|---|
| PDF | DOCX, ODT, TXT, MD, HTML, EPUB | ✅ | text, paragraphs, page breaks and pictures (in reading order; background pictures are left out; MD: up to 256 KB each); layout and fonts are not kept |
| DOCX, ODT, RTF | PDF, TXT, MD, HTML, EPUB, and each other | ✅ | headings, lists, tables, bold/italic, links and pictures, also in table cells (not in TXT; MD: up to 256 KB each); exact layout is not kept |
| Markdown | HTML, TXT (direct), DOCX, ODT, RTF*, PDF, EPUB | ✅ | *RTF output is 🔜 |
| HTML | MD, TXT, DOCX, ODT, PDF, EPUB | ✅ | pictures embedded in the page come along, also in table cells (not in TXT; MD: up to 256 KB each) |
| TXT | DOCX, ODT, PDF, HTML, MD, EPUB | ✅ | |
| EPUB | TXT, MD, HTML, DOCX, ODT, PDF | ✅ | chapters in reading order; pictures (including covers), not in TXT (MD: up to 256 KB each) |
| ODT, RTF | PDF laid out by the DOCX layout core: styles, lists, tables, pictures, sections, headers and footers, page numbers | ✅ | docs/16 §10 |
| PDF | XLSX: tables as cells, numbers as numbers, a sheet per page plus "All tables" | ✅ | docs/17 §2; scans are OCR'd first |
| DOCX, DOC, ODT, RTF | PDF with exact layout; DOC ⇄ DOCX ⇄ ODT ⇄ RTF | 🧩 Office | LibreOffice, headless |
| DOCX, DOC, ODT, RTF; XLSX, XLS, ODS; PPTX, PPT, ODP | PDF exactly as Office prints it; DOC → DOCX, XLS → XLSX, PPT → PPTX | 🪟 Office | Microsoft Office when installed (Microsoft 365, 2016+), through COM automation, macros off (docs/12 §8) |
| scanned PDF (no text layer) | DOCX, ODT, TXT, MD, HTML, EPUB | 🪟 OCR | pages are rendered at 200 dpi and recognized |
| PDF | images per page | ✅ (Tool) | **Tools → Pages JPG / PNG**: a page picker opens for multi-page PDFs (docs/12 §4) |
| PDF | selected pages → new PDF, split, rotate, delete | ✅ (Tool) | **Tools → Pages…** |
| Pages (Apple) | PDF, DOCX | 🔜 | iWork import |
| PDF | DOCX with layout and images | 🔜 | layout reconstruction (docs/04 §19) |

## Slides

| From | To | Status |
|---|---|---|
| PPTX | PDF: every slide drawn by our own engine (shapes, pictures, tables, text, SmartArt, charts), docs/16 §8 | ✅ |
| PPTX, PPT, ODP | PDF, each other | 🧩 Office |
| Keynote | PDF, PPTX | 🔜 |

## Spreadsheets and data

| From | To | Status | Notes |
|---|---|---|---|
| XLSX, XLS, ODS | CSV, TSV, JSON | ✅ | first non-empty sheet; formulas as values |
| CSV, TSV, JSON | XLSX | ✅ | typed cells, bold header |
| XLSX, XLS, ODS, CSV, TSV | HTML, MD, DOCX, ODT, PDF (as a table) | ✅ | |
| JSON, NDJSON, YAML, TOML, CSV, TSV | each other | ✅ | type inference; `007` stays text |
| XLSX | PDF printed like Excel: print area, page setup, fit to pages, repeated titles, breaks, styles, number formats, merges, pictures, headers and footers | ✅ | docs/16 §9; charts drawn (§11) |
| XLS, ODS | PDF with sheet layout; XLS ⇄ XLSX ⇄ ODS | 🧩 Office | |
| XML ⇄ JSON (then YAML, TOML, CSV…) | | ✅ | attributes as `@name`, text as `#text`, repeated elements as arrays |
| Parquet ⇄ CSV/JSON | | 🔜 | |
| Numbers (Apple) | XLSX, CSV | 🔜 | |

## Audio

| From | To | Status | Notes |
|---|---|---|---|
| MP3, M4A/AAC, FLAC, OGG (Vorbis), WAV, AIFF, CAF | WAV, FLAC | ✅ | symphonia decoder; FLAC encoder in pure Rust; WAV/FLAC are lossless re-wraps of the decoded audio |
| MP3, M4A, AAC, WAV, WMA, FLAC, AC3, AMR | MP3, M4A, AAC, WAV | ✅ 🪟 | Windows Media Foundation (`os-media`): Windows' own encoders, nothing bundled |
| any audio | MP3, M4A, AAC, OGG, Opus, WAV, FLAC, AIFF | 🧩 Media | FFmpeg |
| Opus, WMA, AMR, AC3 | anything | 🧩 Media | |

## Video

| From | To | Status | Notes |
|---|---|---|---|
| MP4, MOV, MKV, WebM, AVI, WMV, 3GP, TS | MP4 (H.264 + AAC), animated GIF; the soundtrack as MP3, M4A, AAC, WAV | ✅ 🪟 | Windows Media Foundation; hardware encoders when the GPU has them. Checked: MKV, WMV, AVI → MP4 in about 3 s for a 4 s 720p clip |
| MP4, MOV, MKV, WebM, AVI, WMV, FLV, TS, 3GP, OGV | MP4, MKV, WebM, MOV, AVI, GIF | 🧩 Media | FFmpeg; remux when codecs allow |
| any video | MP3, M4A, WAV, FLAC, OGG, Opus (extract audio) | 🧩 Media | |
| any video | built-in remux / audio extraction | 🔜 | pure-Rust demuxers |

## Subtitles

| From | To | Status |
|---|---|---|
| SRT, VTT, ASS/SSA, SBV | each other, TXT | ✅ |
| TTML/DFXP | others | 🔜 |

## Archives

| From | To | Status | Notes |
|---|---|---|---|
| ZIP, TAR, TAR.GZ, TAR.BZ2, TAR.XZ | each other | ✅ | streamed entry by entry; timestamps and permissions kept; unsafe paths dropped |
| TAR.ZST | the others, and back | ✅ (Linux, CLI) / 🔜 (Windows app) | the Windows build has no C compiler for zstd yet |
| 7Z, RAR, ISO, CAB | ZIP, TAR (then the others), **Tools → Extract here** | 🪟 tar | libarchive refuses absolute paths and `..` |
| anything | 7Z, RAR | 🔜 | |
| any archive above | **Tools → Extract here / Test** | ✅ | |
| files and folders | **Actions → Compress to ZIP** | ✅ | |

## Ebooks and fonts

| From | To | Status |
|---|---|---|
| EPUB | TXT, MD, HTML, DOCX, ODT, PDF | ✅ |
| MOBI, AZW3, FB2 | EPUB, PDF | 🔜 (Calibre pack) |
| TTF, OTF ⇄ WOFF ⇄ WOFF2 | | ✅ |
| TTC (collections) | | 🔜 |
