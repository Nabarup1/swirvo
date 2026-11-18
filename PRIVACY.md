# Swirvo Privacy Policy

*Version 1.0, 17 November 2026.*

This policy covers **Swirvo** ("the app"): the Windows app, its Board and page tools, and the `swirvo` command-line tool. It's made by **Nabarup**, India ("I", "me"). Contact: [GitHub Issues](https://github.com/Nabarup1/swirvo/issues).

## The short version

- The app converts and edits your files **on your computer**.
- It doesn't upload your files. It has no account, no ads, no analytics and no telemetry.
- **I receive no personal information from the app.** The only personal information I get is what you choose to send me, for example in a GitHub issue.

## Your files

- Files you convert or edit are read and written **only on your computer**. Results are saved next to the original, or in a folder you choose. **Your original is never changed.**
- While a conversion runs, the app may write temporary files on your computer. It deletes them when the job finishes.
- **Text recognition (OCR)** uses the engine built into Windows (Tesseract on Linux). It runs on your computer.
- **Other programs you installed.** If you have Microsoft Office, LibreOffice or FFmpeg, the app may hand your file to that program **on your computer** to convert it. Those programs' own privacy terms apply to them.
- **Signing.** When you sign a PDF, the name on your certificate, the signing time, and any reason or location you type are written into **that PDF**. Anyone you send it to can see them.
- **Metadata.** Converted files may keep information from the original, such as author or creation date. The *Remove hidden data* and *Strip metadata* tools remove it.

## The keyboard

To show the wheel, the app needs to know when you press **Shift, Ctrl or Alt** while dragging a file, and **Esc** to cancel. Windows sends the app keyboard events for this (Raw Input). **The app ignores every other key. It doesn't record, store or send any keystrokes.**

## What the app stores on your computer

The app keeps a small settings file in your user profile: `%LOCALAPPDATA%\Swirvo` on Windows, `~/.local/share/swirvo` on Linux. It contains:

- your settings, including any destination folders you chose;
- which conversions you use and when (for example "HEIC → JPG"), so the wheel can put your favourites first.

It contains **no file contents and no file names**. It never leaves your computer.

**To delete it:** uninstall the Microsoft Store version (Windows removes the folder), or delete the folder yourself at any time.

## When the internet is used

The app makes **no network connections of its own**. Two parts of Windows that the app uses can go online:

| When | What happens | Who |
|---|---|---|
| You sign a PDF or check its signatures | Windows may download missing certificates of the certificate's issuer from the address written in the certificate. No part of your file is sent. | The certificate's issuer |

If you click a link in the app (for example to this page), it opens in your web browser.

**Before any feature that uses the internet is added** (for example an update check for the direct-download version), this policy will be updated first, and the feature will say what it sends.

## What Microsoft and GitHub tell me

- **Microsoft Store version:** Microsoft gives the publisher reports in its developer dashboard, for example the number of installs by country, ratings and reviews, and, if Windows error reporting is on for your PC, crash reports for the app. I can't identify you from them, and I use them only to fix bugs and improve the app. Windows' own settings and [Microsoft's privacy statement](https://privacy.microsoft.com) control what Windows sends to Microsoft.
- **GitHub downloads:** GitHub shows the publisher a download count for each release file. Nothing else. [GitHub's privacy statement](https://docs.github.com/site-policy/privacy-policies/github-general-privacy-statement) applies to your visit to GitHub.

## When you contact me

Questions and bug reports go through [GitHub Issues](https://github.com/Nabarup1/swirvo/issues), which are **public**. Don't post personal or confidential information or files there. If you need to report something privately (for example a security problem), use GitHub's private vulnerability reporting, described in [SECURITY.md](SECURITY.md).

- I use what you send only to answer you and fix the problem.
- I don't sell personal information, and I don't use it for advertising.

## Your rights

You can ask me to delete anything you sent me privately. Under the GDPR (EU, EEA, UK), India's Digital Personal Data Protection Act 2023 and US state laws, you have the right to access, correct and delete your personal information. Since the app collects none, in practice this covers only what you sent me yourself. You can delete your own GitHub issues and comments at any time.

## Security

Your files stay on your computer, so they're as safe as your computer. The app doesn't weaken Windows' protections, installs no drivers or services, and runs with your normal user permissions.

## Children

The app isn't directed at children, and it collects no personal information from anyone, including children. If a child has contacted me, a parent can ask me to delete the message.

## Changes

If this policy changes, the new version will be posted here with its date. If a change matters, for example a new feature that uses the internet, the app will also tell you.

## Contact

- **Questions:** [GitHub Issues](https://github.com/Nabarup1/swirvo/issues)
- **Private reports:** see [SECURITY.md](SECURITY.md)
- **Made by:** Nabarup, India
