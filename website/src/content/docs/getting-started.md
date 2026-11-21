---
title: "Getting started"
description: "Install Swirvo, convert your first file with one gesture, and find your way around in five minutes."
section: Start
order: 1
---

Swirvo is a file converter and PDF toolkit that lives in your notification area and wakes up in the middle of a drag. There is no window to open: you start dragging a file, tap a key, and drop it on what you want.

## 1. Install it

The easiest way is the **[Microsoft Store](https://apps.microsoft.com/detail/9NPGWFBVTDJQ)**: it's signed by Microsoft, installs in one click and keeps itself up to date.

You can also download the ZIP from the [download page](/download), unzip it and double-click `Swirvo.exe`. It offers to install itself for your user, with no admin rights. Either way, the Swirvo swirl appears in the notification area next to the clock.

> **Tip:** On Windows 11 new icons sometimes hide in the overflow (the **^** next to the clock). Drag the swirl onto the taskbar to keep it in sight.

## 2. Convert your first file

1. In File Explorer (or on the desktop), **start dragging** any file. Keep holding the mouse button.
2. **Tap Shift** once. A wheel of formats opens around your pointer.
3. **Drop the file on a wedge**, say JPG for a photo or DOCX for a PDF.

The converted copy appears next to the original. Your original is never changed, and if a file with that name already exists, the new one gets a fresh name such as `photo (2).jpg`.

You can try this right now on the [homepage demo](/#try), without installing anything.

## 3. Two more keys

| Tap while dragging | Wheel | What's on it |
|---|---|---|
| <kbd>Shift</kbd> | **Convert** | Every format the file can become |
| <kbd>Ctrl</kbd> | **Tools** | Resize, compress, merge, split, watermark, protect… only the ones that fit the file |
| <kbd>Alt</kbd> | **Actions** | Show in folder, copy path, terminal here, to ZIP, open in the Board, copy text |

Tap another key while the wheel is open to switch wheels. Rest on a Convert wedge for half a second to choose **Best**, **Balanced** or **Smallest**.

To change your mind, press <kbd>Esc</kbd>, drop on the middle of the wheel, or let go outside it. The drag carries on as normal, so you can still drop the file on a folder.

## 4. Bigger jobs: the Board

Double-click the tray icon to open **the Board**. Drop in a whole folder, pick a target for each type (all PNGs to JPG, all PDFs to Word), and press **Convert**: everything runs in parallel. The Board also has every tool with its options, live previews for PDF edits, and search with <kbd>Ctrl</kbd> <kbd>F</kbd>. See [The Board](/docs/board).

## 5. From a terminal

Installing Swirvo also gives you the `swirvo` command:

```bash
swirvo convert report.pdf --to docx
swirvo tool pdf-merge a.pdf b.pdf c.pdf
swirvo formats --from heic
```

See [Command line](/docs/command-line), or connect Swirvo to an AI assistant with [MCP](/docs/ai-assistants).

## Where next

- [The wheel](/docs/wheel), in detail
- [PDF tools](/docs/pdf-tools): compress, split, redact, sign and more
- [Settings](/docs/settings): keys, quality, where files go
- [Troubleshooting](/docs/troubleshooting), if something doesn't work
