# 19 · Swirvo for AI assistants (MCP)

`swirvo mcp` runs Swirvo as a [Model Context Protocol](https://modelcontextprotocol.io) server. Any MCP client (Claude Desktop, Claude Code, Cursor, VS Code, Windsurf and others) can then convert files, run Swirvo's tools and read text out of documents **on your computer**. No API key, no upload, no account.

Ask your assistant things like:

- "Convert every PDF in my Downloads folder to Word."
- "Merge these three PDFs, add page numbers and a DRAFT watermark."
- "Read the text of this scanned invoice and tell me the total." (OCR on Windows)
- "Redact the email addresses and phone numbers in contract.pdf."
- "Turn report.docx into a PDF/A file for archiving."

## Tools

| Tool | What it does | Writes files? |
|---|---|---|
| `list_formats` | All formats by category, or (with `from`) what one format can become, and whether it's lossless | no |
| `detect_format` | The real format of each file from its content, flagging extensions that lie | no |
| `convert_files` | Convert files to a format (`to`), with `preset`, `out_dir` and a PDF profile (tagged, PDF/A-2, PDF/A-3, PDF/UA) | new files only |
| `list_tools` | The tools that apply to given files, with every option's key, type, choices and default | no |
| `run_tool` | Run one tool (`pdf-merge`, `pdf-watermark`, `pdf-redact`, `pdf-searchable`, `docx-replace`, …) with `options`, or a PDF page tool with `pages` | new files only |
| `extract_text` | The text of PDFs, Word/ODT/RTF, slides, sheets, Markdown, HTML and images (OCR on Windows) | no |

Results are always **new files** next to the originals (or in `out_dir`). Originals are never changed, so an assistant can safely retry. Tool calls run one at a time; a crash in an engine fails only that call.

## Setup

The command is `swirvo mcp`. Installing Swirvo (from the Microsoft Store, or by running the downloaded `Swirvo.exe` and accepting the install) puts `swirvo` on your PATH. Otherwise use the full path:

| How you got Swirvo | Command |
|---|---|
| Installed from the GitHub download | `swirvo` (or `%LOCALAPPDATA%\Programs\Swirvo\bin\swirvo.exe`) |
| Microsoft Store | `swirvo` |
| Unzipped, not installed | the full path to `swirvo-cli.exe` in that folder |
| Linux (install script or `.deb`) | `swirvo` (the script installs to `~/.local/bin`) |

The examples below use `swirvo`. If you installed Swirvo while the AI app was open, restart the app so it sees the new PATH; or put the full path instead (double the backslashes inside JSON).

### Claude Code and Cowork: the Swirvo plugin

The [`plugin/`](../plugin) folder is a Claude plugin with a skill that teaches Claude to use the `swirvo` command line (convert, tools, read text). It needs Swirvo installed so `swirvo` is on your PATH. For the MCP tools as well, add the server with `claude mcp add` below.

```bash
claude --plugin-dir ./plugin
```

### Claude Desktop: one click

Download `Swirvo-<version>.mcpb` from the [latest release](https://github.com/Nabarup1/swirvo/releases/latest) and double-click it (or drag it onto Claude Desktop's Settings → Extensions). Claude Desktop installs Swirvo as an extension; nothing else to set up. Windows only.

### Claude Desktop: by hand

Settings → Developer → Edit Config, then add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "swirvo": {
      "command": "swirvo",
      "args": ["mcp"]
    }
  }
}
```

Restart Claude Desktop.

### Claude Code

```bash
claude mcp add swirvo -- swirvo mcp
```

### Cursor

`.cursor/mcp.json` in a project, or `~/.cursor/mcp.json` for all projects: the same `mcpServers` block as Claude Desktop.

### VS Code

`.vscode/mcp.json`:

```json
{
  "servers": {
    "swirvo": {
      "type": "stdio",
      "command": "swirvo",
      "args": ["mcp"]
    }
  }
}
```

### Any other client

Start `swirvo mcp` (or the full path to `swirvo-cli.exe` with `mcp`) as a stdio server. Swirvo is also listed in the official [MCP Registry](https://registry.modelcontextprotocol.io) as `io.github.Nabarup1/swirvo`. It speaks MCP protocol versions 2024-11-05 through 2025-11-25.

## Notes

- **Give absolute paths.** The server resolves relative paths against its own working folder, which depends on the client.
- **Access:** the server can read any file your user account can, and writes new files next to them. It never deletes or overwrites anything, and makes no network connections.
- **Windows-only engines** (OCR, Media Foundation video/audio, HEIC/AVIF/RAW through Windows codecs, Microsoft Office) aren't available on Linux; `list_formats` and `list_tools` show what this PC can do.
- Implementation: `crates/cli/src/mcp.rs` (JSON-RPC 2.0 over stdio, no extra dependencies).
