---
title: "Troubleshooting"
description: "The wheel doesn't open, a format is missing, a command isn't found, or a file won't convert. Fixes for the common cases."
section: Help
order: 31
---

## The wheel doesn't open

1. **Is Swirvo running?** Look for the swirl in the notification area (check the **^** overflow next to the clock). If it isn't there, open Swirvo from the Start menu.
2. **Is the wheel paused?** Right-click the tray icon and make sure **Pause the wheel** isn't checked.
3. **Are you still holding the mouse button?** Tap the key *during* the drag, while the file is under your pointer.
4. **Which key opens which wheel?** You may have changed it. Check **Settings → Wheel**.

## A format I expected isn't on the wheel

The wheel only shows what works on this PC.

- **HEIC or AVIF photos:** install the free **HEIF Image Extensions** (and **AV1 Video Extension** for AVIF) from the Microsoft Store.
- **DOC, XLS, PPT, or an exact Office layout:** these need **Microsoft Office** or **LibreOffice** installed, and turned on in **Settings → Engines**.
- **Unusual audio or video formats:** install **FFmpeg** and make sure `ffmpeg.exe` is on your PATH.
- **More than eight targets:** the last wedge is **More…**, which opens the full list in the Board.

`swirvo formats --supported` lists exactly what this PC can do, and the [formats page](/formats) shows what each format needs.

## "swirvo" isn't recognised in the terminal

- **Open a new terminal** after installing: terminals that were already open don't see the new PATH.
- **Microsoft Store install:** open **Settings → Apps → Advanced app settings → App execution aliases** and make sure Swirvo's alias is on.
- **Unzipped but not installed:** use `swirvo-cli.exe` in the folder you unzipped, or run `Swirvo.exe` once and accept the install.
- **AI apps:** restart the app after installing so it sees the new PATH.

## "Windows protected your PC"

The ZIP download isn't code-signed yet, so SmartScreen warns about it. Click **More info → Run anyway**, or install from the [Microsoft Store](https://apps.microsoft.com/detail/9NPGWFBVTDJQ), which is signed by Microsoft. You can check any download against its [checksum](/docs/privacy-security#genuine-downloads).

## A file won't convert

The result tells you why. Common reasons:

- **The file is damaged.** Try **Repair** for a PDF; for other files, open and re-save them in their own app.
- **It's password-protected.** Use **Unlock** with the password first (in the Board, the password is one of the tool's options). A PDF restricted with an owner password needs that password.
- **It's open in another app.** Close it and try again.
- **It isn't what its extension says.** `swirvo detect FILE` shows the real type.

## Where did my file go?

Next to the original, unless you chose a folder in Settings or the Board. If that folder couldn't be written to, it's in **`Downloads\Swirvo`**. See [Where files go](/docs/output).

## Text recognition (OCR) gets the language wrong

Windows' OCR reads the languages installed on your PC. Add a language in **Settings → Time & language → Language & region**, including its optical character recognition feature.

## Still stuck?

[Open an issue on GitHub](https://github.com/Nabarup1/swirvo/issues/new) with your Windows version, Swirvo's version (**Settings → About**), what you did and what happened. A sample file helps, if it isn't private.
