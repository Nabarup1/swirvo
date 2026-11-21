---
title: "Images, Word, video and files"
description: "The tools for photos, Word documents, video and any file, with what each one does exactly."
section: Using Swirvo
order: 15
short: "Other tools"
---

These appear on the <kbd>Ctrl</kbd> wheel when they fit the file you're dragging, and in the Board's Tools tab. Each writes a new file next to the original.

## Images

| Tool | What it does |
|---|---|
| **Resize · 50 %** | Half the width and height, with a high-quality Lanczos filter. |
| **Fit 1920** | The longest side at most 1920 px; smaller images are left alone. |
| **Rotate · 90° right** | Turns the image clockwise. |
| **Flip** | Mirrors it horizontally. |
| **Compress** | JPEG at quality 72, PNG at maximum compression, keeping whichever file is smaller. |
| **Strip info** | Removes EXIF (GPS location, camera), XMP and IPTC data before you share a photo. For JPEG it's lossless. |
| **To one PDF** | Several images become one PDF, a page per image, in name order. |

## Word and Office documents

| Tool | What it does |
|---|---|
| **Compress** | Pictures shrunk to the size they're displayed at; photos become JPEG, screenshots stay lossless. |
| **Clean** | Removes author, company and last editor, comments and editing ids; optionally accepts all tracked changes. |
| **Replace** | Find and replace in the body, headers, footers and notes, even where Word split a word into pieces. Match case and whole words are options. |
| **Pictures** | Saves every picture in the document into a folder. |
| **Word count** | Words, characters, paragraphs and pages, copied to the clipboard. |
| **Mail merge** | A letter with `{{Name}}`, `«Name»` or merge fields, plus a spreadsheet: one DOCX per row, and a PDF each if you like, named from a column. |
| **PDF/A** | Word, ODT, RTF, PowerPoint or Excel to an archival PDF. See [PDF tools](/docs/pdf-tools#pdfa-archives-and-accessibility). |

## Video

| Tool | What it does |
|---|---|
| **Frame** | A picture of one moment. Type a time (`1:23`, `83` or `40 %`), or let Swirvo pick a good frame near the start, skipping black and blank frames. Or one picture every N seconds into a new folder. PNG or JPG. |
| **Compress · to a size** | Email 25 MB, chat 10 or 8 MB, upload 50 or 100 MB, or any size. Gives `clip (25 MB).mp4` in H.264 and AAC. A video already under the size is left alone. |

How **Compress to a size** works: it aims at 94 % of the target, gives the sound a fair share and the picture the rest, and scales the picture down (never up) when it would otherwise turn to mush. If the result is still over, it tries once more with less; a target that's simply too small is refused with the smallest size that would work ("try at least 6.2 MB"). A megabyte here is 1,000,000 bytes, so a limit is never crossed.

Video uses Windows Media Foundation, with your graphics card's encoder when it has one. If FFmpeg is installed, Swirvo uses it instead, encoding in two passes.

## Archives

| Tool | What it does |
|---|---|
| **Extract** | Into a new folder named after the archive; never merges into an existing folder. ZIP, TAR and its compressed forms are built in; 7Z, RAR, ISO and CAB use Windows' own `tar`. Unsafe paths inside archives are refused. |
| **Test** | Reads every entry and reports files, size and errors. |

## Any file

| Tool | What it does |
|---|---|
| **Checksum** | The SHA-256 of each file, copied to the clipboard. |
| **To ZIP** | All the dragged files into one ZIP (on the <kbd>Alt</kbd> wheel). |
| **Copy text** | The text of a document, slide deck, sheet or text file onto the clipboard; photos and screenshots are read with OCR (on the <kbd>Alt</kbd> wheel). |
