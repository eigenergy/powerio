#!/usr/bin/env python3
"""Check that C ABI doc comments agree with the code they describe.

Three checks, plus one conditional check for the retired Arrow bridge:

1. If an Arrow bridge exists, its PIO_ARROW_TABLE_* selector macros,
   dispatch arms, and catalog rows must name the same id set. If it does not,
   the header must not retain orphaned selector macros.
2. `out_*` names in a function's doc comment must be parameters of that
   function (catches a doc naming a parameter that was renamed away).
3. A doc that says to release something with pio_string_release must sit on
   a declaration that actually hands out a `char *`.
4. Comment lint: unbalanced parentheses in a doc block, an immediately
   repeated word or short phrase (up to three words, so "the handle's the
   handle's" is caught the same way "the the" is), and a Rust module path
   (`](v6::`, `](crate::`) leaking into text that renders verbatim into
   powerio.h.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADER = ROOT / "powerio-capi" / "include" / "powerio.h"
LIB = ROOT / "powerio-capi" / "src" / "lib.rs"
ARROW = ROOT / "powerio-capi" / "src" / "arrow_export.rs"

errors: list[str] = []


def check_arrow_table_ids() -> None:
    header = HEADER.read_text()
    macro_ids = {
        int(m.group(2))
        for m in re.finditer(r"#define PIO_ARROW_TABLE_([A-Z0-9_]+) (\d+)", header)
    }
    if not ARROW.exists():
        if macro_ids:
            errors.append(
                f"the header retains Arrow selector macros {sorted(macro_ids)} "
                "without an Arrow bridge"
            )
        return
    src = ARROW.read_text()
    const_ids = {
        m.group(1): int(m.group(2))
        for m in re.finditer(r"pub const (PIO_ARROW_TABLE_[A-Z0-9_]+): i32 = (\d+);", src)
    }
    dispatch_ids = {
        const_ids[name]
        for name in re.findall(r"^\s*(PIO_ARROW_TABLE_[A-Z0-9_]+) => ", src, flags=re.MULTILINE)
    }
    catalog_ids = {
        const_ids[name]
        for name in re.findall(r"table_spec\(\s*(PIO_ARROW_TABLE_[A-Z0-9_]+)", src)
    }
    if macro_ids != dispatch_ids:
        errors.append(
            f"selector macros {sorted(macro_ids)} != dispatch arms {sorted(dispatch_ids)}"
        )
    if catalog_ids != dispatch_ids:
        errors.append(
            f"catalog ids {sorted(catalog_ids)} != dispatch arms {sorted(dispatch_ids)}"
        )


def doc_blocks(src: str):
    """Yield (comment_lines, following_signature_text) for each /// block."""
    lines = src.splitlines()
    i = 0
    while i < len(lines):
        if lines[i].lstrip().startswith("///"):
            start = i
            while i < len(lines) and lines[i].lstrip().startswith("///"):
                i += 1
            sig_lines = []
            j = i
            depth = 0
            while j < len(lines) and len(sig_lines) < 16:
                line = lines[j]
                j += 1
                if line.lstrip().startswith("#["):
                    continue
                sig_lines.append(line)
                depth += line.count("(") - line.count(")")
                if ")" in line and depth <= 0:
                    break
            yield lines[start:i], "\n".join(sig_lines)
        else:
            i += 1


def check_out_params_and_release_verbs() -> None:
    src = LIB.read_text()
    for comment, sig in doc_blocks(src):
        text = "\n".join(comment)
        m = re.search(r"fn (pio_[a-z0-9_]+)", sig)
        if not m:
            continue
        name = m.group(1)
        params = set(re.findall(r"\b([a-z_][a-z0-9_]*):", sig))
        for token in set(re.findall(r"`(out_[a-z0-9_]+)`", text)):
            if token not in params:
                errors.append(f"{name}: doc names `{token}`, not a parameter of it")
        if "pio_string_release" in text and name != "pio_string_release":
            hands_out_string = (
                "-> *mut c_char" in sig
                or "*mut *mut c_char" in sig
                or re.search(r"char\s*\*", sig) is not None
            )
            if not hands_out_string:
                errors.append(
                    f"{name}: doc directs pio_string_release but the signature hands out no char *"
                )


def check_comment_lint() -> None:
    header = HEADER.read_text()
    block: list[str] = []
    line_no = 0
    for idx, line in enumerate(header.splitlines(), 1):
        stripped = line.strip()
        if stripped.startswith("/**") or stripped.startswith("/*"):
            block = [stripped]
            line_no = idx
            if "*/" in stripped:
                _lint_block(block, line_no)
                block = []
        elif block:
            block.append(stripped)
            if "*/" in stripped:
                _lint_block(block, line_no)
                block = []
    for pat in (r"\]\(v6::", r"\]\(crate::"):
        for m in re.finditer(pat, LIB.read_text()):
            errors.append(f"lib.rs doc link leaks a Rust module path: {m.group(0)!r}")


def _lint_block(block: list[str], line_no: int) -> None:
    text = " ".join(l.lstrip("/* ").rstrip("*/ ") for l in block)
    prose = re.sub(r"`[^`]*`", "", text)
    if prose.count("(") != prose.count(")"):
        errors.append(f"powerio.h:{line_no}: unbalanced parentheses in comment block")
    for m in re.finditer(r"\b(\w[\w']*(?: \w[\w']*){0,2}) \1\b", prose):
        if m.group(1) not in {"had", "that"}:
            errors.append(
                f"powerio.h:{line_no}: repeated phrase {m.group(1)!r} in comment block"
            )


def main() -> int:
    check_arrow_table_ids()
    check_out_params_and_release_verbs()
    check_comment_lint()
    if errors:
        for e in errors:
            print(f"capi-doc-integrity: {e}", file=sys.stderr)
        return 1
    print("C ABI doc comments agree with the code")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
