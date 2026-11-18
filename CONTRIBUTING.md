# Contributing to Swirvo

Thanks for helping. Bug reports, format requests and pull requests are all welcome.

## Before you open a pull request

**By submitting a contribution (code, docs, images or anything else) you agree that:**

1. It is your own work, or you have the right to submit it.
2. You grant **Nabarup** a perpetual, worldwide, non-exclusive, royalty-free, irrevocable licence to use, copy, modify, distribute, sublicense and relicense your contribution, under [LICENSE.md](LICENSE.md) or **any other terms, including commercial ones**.
3. You keep the copyright in your contribution and may use it elsewhere yourself.

This keeps the whole project under one owner, so it can be relicensed or offered under other terms later without tracking down every contributor. The pull-request template asks you to confirm it.

## Bugs and requests

- Open an [issue](https://github.com/Nabarup1/swirvo/issues). Say which version, which Windows version, and what you did.
- **Don't attach personal or confidential files.** Issues are public. If a specific file breaks Swirvo, try to reproduce it with a file you made yourself.
- Security problems: see [SECURITY.md](SECURITY.md), not public issues.

## Working on the code

- Read [PROJECT_GUIDE.md](PROJECT_GUIDE.md) first: the rules (never touch originals, never block the UI thread, licences of dependencies) and the conventions.
- Rust 2024, stable toolchain. Before you push:

  ```bash
  cargo fmt --all
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo test --workspace
  ```

- New dependencies must pass `cargo deny check`. No GPL or AGPL code may be linked in.
- Keep pull requests focused: one fix or feature each, with a test where it makes sense.
