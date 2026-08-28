#!/usr/bin/env python3
"""The architecture map gate: the drawn crate edges must equal the cargo
metadata edges, and the committed SVGs must be exactly what the checked in
diagram sources generate.

Modes:
  check-architecture-map.py            validate + assert the SVGs are current
  check-architecture-map.py --render   validate + rewrite the SVGs
"""
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DIAGRAMS = ROOT / "docs" / "diagrams"
ASSETS = ROOT / "docs" / "src" / "assets"
SOURCES = ["architecture.dot", "dataflow.dot"]

def cargo_edges() -> set[tuple[str, str]]:
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    members = {p["name"] for p in meta["packages"]}
    edges = set()
    for package in meta["packages"]:
        for dep in package["dependencies"]:
            if dep["name"] in members and dep["kind"] is None:
                edges.add((package["name"], dep["name"]))
    return edges

def diagram_edges(text: str) -> set[tuple[str, str]]:
    edges = set()
    for match in re.finditer(r'^\s*"?([A-Za-z0-9_-]+)"?\s*->\s*"?([A-Za-z0-9_-]+)"?', text, re.M):
        edges.add((match.group(1), match.group(2)))
    return edges

def main() -> int:
    render = "--render" in sys.argv
    # 1) crate edge agreement, for the diagram nodes that name crates.
    text = (DIAGRAMS / "architecture.dot").read_text()
    crates = {name for name in re.findall(r'"(powerio[a-z-]*)"', text)} | {"powerio"}
    drawn = {(a, b) for a, b in diagram_edges(text)
             if (a in crates or a == "powerio") and (b in crates or b == "powerio")}
    actual = {(a, b) for a, b in cargo_edges() if a in crates and b in crates}
    # powerio-py and powerio-capi depend on more than the facade in their
    # manifests (direct component deps for feature wiring); the map shows the
    # facade edge alone. Require every drawn edge to be real, and every
    # facade-family edge to be drawn.
    missing = {e for e in drawn - actual}
    if missing:
        print(f"architecture.dot draws edges cargo metadata does not have: {sorted(missing)}",
              file=sys.stderr)
        return 1
    core_edges = {e for e in actual
                  if e[0] in {"powerio", "powerio-tx", "powerio-dist", "powerio-prob",
                              "powerio-matrix"}
                  and e[1] in {"powerio-core", "powerio-tx", "powerio-dist",
                               "powerio-prob", "powerio-matrix"}}
    undrawn = core_edges - drawn
    if undrawn:
        print(f"cargo metadata has component edges architecture.dot does not draw: {sorted(undrawn)}",
              file=sys.stderr)
        return 1
    # 2) generated SVGs current.
    ASSETS.mkdir(parents=True, exist_ok=True)
    for name in SOURCES:
        svg = subprocess.run(["dot", "-Tsvg", str(DIAGRAMS / name)],
                             check=True, capture_output=True, text=True).stdout
        # Strip the generator comment lines dot stamps with its version, so
        # the check is deterministic across graphviz releases.
        svg = re.sub(r'<!--.*?-->\n?', "", svg, flags=re.S)
        out = ASSETS / (name.removesuffix(".dot") + ".svg")
        if render:
            out.write_text(svg)
        elif not out.exists() or out.read_text() != svg:
            print(f"{out} is stale; run scripts/check-architecture-map.py --render",
                  file=sys.stderr)
            return 1
    print("architecture map OK: crate edges agree and the SVGs are current")
    return 0

if __name__ == "__main__":
    sys.exit(main())
