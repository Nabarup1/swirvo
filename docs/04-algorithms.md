# 04 · Algorithms & data structures

Everything on the interactive path is O(1) or close to it, allocation-free, and precomputed wherever possible. The heavy algorithms run in workers, off the interactive path.

## Summary

| # | Problem | Structure / algorithm | Cost |
|---|---|---|---|
| 1 | Extension → format | Compile-time perfect hash (`phf`, CHD) | O(1) |
| 1 | Content sniffing | Signature trie plus container probes | O(L) with L ≤ 64 B; one read |
| 2 | Wheel targets for k input formats | Precomputed reachability bitsets, AND | O(k · V/64) |
| 2 | Plan for one file | Dijkstra with lexicographic costs and edge guards | O(E log V), ~20 µs |
| 3 | Wedge order | Decayed frecency plus hysteresis slot assignment | O(n log n), n ≤ 64 |
| 4 | Pointer → wedge | Polar coordinates with angular hysteresis | O(1) |
| 5 | Gesture handling | Explicit finite-state machine, property-tested | O(1) per event |
| 6 | Scheduling | Per-level binary heaps, SJF, aging, EASY backfilling | O(log n) + O(w) |
| 7 | Recipes and batches | Kahn's topological execution, cancellation, memoization | O(V + E) |
| 8 | Parallel video encoding | Painter's partition by binary search | O(n log Σ) |
| 9 | Hitting a target size | Illinois false position on log-size | ≤ 6 encodes |
| 10 | ETA and size estimates | EWMA plus Welford variance | O(1) per update |
| 11 | Caches | FileKey identity, W-TinyLFU (memory), byte-bounded LRU (disk) | O(1) amortized |
| 12 | Debounce and timeouts | Hierarchical timing wheel | O(1) insert/cancel |
| 13 | Copy engine | Reflink → in-kernel copy → double-buffered copy | I/O-bound |
| 14 | Batch rename | Template AST; permutation cycle decomposition | O(n) |
| 15 | PDF → editable | Union-find, R-tree, recursive XY-cut, table detection | O(n log n) |
| 16 | Redaction search | Aho-Corasick, DFA regex, checksum validators | O(n + z) |
| 17 | Bleep ranges | Sort plus interval merge | O(n log n) |
| 18 | Lossless trim | Keyframe binary search; smart cut | O(log K) |
| 19 | Fuzzy search | Bitmask prefilter plus Smith-Waterman scoring | O(N) prefilter |
| 20 | Batch progress | Cost-weighted incremental aggregation | O(1) per update |
| 21 | Memory | Arenas, buffer pools, bounded channels, pixel caps | — |

---

## 1. Format detection

The extension gives an instant guess; content sniffing confirms or corrects it.

```text
detect(path):
  guess = EXT_PHF.get(ascii_lowercase(ext))       # compile-time perfect hash, O(1), no allocation
  head  = read(path, 0..4096)                     # one read; a tail read only for ZIP central directory
  m     = SIG_TRIE.longest_match(head)            # byte trie; nodes may carry an offset and wildcards
  match m.family:
    Zip     → probe_zip(head)      # EPUB/ODF: "mimetype" is the first stored entry at offset 30+;
                                   # OOXML: [Content_Types].xml → word/ | xl/ | ppt/ main part
    IsoBmff → probe_ftyp(head)     # brands: heic heix mif1 → HEIC · avif → AVIF · qt → MOV ·
                                   #         isom mp41 mp42 → MP4 · M4A / M4V
    Riff    → fourcc(head[8..12])  # WAVE · AVI · WEBP
    Ebml    → doctype(head)        # "matroska" | "webm"
    Cfb     → cfb_root_names(path) # WordDocument → DOC · Workbook → XLS · PowerPoint Document → PPT
    Text    → sniff_text(head)     # BOM, UTF-8 validity, then JSON / XML / YAML / CSV / SRT / VTT heuristics
  return resolve(guess, m)         # content wins; the extension only breaks ties between equal-confidence matches
```

- **The trie is generated at build time** from a signature table (`build.rs` → static arrays). The root is a dense 256-entry table; deeper nodes use small sorted edge arrays searched by binary search. Cost: a few dozen byte comparisons.
- **The wheel uses the extension guess immediately** (< 1 µs) and refines it when the sniff returns: ≤ 1 ms on a local SSD, asynchronous with a 30 ms timeout on slow or network volumes. A mismatch, such as a `.jpg` that is really a HEIC, updates the wedges in place and shows a small "Actually HEIC" note.

## 2. Conversion graph and planning

**Graph.** Nodes are formats (V ≈ 200). Edges are engine capabilities (E ≈ 3,000), weighted by a cost vector.

```rust
#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
struct Cost { loss: u16, download: u16, time_ms: u32 }   // compared lexicographically (Balanced preset)
```

- `loss`: 0 for remux and lossless steps, otherwise a quality penalty per lossy re-encode. Two lossy hops always cost more than one.
- `download`: a penalty when the edge needs an engine pack that isn't installed, so installed routes win.
- `time_ms`: from the learned cost model (§10), normalized per unit.
- Presets only change the comparator: **Fastest** = (download, time, loss); **Best** = (loss, time, download).

**Precompute** (at startup and whenever packs, hardware or settings change): run Dijkstra (binary heap) from every source format, and keep:
- `REACH[s]`: a bitset of reachable targets, `[u64; 4]` for up to 256 formats;
- `NEXT[s][t]`: the shortest-path tree, so the path to any target is found by walking parents.

That's 200 Dijkstras × ~20 µs ≈ 4 ms off the UI thread. The result is published through `ArcSwap`, so readers never lock.

**Opening the wheel** for files with distinct formats f₁…f_k:

```text
candidates = REACH[f₁] & REACH[f₂] & … & REACH[f_k]       # k ANDs of 4 words
wedges     = rank(candidates, context)                     # §3; at most popcount(candidates) items
```

**Edge guards** depend on the actual file, for example `video → mp3` needs an audio stream, and `png (alpha) → jpg` adds a flatten penalty and the ⚠ badge. Static reachability is computed optimistically, ignoring guards, for the instant wheel. At drop time a guard-aware Dijkstra runs on the probed `MediaInfo` (still microseconds) to produce the real plan. If a guard removes every path, the wedge shows why before you drop, for example *No audio track*.

**Fast paths** are edges with `loss = 0` and tiny time cost (remux, DCT-domain JPEG transforms, PDF object copy, JPEG → JXL recompression), so the planner picks them automatically whenever they're valid ([05](05-conversion-engines.md) §2).

**Explanations:** walking the path produces the hover text and `swirvo plan --explain`:
`HEIC → (decode: libheif, 12 tiles ∥) → RGB8 sRGB → (encode: jpegli q85 4:2:0) → JPG`.

## 3. Ranking with stable slots

**Frecency with exponential decay**, stored as two numbers per key, so there's no history to scan:

```text
on_use(ctx, target, now):
    e = table[ctx][target]
    e.score = e.score · 2^(−(now − e.t_last) / τ) + 1        # τ = 14 days (half-life)
    e.t_last = now
score_now(e, now) = e.score · 2^(−(now − e.t_last) / τ)
rank_score = score_now(fmt ctx) + 0.3 · score_now(category ctx) + prior(target)
```

**Stable slot assignment** protects muscle memory:

1. Pinned slots never move.
2. A target that is already showing keeps its slot.
3. Re-ranking happens at most once per context per day (on the first open after 04:00). An auto slot is swapped only if the challenger's score is more than 1.25 × the occupant's.
4. New targets take the lowest-ranked auto slot; ties go to the slot nearest 12 o'clock.

So positions change at most once a day, and only for a clearly better candidate.

## 4. Radial hit-testing

```text
v = p − c;  d = |v|                         # c = hub center (clamped placement, 02 §3.7)
if   d < r₀                  → Hub (cancel zone)
elif d ≤ R                   → ring 0
elif d ≤ R₂ and subring_open → ring 1
else                         → Outside (exit grace: 48 px for 300 ms)
θ = atan2(v.x, −v.y) mod 2π                 # 0 at 12 o'clock, clockwise
w = 2π / n;  i = ⌊(θ + w/2) / w⌋ mod n
hysteresis: keep the current i while |θ − center(i)| < w/2 + 3°   (no flicker on boundaries)
```

- **Marking-menu flicks:** if the pointer travels more than 0.45 R within 300 ms of opening, in a consistent direction (EMA-smoothed velocity), the direction selects the wedge on release, even if the wheel hasn't finished drawing. Experts don't have to wait for the visuals.
- **Jitter filter** for pen and touch: the One Euro filter (min cutoff 1 Hz, β ≈ 0.007). It smooths hard at low speed and keeps lag low when moving fast.

## 5. Gesture state machine

The states are the ones in [02](02-ux-ui-spec.md) §3.9: `Idle → Probing → Open ⇄ Hover ⇄ SubRing → Committing → Receipt`, plus `Fading` and `KeyboardOpen`. The machine is pure Rust (`crates/wheel`): `fn step(state, event, now) -> (state, Vec<Effect>)`, with no OS calls. Platform layers turn raw events into `Event`s and carry out the `Effect`s (show probe, render, set drop effect).

Property tests (proptest) over random event sequences check that:
- every sequence returns to `Idle` within the timeouts (no stuck overlay);
- `Esc` from any state reaches `Idle` within one event;
- nothing commits without a valid wedge and a non-empty item list;
- Drop always yields exactly one effect, `Accept(Copy)` or `Reject`, and never Move.

## 6. Scheduling

**Queues**, ordered by strict priority with aging:

| Level | Source | Policy |
|---|---|---|
| L0 Interactive | Wheel, context menu, ≤ 20 files | Always first; can pause L1/L2 steps |
| L1 Batch | Hub, CLI, large drops | SJF by estimated cost; FIFO among equals; groups stay together |
| L2 Background | Watch folders | Promoted to L1 after 2 min of waiting; paused on battery saver |

**Shortest job first** within a level minimizes mean completion time, so a quick HEIC jumps ahead of a 2 GB video. The estimate ĉ comes from the perf model (§10).

**Resource vectors.** Each step declares `d = (cpu_threads, ram_mb, gpu_enc, gpu_dec, io_class, soffice_slot)`. Capacity `C` comes from the hardware probe and the current policy (battery saver halves the CPU budget; NVENC sessions are probed, not assumed).

**Admission with EASY backfilling:**

```text
loop on (step finished | step queued | capacity changed):
    h = head of the highest non-empty level
    if d(h) ≤ free: start(h); continue
    shadow = earliest time enough resources free up for h      # from running steps' ETAs
    for s in next w = 16 queued steps (priority order):
        if d(s) ≤ free and now + ĉ(s) ≤ shadow: start(s)       # fills the gap, never delays h
```

**Preemption:** when an L0 step can't fit, the lowest-priority running L1/L2 step is throttled. On Windows that's the job object's CPU rate cap at 5 %; on Linux, `SIGSTOP` or the cgroup freezer. In-process engines pause cooperatively at cancellation-token checkpoints. The step resumes when L0 finishes.

## 7. DAG execution (recipes, multi-step plans, batches)

```text
indeg[v] for every step v;  ready = { v : indeg[v] = 0 }
while ready ∪ running ≠ ∅:
    admit(ready)                                   # §6
    on done(v):   for u in children(v): if --indeg[u] == 0: ready.push(u)
    on failed(v): cancel descendants(v) (DFS); siblings keep going (partial success is reported)
memo key = BLAKE3(op ‖ params ‖ input identities) → reuse the artifact if it's already in the cache
```

Cancellation tokens form a tree: cancelling a batch cancels its jobs, which cancel their steps, which kill their tool processes.

## 8. Parallel chunked video encoding

For slow software encoders (AV1, or HEVC without hardware) on videos longer than 2 minutes, split at scene cuts and encode chunks in parallel, then concatenate them losslessly.

```text
scene segment durations s₁…s_n, k workers
lo = max(s), hi = Σs
while lo < hi:                              # binary search on the answer (painter's partition)
    mid = (lo + hi) / 2
    if greedy_chunks(mid) ≤ k: hi = mid else: lo = mid + 1
→ the smallest possible maximum chunk duration; O(n log Σ)
```

Each chunk starts on a keyframe, so the concat demuxer joins them with stream copy. Speed-up is close to linear up to the number of physical cores. Hardware encoders skip this, since they're already faster than real time.

## 9. Compressing to a target size

**Images.** The function f(q) = log size(q) − log target is roughly linear and monotonic in quality, so use the **Illinois** variant of false position:

```text
bracket [q_lo, q_hi] = [30, 95]; estimate f at both ends on a ¼-area proxy, scaled by the area ratio
repeat ≤ 6:
    q = q_hi − f_hi · (q_hi − q_lo) / (f_hi − f_lo)         # false position
    f = f(q) at full resolution
    if |f| < log 1.03: return q                             # within ±3 %
    replace the end with the same sign as f; if one end is kept twice in a row, halve its f (Illinois)
fallback: integer bisection; if even q_lo is too big, step the size down (2048 → 1600 → 1280) and repeat
```

**Video** has a closed form. `v_bitrate = target_bytes · 8 / duration − a_bitrate − overhead` (overhead ≈ 1.5 %), encoded in two passes, or as VBR with a max rate on hardware encoders. If v_bitrate falls below a quality floor for the resolution (≈ 0.06 bits per pixel per frame), drop to a lower rung (1080 → 720 → 540) and tell the user.

**"Visually lossless" preset:** sample 3–5 segments of 4 s at scene-diverse points, binary-search CRF in [18, 40] until VMAF ≥ 95 on average and ≥ 90 at the 5th percentile, then encode the whole video. That costs about 4–6 sample encodes, and the file is usually 30–60 % smaller than a fixed CRF.

## 10. Estimators (ETA and size before work starts)

- **Throughput:** for each key (engine, op, preset, resolution bucket), an EWMA (α = 0.2) of seconds per unit (megapixel, media second, page), with **Welford** online variance. ETA = units × mean, with an 80 % interval of ±1.28σ, shown as a range when the coefficient of variation exceeds 0.2.
- **Output size:** an EWMA of bytes per megapixel (images) or achieved bitrate (audio and video) per (target, preset, content class), seeded with shipped priors. This produces the hover bubble's "≈ 2.1 MB (−38 %)" with no encoding at all.
- **Content class** comes from cheap features of the thumbnail (color count, edge density, flat-area ratio): *photo*, *graphic/screenshot* or *mixed*. It picks encoder settings ([05](05-conversion-engines.md) §3) and size priors.

## 11. Caches

- **FileKey identity** avoids reading content: `(volume serial, 128-bit file ID)` from `GetFileInformationByHandleEx(FileIdInfo)` on Windows, `(st_dev, st_ino)` on Linux, plus size and mtime. A content hash (BLAKE3, SIMD and multi-threaded) is computed only for dedup, verification or memo keys.
- **Probe cache** in memory: `moka` (W-TinyLFU admission keeps one-off scans from evicting hot entries), 10k entries.
- **Thumbnail cache** on disk: content-addressed files sharded by BLAKE3 prefix, with an SQLite index holding `last_access`. Above the cap, the least recently used 10 % (by bytes) is evicted in one batch, which is O(1) amortized.

## 12. Hierarchical timing wheel

Watch folders produce bursts of file-system events, and every job has timeouts. A 4-level × 64-slot wheel with a 50 ms tick gives O(1) insert and cancel, and costs nothing when idle: the tick timer is armed only while timers exist.

```text
fs event on path p → reset settle timer(p, 750 ms)
timer fires → stable? (size and mtime unchanged, and an exclusive open succeeds on Windows /
                       IN_CLOSE_WRITE seen on Linux) → enqueue; else re-arm
already-processed set keyed by FileKey (persisted) → no double work after a restart
```

## 13. Copy engine (Copy to / Move to, extraction, outputs across volumes)

1. **Clone** where the file system supports it: `ioctl(FICLONE)` on Btrfs/XFS; block cloning on ReFS/Dev Drive (`FSCTL_DUPLICATE_EXTENTS_TO_FILE`; `CopyFile2` does it automatically on recent Windows 11). Instant, with no extra space used.
2. **In-kernel copy**: `copy_file_range` on Linux; `CopyFile2` with `COPY_FILE_NO_BUFFERING` for files over 256 MB on Windows, so large copies don't flush the page cache.
3. **Fallback**: a double-buffered copy with two 4 MiB aligned buffers (read N+1 while writing N).
4. **Optional verification**: BLAKE3 computed while streaming, so the data is never read a second time.

## 14. Batch rename

- A **template language** (`{name}-{n:03}.{ext}`, `{date:%Y-%m-%d}`, `{w}x{h}`, `{lower}`, `{replace:a:b}`) parsed by recursive descent into an AST and evaluated per file. The preview shows a character-level diff (Myers).
- **Conflicts:** build the map old → new. Duplicate targets are an error, shown before anything changes. The rename graph has in-degree and out-degree ≤ 1, so it is a set of **chains and cycles**. Process each chain from its end; break each cycle (a→b, b→a) with one temp name. O(n). Case-only renames on Windows (`photo.JPG → photo.jpg`) go through a temp name too. Every step is journaled for undo.

## 15. PDF → DOCX / XLSX / Markdown: layout reconstruction

> Built as `crates/engines/src/pdflayout` with hayro instead of PDFium and our own DOCX writer instead of `docx-rs`; [13](13-pdf-reconstruction.md) is the current design and supersedes the sketch below.

1. **Extract** glyph runs (Unicode, font, size, bounding box, baseline) and vector rulings with PDFium.
2. **Glyphs → words → lines:** union-find merges glyphs on the same baseline when the gap is < 0.25 × font size. Lines come from sorting by (baseline y, x) and sweeping.
3. **Lines → blocks:** an R-tree (`rstar`) answers neighbor queries. Merge vertically adjacent lines with compatible left edges and leading.
4. **Reading order:** recursive **XY-cut** on projection profiles (O(n log n)). It handles columns, sidebars and headers/footers, which are detected by repeating positions across pages.
5. **Tables:** *lattice* mode builds a cell grid from intersecting rulings (sweep line). *Stream* mode clusters x-alignments into columns (1-D density clustering) and baselines into rows.
6. **Semantics:** headings come from font-size clustering (Jenks natural breaks); lists from bullet and number patterns; images are kept with their positions.
7. **Emit** DOCX (`docx-rs`), XLSX (`rust_xlsxwriter`), Markdown or EPUB. Scanned pages go through OCR first (§16 boxes feed step 2).

## 16. Redaction search

- User word lists: **Aho-Corasick**, O(n + m + z).
- Patterns (email, phone, IBAN, card numbers): the `regex` crate (finite automata, guaranteed linear time, no catastrophic backtracking), followed by **validators** (Luhn for cards, mod-97 for IBAN) to cut false positives.
- Matches map back to glyph quads (PDF) or OCR boxes (images). The boxes on each line are merged (interval merge).
- **True PDF redaction:** remove the content-stream text operators and image pixels inside the boxes, draw opaque rectangles, then strip metadata, annotations and hidden layers in those areas. Finally re-extract the text and assert zero matches. If any remain, the job fails instead of producing a false sense of safety.

## 17. Bleeping words

Whisper word timestamps → intervals [t_s − 30 ms, t_e + 30 ms] → sort and merge overlaps (O(n log n)) → one FFmpeg filter graph that mutes or swaps in a 1 kHz tone, with 5 ms fades so there are no clicks.

## 18. Lossless trim (smart cut)

The packet probe gives a sorted keyframe index. For a cut [a, b]: k₁ = the first keyframe ≥ a, and k₂ = the last keyframe ≤ b (both by binary search, O(log K)). Re-encode only [a, k₁) and [k₂, b) with matching codec parameters, stream-copy [k₁, k₂), and concatenate. The result is frame-exact, nearly instant, and the middle is untouched. If the codec parameters can't be matched (an exotic profile), fall back to a full re-encode and say so.

## 19. Fuzzy search (command bar, *More…*, settings search)

Each candidate stores a 64-bit mask of the ASCII letters and digits it contains. A query passes the prefilter only if `(mask_q & !mask_c) == 0`, so one AND rejects most items. Survivors are scored with `nucleo` (Smith-Waterman-style, with fzf bonuses for word starts and camelCase). That is under 0.2 ms for 2,000 items.

## 20. Batch progress and ETA

Batch progress is (Σ finished cost + Σ partial cost of running steps) / Σ estimated cost, updated incrementally in O(1) per event. The ETA comes from the EWMA of cost units per second. The UI is throttled to 10 Hz; terminal events are delivered immediately.

## 21. Memory discipline

- **mimalloc** as the global allocator in workers (benchmark against the system allocator; it usually wins on Windows).
- **Bump arenas** (`bumpalo`) for per-request scratch in the planner and the IPC decoder; reset after each request.
- **Buffer pools** (`crossbeam::ArrayQueue<Vec<u8>>`) for frame and tile buffers, so the hot loops don't allocate.
- **Bounded channels** (capacity 4) between pipeline stages, so a fast decoder can't outrun a slow encoder and balloon memory.
- **Streaming everywhere:** libvips' demand-driven tiles for huge images, streamed archive entries, Arrow record batches for data files.
- **Caps:** 268 MP pixel limit by default, a maximum archive expansion ratio of 100:1, and per-job RAM caps from the job object or cgroup.
