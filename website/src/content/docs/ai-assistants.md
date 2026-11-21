---
title: "AI assistants (MCP)"
description: "Connect Swirvo to Claude Desktop, Claude Code, Cursor, VS Code or any MCP client, so your assistant can convert and read files locally."
section: Power users
order: 21
short: "AI assistants"
---

`swirvo mcp` runs Swirvo as a [Model Context Protocol](https://modelcontextprotocol.io) server. Any MCP client can then convert files, run Swirvo's tools and read text out of documents **on your computer**. No API key, no upload, no account. There's an overview on the [AI page](/ai).

## Claude Desktop: one click

Download `Swirvo-<version>.mcpb` from the [latest release](https://github.com/Nabarup1/swirvo/releases/latest) and double-click it, or drag it onto Claude Desktop's **Settings → Extensions**. Windows only.

## Claude Desktop: by hand

**Settings → Developer → Edit Config**, then add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "swirvo": { "command": "swirvo", "args": ["mcp"] }
  }
}
```

Restart Claude Desktop.

## Claude Code

```bash
claude mcp add swirvo -- swirvo mcp
```

There's also a Claude plugin with a skill that teaches Claude to use the `swirvo` command line, for Claude Code and Cowork, in the repository's [plugin folder](https://github.com/Nabarup1/swirvo/tree/main/plugin).

## Cursor

`.cursor/mcp.json` in a project, or `~/.cursor/mcp.json` for all projects, with the same `mcpServers` block as Claude Desktop.

## VS Code

`.vscode/mcp.json`:

```json
{
  "servers": {
    "swirvo": { "type": "stdio", "command": "swirvo", "args": ["mcp"] }
  }
}
```

## Any other client

Start `swirvo mcp` as a stdio server (or the full path to `swirvo-cli.exe` with `mcp` for an unzipped copy). It speaks MCP protocol versions 2024-11-05 to 2025-11-25, and is listed in the official [MCP Registry](https://registry.modelcontextprotocol.io) as `io.github.Nabarup1/swirvo`.

If you installed Swirvo while the AI app was open, restart the app so it sees the new PATH, or use the full path (with doubled backslashes inside JSON).

## The tools

| Tool | What it does | Writes files? |
|---|---|---|
| `list_formats` | All formats by category, or what one format can become, and whether it's lossless | no |
| `detect_format` | The real format of each file, flagging extensions that lie | no |
| `convert_files` | Convert files, with a preset, an output folder and a PDF profile | new files only |
| `list_tools` | The tools for given files, with every option's key, type, choices and default | no |
| `run_tool` | Run one tool with options, or a PDF page tool with pages | new files only |
| `extract_text` | The text of PDFs, Office files, Markdown, HTML and images (OCR on Windows) | no |

## Good to know

- **Use absolute paths.** The server resolves relative paths against its own working folder, which depends on the client.
- **Access:** the server can read any file your user account can, and writes new files next to them. It never deletes or overwrites anything and makes no network connections.
- **Windows-only engines** (OCR, Media Foundation audio and video, HEIC/AVIF/RAW, Microsoft Office) aren't available on Linux; `list_formats` and `list_tools` show what this PC can do.
- Tool calls run one at a time, and a crash in an engine fails only that call.
