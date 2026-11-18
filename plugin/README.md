# Swirvo plugin

Lets Claude convert files, run PDF and document tools, and read text from documents and images **on your own computer**, using [Swirvo](https://github.com/Nabarup1/swirvo), a free, private file converter and PDF toolkit for Windows.

Ask for things like "convert these PDFs to Word", "merge these three PDFs and add page numbers", "split this PDF into pages 1-3 and 4-9", "read this scanned invoice" or "redact the email addresses in contract.pdf".

## Requirements

Install the Swirvo app on Windows 10 (1809 or later) or 11 first, so the `swirvo` command is available:

- from the Microsoft Store, or
- from the [GitHub releases](https://github.com/Nabarup1/swirvo/releases): unzip, run `Swirvo.exe` and accept the install. This adds `swirvo` to your user PATH.

## What this plugin contains and runs

- `skills/swirvo/SKILL.md` tells Claude how to use the `swirvo` command line (convert, tools, read text) and which tool fits which task. Claude runs those commands in your terminal, so you see and approve each one.
- `.claude-plugin/icon.png` is the Swirvo icon.

The plugin itself runs nothing on its own and starts no server. The `swirvo` command reads the files you name and writes results as new files next to them; it never changes or deletes your originals, makes no network connections and sends nothing anywhere. Text it prints (for example from `copy-text`) becomes part of your conversation with Claude. See the [privacy policy](https://github.com/Nabarup1/swirvo/blob/main/PRIVACY.md).

The skill works where Claude can run terminal commands: Claude Code, and Cowork on your computer. To give other AI apps Swirvo's tools directly, add its MCP server by hand: see [docs/19](https://github.com/Nabarup1/swirvo/blob/main/docs/19-ai-assistants-mcp.md).

## License

The plugin files are under the [PolyForm Noncommercial License 1.0.0](https://github.com/Nabarup1/swirvo/blob/main/LICENSE.md). The Swirvo app is free to use ([EULA](https://github.com/Nabarup1/swirvo/blob/main/EULA.md)).
