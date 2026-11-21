---
title: "Privacy and security"
description: "Why nothing leaves your PC, how conversions are isolated, and how to check that a download is genuine."
section: Help
order: 30
---

## Nothing leaves your PC

- Swirvo has **no network code**: no uploads, no account, no ads, no analytics, no crash reports sent anywhere.
- **Offline mode** is on by default, so even future features that could use the network stay off unless you turn it off.
- Your files are processed in memory and on your disk, and the results stay where you save them.

The full text is the [privacy policy](/privacy).

## Isolated conversions

Every conversion runs in a **separate, restricted worker process** with a memory cap. A damaged or hostile file can only crash its own job: the app, the wheel and your other jobs carry on, and the result says what went wrong.

## Your files are never changed

Results are always new files, written to a temporary name and moved into place in one step; nothing existing is ever replaced. See [Where files go](/docs/output).

## Passwords

Swirvo asks for a PDF password when a tool needs one. If you forget a password you set with **Protect**, the file can't be recovered by anyone, Swirvo included.

## Genuine downloads

- The **Microsoft Store** version is signed by Microsoft.
- The GitHub downloads aren't code-signed yet; each one has a `.sha256` checksum next to it. Compare it with:

```powershell
Get-FileHash .\Swirvo-0.1.2-windows-x64.zip -Algorithm SHA256
```

```bash
sha256sum -c swirvo-cli-0.1.2-linux-x64.tar.gz.sha256
```

- Releases are built from the public source by GitHub Actions, from a version tag.

## Report a security problem

Please don't open a public issue for a vulnerability. Follow [SECURITY.md](https://github.com/Nabarup1/swirvo/blob/main/SECURITY.md) in the repository.
