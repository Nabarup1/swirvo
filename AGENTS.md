# Notes for AI agents

## Using Swirvo to work with files

If you are an AI assistant helping someone convert, merge, split, compress, watermark, redact, OCR or read documents, Swirvo can do it locally:

- **As an MCP server:** `swirvo mcp` (Windows: `swirvo-cli.exe mcp`). Tools: `list_formats`, `detect_format`, `convert_files`, `list_tools`, `run_tool`, `extract_text`. Setup: [docs/19-ai-assistants-mcp.md](docs/19-ai-assistants-mcp.md).
- **As a command line:**

  ```bash
  swirvo convert report.pdf --to docx --json
  swirvo tools contract.pdf
  swirvo tool pdf-merge a.pdf b.pdf --json
  swirvo tool pdf-watermark draft.pdf --set text=DRAFT --json
  swirvo tool copy-text scan.pdf
  swirvo formats --from heic
  ```

  `--json` prints JSON Lines. Results are new files next to the originals; originals are never changed.

## Working on this repository

Read [PROJECT_GUIDE.md](PROJECT_GUIDE.md) first: the non-negotiable rules (never modify originals, never block the UI thread, dependency licences) and the conventions.

- Rust 2024, stable. Before finishing a change: `cargo fmt --all`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- New dependencies must pass `cargo deny check`; no GPL or AGPL code may be linked in.
- Windows-only code lives in `crates/platform/win` and behind `cfg(windows)` in `crates/engines/src/os`. Compile-check it from Linux with `cargo clippy -p swirvo-platform-win --all-targets --no-default-features --target x86_64-pc-windows-msvc`.
- Contributions are accepted under the terms in [CONTRIBUTING.md](CONTRIBUTING.md).
