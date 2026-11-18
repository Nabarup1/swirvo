#!/usr/bin/env python3
"""Write THIRD-PARTY-NOTICES.txt for the binaries we ship.

MIT, BSD, ISC, Zlib and Apache licences all ask that the copyright notice and licence text
travel with binary copies. This walks every crate compiled into the Windows binaries (normal
dependencies only; build tools and proc-macros don't ship), and copies each crate's licence
files, including licences of assets bundled inside crates (hayro's Foxit fonts and Adobe
CMaps, for example).

Usage: scripts/third-party-notices.py [OUT] [--target TRIPLE]
Default OUT is dist/THIRD-PARTY-NOTICES.txt; default target x86_64-pc-windows-msvc.
Standard library only; runs `cargo metadata --offline`.
"""

import json
import pathlib
import subprocess
import sys

ROOTS = ["swirvo-platform-win", "swirvo-cli"]
LICENSE_PREFIXES = ("license", "licence", "copying", "notice", "copyright", "unlicense")
SKIP_DIRS = {"tests", "test", "target", "benches", "examples", ".git", "fuzz"}

repo = pathlib.Path(__file__).resolve().parent.parent
args = [a for a in sys.argv[1:] if not a.startswith("--")]
target = "x86_64-pc-windows-msvc"
if "--target" in sys.argv:
    target = sys.argv[sys.argv.index("--target") + 1]
    args = [a for a in args if a != target]
out = pathlib.Path(args[0]) if args else repo / "dist" / "THIRD-PARTY-NOTICES.txt"

meta = json.loads(
    subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--offline", "--filter-platform", target],
        cwd=repo,
    )
)
pkgs = {p["id"]: p for p in meta["packages"]}
nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

# Walk normal dependencies from the shipped binaries.
seen = set()
stack = [i for i, p in pkgs.items() if p["name"] in ROOTS]
while stack:
    i = stack.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        if any(k["kind"] is None for k in d["dep_kinds"]):
            stack.append(d["pkg"])

shipped = sorted(
    (pkgs[i] for i in seen if pkgs[i]["source"] is not None),  # path crates are our own
    key=lambda p: (p["name"].lower(), p["version"]),
)


def license_files(root: pathlib.Path):
    found = []
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root)
        if any(part.lower() in SKIP_DIRS for part in rel.parts[:-1]) or len(rel.parts) > 4:
            continue
        name = path.name.lower()
        if path.is_file() and name.startswith(LICENSE_PREFIXES) and path.stat().st_size < 200_000:
            found.append(path)
    return found


# Unique licence texts -> the components that use them (Apache-2.0 alone appears hundreds of
# times verbatim), so each text is printed once.
texts: dict[str, list[str]] = {}
# SPDX id -> a full text, for crates that ship none. Short templates are built in; longer
# ones (Apache-2.0, MPL-2.0, MIT) are taken from crates that do ship them.
canonical: dict[str, str] = {
    "BSD-3-Clause": """Copyright (c) the authors listed above. All rights reserved.

Redistribution and use in source and binary forms, with or without modification, are
permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this list of
   conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice, this list of
   conditions and the following disclaimer in the documentation and/or other materials
   provided with the distribution.
3. Neither the name of the copyright holder nor the names of its contributors may be used to
   endorse or promote products derived from this software without specific prior written
   permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND ANY EXPRESS
OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL,
EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE
GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING
NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED
OF THE POSSIBILITY OF SUCH DAMAGE.""",
    "Zlib": """Copyright (c) the authors listed above.

This software is provided 'as-is', without any express or implied warranty. In no event will
the authors be held liable for any damages arising from the use of this software.

Permission is granted to anyone to use this software for any purpose, including commercial
applications, and to alter it and redistribute it freely, subject to the following
restrictions:

1. The origin of this software must not be misrepresented; you must not claim that you wrote
   the original software. If you use this software in a product, an acknowledgment in the
   product documentation would be appreciated but is not required.
2. Altered source versions must be plainly marked as such, and must not be misrepresented as
   being the original software.
3. This notice may not be removed or altered from any source distribution.""",
}
missing = []
rows = []
for p in shipped:
    root = pathlib.Path(p["manifest_path"]).parent
    files = license_files(root)
    who = f"{p['name']} {p['version']}"
    rows.append((p, bool(files)))
    if not files:
        missing.append(p)
    for f in files:
        body = f.read_text(encoding="utf-8", errors="replace").strip()
        texts.setdefault(body, []).append(f"{who} ({f.relative_to(root)})")
        low = f.name.lower()
        if "apache" in low or body.startswith("Apache License"):
            canonical.setdefault("Apache-2.0", body)
        if "Mozilla Public License Version 2.0" in body[:200]:
            canonical.setdefault("MPL-2.0", body)
        if low.endswith("mit") or low.endswith("mit.txt"):
            canonical.setdefault("MIT", body)

lines = [
    "THIRD-PARTY SOFTWARE NOTICES",
    "",
    "This product includes the open-source components listed below, each used under its",
    "licence. Licence texts follow the list; each text names the components it covers.",
    "Our own code is not covered by these notices.",
    "",
    f"Components ({len(shipped)}):",
    "",
]
for p, has in rows:
    src = p.get("repository") or "crates.io"
    lines.append(f"  {p['name']} {p['version']}  [{p.get('license') or 'see texts'}]  {src}")
lines += ["", "=" * 78, "LICENCE TEXTS", "=" * 78, ""]
for body, users in sorted(texts.items(), key=lambda kv: kv[1][0].lower()):
    lines.append("Used by: " + "; ".join(users))
    lines.append("-" * 78)
    lines.append(body)
    lines += ["", "=" * 78, ""]
if missing:
    lines.append("Components that ship no licence file, under the standard texts below:")
    for p in missing:
        lines.append(f"  {p['name']} {p['version']}  [{p.get('license')}]  authors: "
                     + ", ".join(p.get("authors") or ["the authors"]))
    lines.append("")
    needed = set()
    for p in missing:
        for part in (p.get("license") or "").replace("(", " ").replace(")", " ").split():
            if part not in ("OR", "AND", "WITH"):
                needed.add(part)
    for spdx in sorted(needed):
        lines.append(f"--- {spdx} (standard text) ---")
        lines.append(canonical.get(spdx, f"See https://spdx.org/licenses/{spdx}.html"))
        lines += ["", ""]

out.parent.mkdir(parents=True, exist_ok=True)
out.write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"{out}: {len(shipped)} components, {len(texts)} distinct texts, "
      f"{out.stat().st_size // 1024} KB")
