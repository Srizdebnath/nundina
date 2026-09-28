#!/usr/bin/env python3
"""Every SRS requirement ID cited anywhere in the repo must be defined in
SRS 08 (#63). IDs look like CG-P3, AE-P8, ME-P3, QC-P1, LF-P4, TE-P5. A
reference such as `CG-P8` (undefined) or `LF/P4` (malformed) fails the job.

Definitions are the first cell of SRS 08's requirement tables: `| AE-P8 |`.
"""
import re
import subprocess
import sys
from pathlib import Path

root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
srs = root / "preresearch/architecture/08-SRS-Pilot-Rails.md"
PFX = r"(?:CG|AE|ME|QC|LF|TE)"
defined = set(re.findall(rf"^\| ({PFX}-P\d+) \|", srs.read_text(encoding="utf-8"), re.M))
if not defined:
    sys.exit(f"no requirement definitions found in {srs}")

files = subprocess.check_output(
    ["git", "ls-files", "*.md", "*.rs", "*.ts", "*.tsx", "*.toml", "*.yml"], cwd=root, text=True
).split()
bad = []
for f in files:
    for n, line in enumerate((root / f).read_text(encoding="utf-8", errors="replace").splitlines(), 1):
        for m in re.finditer(rf"\b({PFX})-P(\d+)\b", line):
            if f"{m[1]}-P{m[2]}" not in defined:
                bad.append((f, n, f"undefined requirement ID {m[0]}"))
        for m in re.finditer(rf"\b({PFX})/P(\d+)\b", line):
            bad.append((f, n, f"malformed requirement ID {m[0]} (use {m[1]}-P{m[2]})"))

for f, n, msg in bad:
    print(f"::error file={f},line={n}::{msg}")
print(f"{len(defined)} requirement IDs defined in SRS 08; {len(bad)} bad reference(s)")
sys.exit(1 if bad else 0)
