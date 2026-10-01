#!/usr/bin/env python3
"""Every path in the README repo map, and every relative link in README.md,
must exist on disk. The list is derived from the README itself, so a new map
entry is checked without anyone editing this script (#54).

Entries that are gitignored by design (e.g. token/keys/) are skipped: they
never exist in a checkout.
"""
import re
import subprocess
import sys
from pathlib import Path

root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
readme = (root / "README.md").read_text(encoding="utf-8")

paths = set()

# 1. The tree block under "## Repo structure".
m = re.search(r"^## Repo structure\s*\n+```[^\n]*\n(.*?)^```", readme, re.S | re.M)
if not m:
    sys.exit("README.md: no '## Repo structure' code block found")
tree_paths = 0
stack = []  # dir paths, one per depth
for line in m.group(1).splitlines():
    t = re.match(r"^((?:│   |    )*)(?:├── |└── )(\S+)", line)
    if not t:
        continue
    depth = len(t.group(1)) // 4
    name = t.group(2)
    stack = stack[:depth]
    path = (stack[-1] if stack else "") + name
    paths.add(path.rstrip("/"))
    tree_paths += 1
    if name.endswith("/"):
        stack.append(path)

# A reformatted tree (different box-drawing, tabs) would match nothing and
# leave only the link check below, silently (#98). The map has 20+ entries;
# fewer parsed means the parser, not the repo, changed.
MIN_TREE_PATHS = 20
if tree_paths < MIN_TREE_PATHS:
    sys.exit(f"::error file=README.md::parsed only {tree_paths} repo-map tree entries "
             f"(want >= {MIN_TREE_PATHS}): the tree format changed; fix the regex in {__file__}")

# 2. Relative Markdown links anywhere in README.md.
for link in re.findall(r"\]\(([^)#\s]+)(?:#[^)]*)?\)", readme):
    if not re.match(r"^[a-z]+:", link):
        paths.add(link.rstrip("/"))

missing = []
for p in sorted(paths):
    if (root / p).exists():
        continue
    # Probe a child too: a `dir/` ignore rule only matches a path known to be a directory.
    probes = [p, p + "/.probe"]
    if any(subprocess.run(["git", "check-ignore", "-q", "--no-index", q], cwd=root).returncode == 0 for q in probes):
        print(f"skip (gitignored by design): {p}")
        continue
    missing.append(p)

for p in missing:
    print(f"::error file=README.md::README maps a missing path: {p}")
print(f"checked {len(paths)} README paths ({tree_paths} from the tree), {len(missing)} missing")
sys.exit(1 if missing else 0)
