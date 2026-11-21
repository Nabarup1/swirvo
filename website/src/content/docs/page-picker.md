---
title: "The page picker"
description: "Pick pages of a long PDF by clicking or typing, then export them, split them into groups, rotate, delete or reorganise."
section: Using Swirvo
order: 14
short: "Page picker"
---

When you drop a PDF with more than one page on **Pages JPG**, **Pages PNG**, **Split**, **Rotate** or **Pages…**, the page picker opens. Every page starts selected, so the plain result is one <kbd>Enter</kbd> away.

## Choose pages

- **Click** a page to select or deselect it; <kbd>Shift</kbd>-click selects a range.
- **Press and sweep** across pages to give them all the same state.
- **Type** in the Pages field: `1-3, 7, 10-` (a trailing `-` means "to the end"), or `odd`, `even`, `all`. The field and the grid stay in sync both ways; a mistake turns the field red and keeps the last good selection.
- **All**, **None**, **Odd**, **Even** and **Invert** do what they say.

Thumbnails render around where you're looking first, so even a 500-page PDF scrolls smoothly.

## Then do something

| Operation | Result, next to the PDF |
|---|---|
| **Images JPG / PNG** | `Report (images)\Report p03.jpg`, at 150 or 300 dpi (one page gives a single file) |
| **New PDF** | `Report (pages 1-3, 7).pdf` |
| **Split · Each page** | `Report (pages)\Report p03.pdf` for each selected page |
| **Split · Groups** | `Report (parts)\Report 1 (p1-15, 16, 18-20).pdf`, one per group |
| **Rotate** | `Report (rotated).pdf`, only the selected pages turned |
| **Delete** | `Report (edited).pdf` without the selected pages |

After an export, a bar says what was saved, with **Show in folder**. <kbd>Esc</kbd> closes the picker.

## Split into groups

Choose **Split**, then **Groups**. Each output PDF gets a chip (*PDF 1 · 17 pages*), and **+ PDF** adds another. Click pages to add them to the active group; each page shows a numbered dot in its group's colour (groups may share pages). The whole split is also text you can type or paste:

```
1-15, 16, 18-20; 25-40, 42
```

A `;`, a `|` or a new line starts the next PDF. Mistakes are named precisely ("PDF 2: page 61 doesn't exist"). The same split is the `pdf-split-ranges` tool on the command line.

## Organise

The last operation, **Organise**, turns the pages into tiles you rearrange by hand. Nothing is written until **Save as a new PDF**, which writes `Report (organised).pdf`.

- **Drag tiles** to reorder; a selected group moves together, and the view scrolls near the edges.
- **Rotate**, **Duplicate** (<kbd>Ctrl</kbd> <kbd>D</kbd>), **Delete** (<kbd>Del</kbd>), **Blank page**, and **Insert PDF…** after the selection (or drop PDFs on the window).
- **Undo** with <kbd>Ctrl</kbd> <kbd>Z</kbd>, up to 50 steps.
- **Edit page** (<kbd>Enter</kbd> or double-click) opens the page large, with:
  - **Comments**: highlight, note, text, drawing and box, in five colours, saved as real PDF comments.
  - **Sign**: place a picture of your signature (PNG or JPEG).
  - **Black out**: boxes whose content is removed for good.
  - **Crop**: drag the part to keep.

When the pages stay in their order, the PDF is edited in place, so links, form fields and bookmarks survive and a protected PDF stays protected.
