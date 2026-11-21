---
title: "The Board"
description: "The workspace behind the wheel, for folders of files, tools with options, previews and search."
section: Using Swirvo
order: 11
---

The Board is where you go when one drag isn't enough: a folder of photos, a stack of PDFs, or a tool with options to set.

## Open it

- **Double-click the tray icon**, or right-click it and choose **Open Board**.
- Mid-drag, tap <kbd>Alt</kbd> and drop on **Board**: the files you're dragging open in it.
- On the Convert wheel, **More…** opens the Board too.

## Add files

Drop files or whole folders anywhere on the Board, or use **Add files**. Every file becomes a card with a thumbnail (photos, SVGs, and the first page of PDFs, Word, PowerPoint, Excel, ODT and RTF files), its type, name and size.

Click a card to select it; <kbd>Ctrl</kbd>-click and <kbd>Shift</kbd>-click add to the selection; <kbd>Ctrl</kbd> <kbd>A</kbd> selects all; <kbd>Delete</kbd> removes cards from the Board (never from disk).

## Convert

The **Convert** tab has one row per type of file on the Board, each with a target:

```
PNG  · 8 files   → JPG
PDF  · 2 files   → DOCX
HEIC · 3 files   → JPG
```

Each dropdown lists every target that type can reach, best first, ranked the same way as the wheel. Then set:

- **Quality**: Best, Balanced or Smallest.
- **Save to**: next to the originals, or a folder you choose.

Press **Convert**. It acts on the selected cards if you've selected some, otherwise on everything that's waiting. Files run in parallel, one per core by default; while they run, the button becomes **Cancel**. Each card's status reads, for example, `PNG → JPG`, then a spinner, then `✓ 1.2 MB · −62%` or the reason it failed. Finished cards offer **Show in folder**.

## Tools

The **Tools** tab lists the tools that fit the selected files, each with **Run**. Tools with options (a password, watermark text, page ranges, a target size) show a form.

### Live previews

For PDF tools that change how pages look, the card grid gives way to a **preview of the page as the tool will leave it**: watermark, page numbers, header and footer, Bates numbers, crop, trim, resize, N-up, booklet, grayscale, redact and compress. Switch between *After* and *Before*, and step through pages. Nothing is written until you run the tool.

You can also set things by pointing:

- **Crop:** drag the box's edges or corners on the page, for every page or just this one.
- **Watermark:** drag it anywhere, and scroll over the page to make it bigger or smaller.
- **Page numbers and Bates:** click one of six spots on the page.
- **Digital signature:** drag the signature box to where it should go, on any page.

## Search

<kbd>Ctrl</kbd> <kbd>F</kbd> searches everything at once: files on the Board by name, formats in the Convert list, and tools by name or description. Tools that match but don't fit the files on the Board are listed under **Not for these files**, so you can find a tool before you've added its file. <kbd>Esc</kbd> clears the search.

## Status bar

Along the bottom: how many files and how much data is on the Board, and which optional engines Swirvo found on this PC (Office, FFmpeg, OCR).
