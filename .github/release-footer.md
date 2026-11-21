## Download

**Easiest on Windows: [Swirvo on the Microsoft Store](https://apps.microsoft.com/detail/9NPGWFBVTDJQ)** (free, signed, updates itself). The files below are the same app for a manual install.

| | File | What it is |
|---|---|---|
| **Windows 10 (1809+) / 11, x64** | `Swirvo-…-windows-x64.zip` | The app: the wheel, the Board, the page picker and `swirvo-cli.exe`. Unzip and double-click `Swirvo.exe`; it offers to install itself (Start menu, no admin rights). |
| **Linux x64 / ARM64** | `swirvo_…_amd64.deb`, `swirvo_…_arm64.deb`, `swirvo-cli-…-linux-x64.tar.gz`, `swirvo-cli-…-linux-arm64.tar.gz` | The `swirvo` command-line tool (convert, tools, detect, MCP server). Runs on glibc 2.28+ (Ubuntu 18.10+, Debian 10+, RHEL 8+, Fedora, Mint 20+). Easiest: `curl -fsSL https://raw.githubusercontent.com/Nabarup1/swirvo/main/install.sh \| sh`. The Linux desktop app is not ready yet. |

**Use it from AI assistants:** double-click `Swirvo-….mcpb` to add Swirvo to Claude Desktop, or run `swirvo mcp` as a local MCP server for Claude Desktop, Claude Code, Cursor and VS Code. Setup: [docs/19](https://github.com/Nabarup1/swirvo/blob/main/docs/19-ai-assistants-mcp.md).

**"Windows protected your PC"?** This build isn't code-signed yet, so Windows SmartScreen warns about it. Click **More info → Run anyway**. The [Microsoft Store version](https://apps.microsoft.com/detail/9NPGWFBVTDJQ) is signed by Microsoft and has no warning. Each file has a `.sha256` checksum next to it.

Free to use at home and at work ([EULA.md](https://github.com/Nabarup1/swirvo/blob/main/EULA.md)). Source code: [PolyForm Noncommercial 1.0.0](https://github.com/Nabarup1/swirvo/blob/main/LICENSE.md). Nothing is uploaded; see the [privacy policy](https://github.com/Nabarup1/swirvo/blob/main/PRIVACY.md).
