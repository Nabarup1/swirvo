---
title: "Install, update and remove"
description: "Every way to install Swirvo on Windows, how updates work, and how to remove it cleanly."
section: Start
order: 2
short: "Install and update"
---

## What you need

- **Windows 10 version 1809 or later, or Windows 11**, 64-bit.
- No admin rights, no runtime, no drivers, no services.
- About **29 MB** of disk space.

Optional extras that light up more formats when they're installed (Swirvo finds them by itself):

- Windows' **HEIF Image Extensions** and **AV1 Video Extension** for HEIC and AVIF photos, and the **Raw Image Extension** for a full RAW develop. They're common on Windows 11 and free in the Microsoft Store.
- **Microsoft Office** (Microsoft 365, 2016 or later) or **LibreOffice**, for Office files printed exactly as Office prints them, and old formats like DOC and PPT.
- **FFmpeg**, for more audio and video formats.

## Microsoft Store (recommended)

1. Open [Swirvo in the Microsoft Store](https://apps.microsoft.com/detail/9NPGWFBVTDJQ) and click **Get**.
2. Open Swirvo from the Start menu once. The swirl appears in the notification area.

The Store version is signed by Microsoft, updates itself, and makes the `swirvo` command available in every terminal.

## ZIP download

1. Download `Swirvo-<version>-windows-x64.zip` from the [download page](/download).
2. Unzip it anywhere and double-click `Swirvo.exe`.
3. Accept the offer to install. Swirvo copies itself to `%LOCALAPPDATA%\Programs\Swirvo`, adds itself to the Start menu and to **Settings → Apps**, and puts `swirvo` on your user PATH. It doesn't need admin rights.

If you skip the install, Swirvo still runs from the folder you unzipped, and the tray menu keeps an **Install Swirvo…** item until you change your mind. In that case the command-line tool is `swirvo-cli.exe` in the same folder.

### "Windows protected your PC"

The ZIP isn't code-signed yet, so Windows SmartScreen warns the first time you run it. Click **More info → Run anyway**, or install from the Microsoft Store instead, which has no warning. Every download has a `.sha256` checksum next to it if you want to [check it](/download#files-title).

## Start with Windows

Swirvo doesn't run at sign-in unless you turn on **Start with Windows** in [Settings](/docs/settings).

## Updates

- **Store:** automatic.
- **ZIP:** download the latest release and run its `Swirvo.exe`, then accept the install.

What changed in each version is in the [changelog](/changelog).

## Remove Swirvo

Open **Settings → Apps → Installed apps**, find **Swirvo**, and choose **Uninstall**. This works for both the Store and the ZIP install. Uninstalling the ZIP install also takes `swirvo` off your PATH; nothing else in your PATH is touched.

Files Swirvo made are yours: they stay where they were saved.

## Linux

On Linux there's the `swirvo` command-line tool for x64 and ARM64. See [Linux](/docs/linux).
