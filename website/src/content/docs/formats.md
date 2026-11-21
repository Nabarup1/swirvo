---
title: "Formats and conversions"
description: "What Swirvo can convert, what does the work, and why some targets appear only on some PCs."
section: Using Swirvo
order: 12
short: "Formats"
---

Swirvo knows about a hundred formats across images, documents, PDF, slides, spreadsheets, data, audio, video, subtitles, e-books, archives and fonts. The [formats page](/formats) lists every one with what it can become; this page explains how it works.

## Only what works on your PC

The wheel only ever shows targets that work on **this** PC. Some conversions are built into Swirvo; others use something Windows already has, or a program you installed:

| Label | What does the work |
|---|---|
| **Built in** | Swirvo's own engines. Works with nothing else installed. |
| **Windows** | An engine Windows already ships: image codecs (HEIC, AVIF, RAW), Media Foundation (audio and video), `tar` (7Z, RAR, ISO, CAB). Adds 0 bytes to Swirvo. |
| **OCR** | Windows' text recognition reads photos and scans. |
| **Office** | Microsoft Office or LibreOffice, when installed, for Office files exactly as Office prints them, and old formats like DOC, XLS and PPT. |
| **FFmpeg** | More audio and video formats, when FFmpeg is installed. |

## Only sensible targets

Chaining engines freely would find absurd routes, like a video's first frame read by OCR into a Word file. Swirvo only offers targets that make sense for the source:

| From | You'll be offered |
|---|---|
| Images | Other images and PDF. Photos and screenshots can also become text or Word through OCR. |
| Video | Video, audio (the soundtrack), animated GIF. A still frame is a tool. |
| Audio | Audio. |
| PDF | Documents, e-books, text, and its tables as Excel or CSV. Page images are a tool. |
| Documents, e-books, text | Documents, PDF, e-books, text. |
| Slides | Slides and PDF. |
| Spreadsheets | Spreadsheets, data, PDF, text, and Word, ODT, HTML or Markdown as a table. |
| Data (JSON, YAML, TOML, XML) | Data and spreadsheets. |
| Subtitles | Subtitles and a plain-text transcript. |
| Archives, fonts | The same category. |

When a conversion needs several steps (SVG → PNG → JPG, say), Swirvo plans the route by itself and prefers the most faithful one.

## Highlights

- **PDF to Word** rebuilds the layout: fonts, columns, tables and pictures. Scanned PDFs are read with OCR first.
- **PDF to Excel** turns tables into real cells: numbers as numbers, merged cells merged, a sheet per page plus an "All tables" sheet.
- **Word, Excel and PowerPoint to PDF** without Office installed: Swirvo has its own layout engines. Excel prints with its print area, page setup, styles and charts.
- **PDF/A, tagged PDF and PDF/UA** from Word, ODT, RTF, PowerPoint or Excel. See [PDF tools](/docs/pdf-tools#pdfa-archives-and-accessibility).
- **Camera RAW to JPG** uses the camera's full-size embedded preview; a full develop to PNG uses Windows' Raw Image Extension.
- **Data** converts with care: `007` stays text, and XML attributes become `@name`.

## HEIC and AVIF photos

iPhone photos (HEIC) and AVIF images are decoded by Windows. If they don't appear on the wheel, install the free **HEIF Image Extensions** (and the **AV1 Video Extension** for AVIF) from the Microsoft Store.

## Check a file's real type

Swirvo identifies files by their content, not their extension, and flags extensions that lie. From a terminal:

```bash
swirvo detect "mystery.bin"
```
