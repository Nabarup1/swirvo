# The Swirvo website

The product site, downloads and docs at **[swirvo.vercel.app](https://swirvo.vercel.app)**. Static pages built with [Astro](https://astro.build); no server code, no cookies, no analytics, no third-party requests.

## Run it locally

Node 22.23 or newer (Node 26 LTS works).

```bash
cd website
npm install
npm run dev        # http://localhost:4321, reloads as you edit
npm run build      # the static site in dist/
npm run preview    # serve dist/ at http://localhost:4321
```

`npm run dev` and `npm run build` first run `scripts/sync.mjs`, which copies from the repository root what the site shows: the changelog, the legal texts, `install.sh`, `llms.txt`, screenshots and the demo video. The copies are committed too, so the site also builds on its own. At build time it reads the latest GitHub release once (file sizes, checksums), so visitors' browsers never call GitHub.

## Deploy (Vercel)

Import the GitHub repository in Vercel with **Root Directory: `website`**. Everything else comes from `vercel.json` (Astro, `npm run build`, `dist`, clean URLs, security headers). Every push to `main` then deploys. Set `SITE_URL` in the project's environment variables if the domain changes.

## What's where

| Path | What |
|---|---|
| `src/pages/` | Home, download, formats, tools, AI, changelog, legal pages, 404; `docs/[...slug].astro` renders the docs |
| `src/content/docs/` | The documentation, one Markdown file per page (`section` and `order` place it in the sidebar) |
| `src/components/home/` | The homepage sections: hero, live demo, wheels, Board, formats, toolkit, privacy, engines, AI, CLI, video, FAQ |
| `src/data/` | Facts the pages draw on: links, all 59 tools (from `crates/jobs/src/tools.rs`), the format graph (from docs/10), the demo's files |
| `src/lib/logo.ts` | The Swirvo mark, ported from `crates/render/src/icon.rs` so the site draws exactly the app's icon |
| `src/scripts/` | Motion (springs, reveals, magnetic buttons, page transitions), the cursor, the wheel demo, the 3D hero, search |
| `src/styles/` | Design tokens (colour, type, spring easings), base styles, the wheel |

## Design notes

- **Palette:** "ink and ember". The app icon's gradient (`#F79A45` → `#D9482B`) is the only loud colour; dark ink or warm paper everywhere else. Light and dark themes, plus System.
- **Type:** Bricolage Grotesque (headlines), Instrument Sans (text), Instrument Serif (emphasis), JetBrains Mono (code). All SIL Open Font License, served from this site.
- **Motion:** real spring physics (`Spring` in `motion.ts`, and CSS `linear()` curves generated from a damped oscillator) for presses, drags, menus, tabs and the wheel. Everything respects `prefers-reduced-motion`.
- **The wheel demo** uses the app's own geometry, captions and tool labels, so it looks and behaves like the real thing, including touch (hold, drag) and keyboard (focus a file, tap Shift, arrows, Enter).

## Credits

Open-source code and fonts the site ships are listed with their licences in `public/credits.txt` (served at `/credits.txt`).
