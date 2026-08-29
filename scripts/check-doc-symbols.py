#!/usr/bin/env python3
"""The documentation symbol gate: names taught by active guide pages must
exist on the surfaces that ship. A rendered page with a dead symbol fails.

Checked shapes:
- C symbols (`pio_*`) in active pages exist in the installed header; history
  pages (the ABI and migration set) may name removed ones.
- CLI invocations in shell fences use real subcommands.
- Julia identifiers in julia fences that PowerIO.jl exports; needs POWERIO_JL
  naming a checkout, else skipped.
- Python `powerio.<attr>` references exist in the package's __init__.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs" / "src"
HISTORY_PAGES = {
    "abi-v5.md", "abi-v6.md", "capi-arrow.md", "migration.md", "migration-v1.md",
    "migration-v0.9.md", "migration-v0.7.md", "retired-names.md", "developer.md",
}
# Pages whose lower half narrates retired surfaces: everything above the
# heading is live prose, checked like any other page.
HISTORY_BELOW = {
    "pio-json-schema.md": "## The 0.9 package",
}

def fail(page: Path, message: str) -> None:
    print(f"{page.relative_to(ROOT)}: {message}", file=sys.stderr)
    global failed
    failed = True

failed = False

header = (ROOT / "powerio-capi/include/powerio.h").read_text()
c_symbols = set(re.findall(r"\b(pio_[a-z0-9_]+)\s*\(", header))
c_symbols |= set(re.findall(r"#define (PIO_[A-Z0-9_]+)", header))

cli_main = (ROOT / "powerio-cli/src/main.rs").read_text()
cli_commands = set()
for match in re.finditer(r"^    ([A-Z][A-Za-z0-9]+) \{", cli_main, re.M):
    name = match.group(1)
    cli_commands.add(re.sub(r"(?<!^)([A-Z])", r"-\1", name).lower())
cli_commands |= {"help", "tui"}
for match in re.finditer(r'visible_alias = "([a-z-]+)"', cli_main):
    cli_commands.add(match.group(1))
for match in re.finditer(r'#\[command\(name = "([a-z-]+)"', cli_main):
    cli_commands.add(match.group(1))

python_init = (ROOT / "python/powerio/__init__.py").read_text()

julia_exports: set[str] = set()
julia_root = Path(__import__("os").environ.get("POWERIO_JL", ""))
if julia_root and (julia_root / "src/PowerIO.jl").exists():
    text = (julia_root / "src/PowerIO.jl").read_text()
    for match in re.finditer(r"^export (.+?)$", text, re.M):
        julia_exports |= {n.strip().rstrip(",") for n in match.group(1).split(",") if n.strip()}
    # continuation lines of export statements
    for match in re.finditer(r"^export [^\n]*(?:\n\s+[^\n]+)*", text, re.M):
        for name in re.findall(r"[A-Za-z_][A-Za-z0-9_!]*", match.group(0)):
            if name != "export":
                julia_exports.add(name)

PAGES = sorted(DOCS.glob("*.md")) + [ROOT / "README.md"]

for page in PAGES:
    text = page.read_text()
    if page.name in HISTORY_BELOW:
        text = text.split(HISTORY_BELOW[page.name])[0]
    is_history = page.name in HISTORY_PAGES
    # C symbols anywhere in the page's prose or code. A family shorthand
    # (`pio_dc_data_*`) is prose, not a symbol; the star and the trailing
    # underscore are excluded.
    for symbol in set(re.findall(r"\bpio_[a-z0-9_]*[a-z0-9](?!\*)\b", text)):
        if symbol not in c_symbols and not is_history:
            fail(page, f"names C symbol {symbol}, absent from powerio.h")
    # CLI subcommands in sh fences.
    if not is_history:
        for fence in re.findall(r"```sh\n(.*?)```", text, re.S):
            for line in fence.splitlines():
                match = re.match(r"\s*powerio\s+([a-z-]+)", line)
                if match and match.group(1) not in cli_commands:
                    fail(page, f"teaches CLI subcommand `powerio {match.group(1)}`, absent from the binary")
    # Julia identifiers taught in julia fences: calls of exported-looking names.
    if julia_exports:
        for fence in re.findall(r"```julia\n(.*?)```", text, re.S):
            for name in set(re.findall(r"(?<![.\w])([a-z][a-z_0-9]*[a-z0-9]!?)\(", fence)):
                if name in {"println", "print", "read", "readdir", "joinpath", "string",
                            "length", "first", "filter", "endswith", "typeof", "show",
                            "get", "sort", "collect", "pairs", "String"}:
                    continue
                if name not in julia_exports and not is_history:
                    fail(page, f"julia example calls {name}(), not exported by PowerIO.jl")
    # Python attribute references.
    for attr in set(re.findall(r"\bpowerio\.([a-z_A-Z][A-Za-z_0-9]*)", text)):
        if attr in {"dev", "h", "versions", "dcopf"}:
            continue
        if (ROOT / "python/powerio" / attr).is_dir():
            continue
        if is_history:
            continue
        if f"def {attr}" not in python_init and f'"{attr}"' not in python_init \
                and f"class {attr}" not in python_init and f"{attr} =" not in python_init:
            fail(page, f"names python attribute powerio.{attr}, absent from the package")

# The cross language operation table: every cell of languages.md must name
# live symbols in its column's surface. The global passes above already cover
# the C column and `powerio.<attr>`; this pass reads the other columns.
rust_items: set[str] = set()
for crate in ("powerio", "powerio-core", "powerio-tx", "powerio-dist",
              "powerio-matrix", "powerio-prob"):
    for path in (ROOT / crate / "src").rglob("*.rs"):
        text = path.read_text()
        rust_items |= set(re.findall(
            r"pub(?:\([^)]*\))? (?:async )?(?:unsafe )?fn ([A-Za-z_][A-Za-z0-9_]*)", text))
        rust_items |= set(re.findall(
            r"pub(?:\([^)]*\))? (?:struct|enum|trait|mod|type) ([A-Za-z_][A-Za-z0-9_]*)", text))

python_defs = python_init + (ROOT / "python/powerio/__init__.pyi").read_text() \
    + (ROOT / "python/powerio/_powerio.pyi").read_text() \
    + (ROOT / "python/powerio/dist.py").read_text()

lang_page = DOCS / "languages.md"
RECEIVERS = {"module", "m", "net", "case", "path", "bytes", "name", "fmt",
             "T", "self", "stored", "transform", "select", "time",
             "Err", "NULL", "Cargo"}

def cell_fragments(cell: str) -> list[str]:
    return re.findall(r"`([^`]+)`", cell)

for line in lang_page.read_text().splitlines():
    if not line.startswith("|") or line.startswith("|---") or "| Concept |" in line:
        continue
    cells = [c.strip() for c in line.strip("|").split("|")]
    if len(cells) != 5:
        continue
    concept, rust_cell, python_cell, julia_cell, _c_cell = cells
    for fragment in cell_fragments(rust_cell):
        # A path names its final item; crate and module prefixes are not
        # themselves the taught symbol.
        fragment = re.sub(r"(?:[A-Za-z_][A-Za-z0-9_]*::)+", "", fragment)
        names = set(re.findall(r"([A-Za-z_][A-Za-z0-9_]*)\s*\(", fragment))
        names |= set(re.findall(r"\b([A-Z][A-Za-z0-9_]+)\b", fragment))
        for token in names - RECEIVERS:
            if token not in rust_items:
                fail(lang_page, f"operation table ({concept}): Rust names `{token}`, not a public item")
    for fragment in cell_fragments(python_cell):
        names = set(re.findall(r"\.([a-z_][a-z0-9_]*)\s*\(", fragment))
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", fragment):
            names.add(fragment)
        for token in names - RECEIVERS:
            if (f"def {token}" not in python_defs and f"class {token}" not in python_defs
                    and f"{token} =" not in python_defs and f'"{token}"' not in python_defs):
                fail(lang_page, f"operation table ({concept}): Python names `{token}`, absent from the package")
    if julia_exports:
        for fragment in cell_fragments(julia_cell):
            names = set(re.findall(r"(?<![.\w])([a-z][a-z0-9_]*!?)\s*\(", fragment))
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_!]*", fragment):
                names.add(fragment.rstrip("!"))
            for token in names - RECEIVERS:
                if token not in julia_exports:
                    fail(lang_page, f"operation table ({concept}): Julia names `{token}`, not exported by PowerIO.jl")

if failed:
    sys.exit(1)
print("documentation symbols OK")
