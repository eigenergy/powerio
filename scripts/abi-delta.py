#!/usr/bin/env python3
"""The classified C ABI delta: every difference between the last published
v5 header (git tag, default v0.9.0) and the working tree header, classified
per symbol, with the ABI number asserted to follow from the classification.

    scripts/abi-delta.py            # print the classified delta, assert necessity
    scripts/abi-delta.py --json     # machine readable form

Classification per symbol: removed, added, signature-changed, or unchanged.
A removal or a signature change makes the surface binary incompatible, so the
gate requires PIO_ABI_VERSION > the baseline's when either class is nonempty.
Ownership and behavior changes that keep a signature are recorded by hand in
the ABI history page; this tool owns the mechanical half.
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADER = "powerio-capi/include/powerio.h"
BASELINE = "v0.9.0"

def declarations(text: str) -> dict[str, str]:
    # Strip comments and collapse whitespace, then take every pio_* function
    # declaration: return type + name + parameter list, up to the semicolon.
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    text = re.sub(r"//[^\n]*", " ", text)
    decls = {}
    for match in re.finditer(
            r"([A-Za-z_][A-Za-z0-9_ *]*?\*?\s*)\b(pio_[a-z0-9_]+)\s*\(([^;{]*)\)\s*;",
            text, re.S):
        ret, name, params = match.groups()
        signature = re.sub(r"\s+", " ", f"{ret.strip()} {name}({params.strip()})").strip()
        decls[name] = signature
    return decls

def abi_version(text: str) -> int:
    match = re.search(r"#define PIO_ABI_VERSION (\d+)", text)
    if not match:
        raise SystemExit("no PIO_ABI_VERSION in header")
    return int(match.group(1))

def main() -> int:
    old_text = subprocess.run(
        ["git", "show", f"{BASELINE}:{HEADER}"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout
    new_text = (ROOT / HEADER).read_text()
    old = declarations(old_text)
    new = declarations(new_text)
    removed = sorted(set(old) - set(new))
    added = sorted(set(new) - set(old))
    changed = sorted(name for name in set(old) & set(new) if old[name] != new[name])
    unchanged = sorted(name for name in set(old) & set(new) if old[name] == new[name])
    old_abi = abi_version(old_text)
    new_abi = abi_version(new_text)

    delta = {
        "baseline": BASELINE,
        "baseline_abi": old_abi,
        "abi": new_abi,
        "removed": removed,
        "added": added,
        "signature_changed": {name: {"was": old[name], "is": new[name]} for name in changed},
        "unchanged_count": len(unchanged),
    }
    if "--json" in sys.argv:
        print(json.dumps(delta, indent=2))
    else:
        print(f"baseline {BASELINE} (ABI {old_abi}) -> working tree (ABI {new_abi})")
        print(f"removed ({len(removed)}):")
        for name in removed:
            print(f"  - {name}")
        print(f"signature changed ({len(changed)}):")
        for name in changed:
            print(f"  ~ {name}")
            print(f"      was: {old[name]}")
            print(f"      is:  {new[name]}")
        print(f"added ({len(added)}):")
        for name in added:
            print(f"  + {name}")
        print(f"unchanged: {len(unchanged)}")

    # The necessity gate: the recorded ABI number must follow from the
    # classified compatibility changes.
    breaking = bool(removed or changed)
    # The migration guide's arithmetic must match this classification: the
    # page states how many symbols the baseline header declared and how many
    # survive unchanged, and a hand edited number drifts.
    guide = ROOT / "docs" / "src" / "abi-v6.md"
    if guide.exists():
        import re as _re
        m = _re.search(r"Of the\s+(\d+) symbols the v0\.9\.0 header declared, (\d+) survive unchanged",
                       guide.read_text())
        if m:
            stated_total, stated_unchanged = int(m.group(1)), int(m.group(2))
            actual_total = len(old)
            if stated_total != actual_total or stated_unchanged != len(unchanged):
                print(f"error: abi-v6.md states {stated_total} baseline symbols with "
                      f"{stated_unchanged} unchanged; the header delta gives "
                      f"{actual_total} with {len(unchanged)}", file=sys.stderr)
                return 1

    if breaking and new_abi <= old_abi:
        print(f"error: the delta removes or re-signatures symbols but the ABI "
              f"stayed at {new_abi}", file=sys.stderr)
        return 1
    if not breaking and new_abi != old_abi:
        print(f"error: the delta is additive yet the ABI moved from {old_abi} "
              f"to {new_abi}; an additive surface keeps its number",
              file=sys.stderr)
        return 1
    if "--json" not in sys.argv:
        print(f"ABI necessity OK: {'breaking' if breaking else 'additive'} delta, "
              f"ABI {old_abi} -> {new_abi}")
    return 0

if __name__ == "__main__":
    sys.exit(main())
