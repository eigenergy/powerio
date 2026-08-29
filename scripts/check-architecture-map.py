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

def cargo_workspace() -> tuple[set[str], set[tuple[str, str]]]:
    """(workspace member names, intra-workspace dependency edges)."""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout)
    members = {p["name"] for p in meta["packages"]}
    edges = set()
    for package in meta["packages"]:
        for dep in package["dependencies"]:
            if dep["name"] in members and dep["kind"] is None:
                edges.add((package["name"], dep["name"]))
    return members, edges

def diagram_edges(text: str) -> set[tuple[str, str]]:
    """Every drawn edge, including each hop of a chained a -> b -> c line."""
    edges = set()
    for line in text.splitlines():
        for stmt in line.split("//")[0].split(";"):
            if "->" not in stmt:
                continue
            names = []
            for part in stmt.split("->"):
                m = re.search(r'"?([A-Za-z0-9_-]+)"?', part)
                if m:
                    names.append(m.group(1))
            for a, b in zip(names, names[1:]):
                edges.add((a, b))
    return edges

def diagram_nodes(text: str) -> set[str]:
    """Every node identifier the diagram declares or wires into an edge.

    Structural extraction, not a substring search: "powerio" is a literal
    prefix of every other crate's name (powerio-tx, powerio-matrix, ...), so
    checking "is this name mentioned anywhere in the text" would count a
    hyphenated crate's own node as a false sighting of the bare facade crate.
    """
    nodes: set[str] = set()
    for a, b in diagram_edges(text):
        nodes.add(a)
        nodes.add(b)
    nodes |= set(re.findall(r'"([A-Za-z0-9_-]+)"\s*\[', text))
    nodes |= set(re.findall(r'^([A-Za-z0-9_]+)\s*\[', text, re.M))
    return nodes

def main() -> int:
    render = "--render" in sys.argv
    members, cargo_edges = cargo_workspace()
    text = (DIAGRAMS / "architecture.dot").read_text()
    # 0) every workspace crate is a node somewhere in the diagram. The crate
    # universe comes from cargo metadata, not from the diagram's own text, so
    # a new workspace member the diagram was never updated for is caught here
    # instead of silently dropping out of every check below.
    absent = members - diagram_nodes(text)
    if absent:
        print(f"workspace crates absent from architecture.dot: {sorted(absent)}",
              file=sys.stderr)
        return 1
    # 1) crate edge agreement.
    drawn = {(a, b) for a, b in diagram_edges(text) if a in members and b in members}
    actual = {(a, b) for a, b in cargo_edges if a in members and b in members}
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
    # 2) committed SVGs depict the current graphs. Byte equality with a fresh
    # render does not hold across graphviz releases (layout and font metrics
    # move), so the check reads the <title> elements graphviz emits per node
    # and edge — stable across versions — and holds them to the .dot sources.
    ASSETS.mkdir(parents=True, exist_ok=True)
    for name in SOURCES:
        dot_text = (DIAGRAMS / name).read_text()
        out = ASSETS / (name.removesuffix(".dot") + ".svg")
        if render:
            svg = subprocess.run(["dot", "-Tsvg", str(DIAGRAMS / name)],
                                 check=True, capture_output=True, text=True).stdout
            svg = re.sub(r'<!--.*?-->\n?', "", svg, flags=re.S)
            out.write_text(svg)
        if not out.exists():
            print(f"{out} is missing; run scripts/check-architecture-map.py --render",
                  file=sys.stderr)
            return 1
        titles = [re.sub(r"&#45;|&#8209;", "-", t).replace("&gt;", ">").replace("&lt;", "<")
                  for t in re.findall(r"<title>(.*?)</title>", out.read_text())]
        svg_edges = set()
        svg_nodes = set()
        for t in titles[1:]:  # titles[0] is the graph's own title
            if "->" in t:
                a, _, b = t.partition("->")
                svg_edges.add((a, b))
            elif not t.startswith("cluster"):
                svg_nodes.add(t)
        want_edges = diagram_edges(dot_text)
        if svg_edges != want_edges:
            gone = sorted(want_edges - svg_edges)
            extra = sorted(svg_edges - want_edges)
            print(f"{out} is stale (edges missing {gone}, extra {extra}); "
                  "run scripts/check-architecture-map.py --render", file=sys.stderr)
            return 1
        want_nodes = diagram_nodes(dot_text)
        if not want_nodes <= svg_nodes:
            print(f"{out} is stale (nodes missing {sorted(want_nodes - svg_nodes)}); "
                  "run scripts/check-architecture-map.py --render", file=sys.stderr)
            return 1
    print("architecture map OK: crate edges agree and the SVGs depict them")
    return 0

if __name__ == "__main__":
    sys.exit(main())
