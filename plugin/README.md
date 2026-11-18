# Swirvo plugin

Lets Claude convert files, run PDF and document tools, and read text from documents and images **on your own computer**, using [Swirvo](https://github.com/Nabarup1/swirvo), a free, private file converter and PDF toolkit for Windows.

Ask for things like "convert these PDFs to Word", "merge these three PDFs and add page numbers", "split this PDF into pages 1-3 and 4-9", "read this scanned invoice" or "redact the email addresses in contract.pdf".

## Requirements

Install the Swirvo app on Windows 10 (1809 or later) or 11 first, so the `swirvo` command is available:

- from the Microsoft Store, or
- from the [GitHub releases](https://github.com/Nabarup1/swirvo/releases): unzip, run `Swirvo.exe` and accept the install. This adds `swirvo` to your user PATH.

## What this plugin contains and runs

- `.mcp.json` starts the local Swirvo MCP server with the command `swirvo mcp`. It talks to Claude over standard input and output only.
- `skills/swirvo/SKILL.md` tells Claude which Swirvo tool to use for which task.

The server reads the files you name and writes results as new files next to them. It never changes or deletes your originals, makes no network connections and sends nothing anywhere. Text it returns (for example from `extract_text`) becomes part of your conversation with Claude. See the [privacy policy](https://github.com/Nabarup1/swirvo/blob/main/PRIVACY.md).

The plugin runs where local MCP servers run: Claude Code and Cowork on your computer.

## License

The plugin files are under the [PolyForm Noncommercial License 1.0.0](https://github.com/Nabarup1/swirvo/blob/main/LICENSE.md). The Swirvo app is free to use ([EULA](https://github.com/Nabarup1/swirvo/blob/main/EULA.md)).
