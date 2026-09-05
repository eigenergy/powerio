"""powerio's native multiconductor nodal admittance versus an assembly of
OpenDSS's own element primitive admittances (CktElement.YPrim), the
authoritative per element matrix OpenDSS itself stamps from.

Only the passive classes PowerIO stamps into G + jB are assembled here: Line
(non switch), Capacitor and Reactor. The helper explicitly removes transformers
from its local copy before building this passive projection. Transformer buses,
voltage-source buses and every other active or controlled element's buses are
excluded on both sides. This comparison does not establish transformer physics
support. The separate core-shunt comparison checks the proposed winding-local
admittances against OpenDSS. PowerIO's node resolution folds closed switch merges
before comparison, so both sides use the same electrical nodes.

Caveat that motivated building this leg on YPrim instead of the more obvious
YMatrix.getYsparse() (OpenDSS's assembled system Y, "SystemY"): on
ieee34Mod1.dss, getYsparse omits the line shunt capacitance that Line.l5's
own YPrim includes (hand computed 0.14756835687291797 - 0.11686574283071076j
for the bus 812 block; powerio matches it to 5.7e-17, SystemY misses it by
the full shunt). SystemY also folds in the Vsource Thevenin admittance and
the loads' linearized admittance at the solved operating point, both of
which powerio deliberately excludes. YPrim, read per element, has neither
problem, which is why it is the oracle for the main comparison below.

One SystemY cross-check remains valid and is kept as a second, independent
leg: on fourwire_linecode.dss, the sourcebus-loadbus off-diagonal block is
reachable by exactly one element, Line.l1 (the loads sit entirely within
loadbus and the source's Thevenin admittance sits entirely within
sourcebus), so SystemY and powerio's passive admittance must agree there
regardless of what else SystemY folds onto the diagonal blocks.

Fixtures: every `tests/data/dist/micro/*.dss` deck plus the three IEEE test
decks (13, 34, 123 bus), discovered from the repository tree at run time
rather than a fixed path list, so this leg tracks whatever the fixture
directories hold. A deck with zero nodes left to compare after excluding
every non passive element is counted, never marked pass or fail: it asserts
nothing about correctness, only that the exclusion logic ran.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import opendssdirect as dss
import scipy.sparse as sp

REPO_ROOT = Path(__file__).resolve().parents[2]
MICRO_DIR = REPO_ROOT / "tests/data/dist/micro"
IEEE_DECKS = [
    REPO_ROOT / "tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss",
    REPO_ROOT / "tests/data/dist/opendss/ieee34/ieee34Mod1.dss",
    REPO_ROOT / "tests/data/dist/opendss/ieee123/IEEE123Master.dss",
]

ABS_TOL = 1e-8  # observed max 6.4e-9 on IEEE13
SYSTEMY_SMOKE_TOL = 1e-12  # observed ~6.5e-15 on the fourwire_linecode block
SYSTEMY_SMOKE_DECK = MICRO_DIR / "fourwire_linecode.dss"

PASSIVE_CLASSES = ("Line", "Capacitor", "Reactor")
EXCLUDED_CLASSES = (
    "Transformer",
    "Vsource",
    "Isource",
    "Load",
    "Generator",
    "PVSystem",
    "Storage",
)


def discover_decks() -> list[Path]:
    decks = sorted(MICRO_DIR.glob("*.dss"))
    decks += [p for p in IEEE_DECKS if p.exists()]
    return decks


def append_result(case: str, leg: str, mark: str) -> None:
    out = os.environ.get("PIO_RESULTS_TSV")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"{case}\t{leg}\t{mark}\n")


def _resolve_mccheck() -> str | None:
    """Resolve MCCHECK to an absolute path once, at import time, before
    OpenDSS's own Compile command runs. Compile changes the process's actual
    working directory as a side effect (a long standing OpenDSS behavior),
    so a relative MCCHECK path would resolve correctly for the first deck
    and then silently miss for every deck after it."""
    mc = os.environ.get("MCCHECK")
    return str(Path(mc).resolve()) if mc else None


MCCHECK_BIN = _resolve_mccheck()


def pio(deck: Path):
    """powerio's own admittance, node list, and bus/terminal resolution map,
    dumped as JSON by the mccheck helper (calc_multiconductor_admittance_matrix
    has no Python binding)."""
    if not MCCHECK_BIN:
        raise RuntimeError(
            "MCCHECK must point at the built powerio-eval-mccheck binary "
            "(cargo build --release --manifest-path evals/validation/mccheck/Cargo.toml)"
        )
    result = subprocess.run(
        [MCCHECK_BIN, str(deck)], capture_output=True, text=True, check=False
    )
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or "passive admittance helper failed")
    d = json.loads(result.stdout)
    if d.get("scope") != "passive_components":
        raise RuntimeError("admittance helper did not declare its passive projection")
    n = len(d["nodes"])
    Y = np.zeros((n, n), dtype=complex)
    for r, c, re, im in d["entries"]:
        Y[r, c] += complex(re, im)
    resolution = {k.upper(): v for k, v in d["resolution"]}
    return d["nodes"], Y, resolution, d["diagnostics"]


def yprim() -> np.ndarray:
    y = np.array(dss.CktElement.YPrim())
    y = y[0::2] + 1j * y[1::2]
    k = int(round(np.sqrt(len(y))))
    return y.reshape(k, k)


def assemble(deck: Path, res: dict, n: int):
    """Sum every passive element's YPrim onto powerio's node rows. Returns
    the assembled matrix and the set of powerio rows touched by an excluded
    (non passive) element."""
    dss.Text.Command(f'Compile "{deck}"')
    Y = np.zeros((n, n), dtype=complex)
    tainted: set[int] = set()
    switches = 0
    for cls in PASSIVE_CLASSES + EXCLUDED_CLASSES:
        dss.Circuit.SetActiveClass(cls)
        name = dss.ActiveClass.First()
        while name:
            full = dss.CktElement.Name()
            if cls == "Line":
                dss.Circuit.SetActiveElement(full)
                if dss.Properties.Value("switch").lower().startswith(("y", "t")):
                    passive_here = False
                    switches += 1
                else:
                    passive_here = True
            else:
                passive_here = cls in PASSIVE_CLASSES
            # YPrim rows run terminal by terminal, conductor by conductor.
            # NodeOrder gives the node number per conductor; BusNames the bus
            # per terminal. Node 0 is ground and has no unknown row.
            node_order = list(dss.CktElement.NodeOrder())
            bus_names = [b.split(".")[0].upper() for b in dss.CktElement.BusNames()]
            nconds = dss.CktElement.NumConductors()
            rows: list[int | None] = []
            for k, node in enumerate(node_order):
                term = k // nconds
                if node == 0 or term >= len(bus_names):
                    rows.append(None)
                    continue
                tgt = res.get(f"{bus_names[term]}.{node}")
                rows.append(tgt if isinstance(tgt, int) else None)
            if passive_here and dss.CktElement.Enabled():
                Yp = yprim()
                if Yp.shape[0] == len(rows):
                    for a, ra in enumerate(rows):
                        if ra is None:
                            continue
                        for b_, rb in enumerate(rows):
                            if rb is None:
                                continue
                            Y[ra, rb] += Yp[a, b_]
            elif not passive_here:
                for r in rows:
                    if r is not None:
                        tainted.add(r)
            name = dss.ActiveClass.Next()
    return Y, tainted, switches


def compare_deck(deck: Path) -> tuple[str, list[str]]:
    """Returns (mark, failures): mark is 'ok', 'FAIL', or 'n/a' (nothing
    comparable after exclusion, which asserts nothing either way)."""
    print(f"\n===== {deck.name}")
    nodes, P, res, diags = pio(deck)
    D, tainted, switches = assemble(deck, res, len(nodes))
    keep = [i for i in range(len(nodes)) if i not in tainted]
    print(
        f"  nodes={len(nodes)}  compared={len(keep)}  excluded(non-passive)={len(tainted)}"
        f"  switch lines skipped={switches}  powerio diagnostics={len(diags)}"
    )
    for m in diags[:4]:
        print(f"    powerio diagnostic: {m}")
    if not keep:
        print("  nothing comparable")
        return "n/a", []

    sub = np.ix_(keep, keep)
    d = np.abs(P[sub] - D[sub])
    mag = np.abs(D[sub])
    max_abs = float(d.max())
    print(
        f"  max abs diff = {max_abs:.4e}   max |Y| = {mag.max():.4f}"
        f"   max rel = {(d / np.maximum(mag, 1e-9)).max():.3e}"
    )
    bad = np.argwhere(d > ABS_TOL)
    failures = []
    for r, c in bad[:6]:
        failures.append(
            f"{nodes[keep[r]]} x {nodes[keep[c]]}: pio={P[keep[r], keep[c]]:+.8f} "
            f"dss={D[keep[r], keep[c]]:+.8f}"
        )
    if len(bad) > 6:
        failures.append(f"... {len(bad)} mismatching entries total")
    return ("FAIL" if failures else "ok"), failures


def systemy_smoke() -> list[str]:
    """The one SystemY comparison that is valid: see the module docstring.
    Only Line.l1 reaches the sourcebus-loadbus off diagonal block."""
    if not SYSTEMY_SMOKE_DECK.exists():
        return [f"{SYSTEMY_SMOKE_DECK} is missing"]
    nodes, P, res, _diags = pio(SYSTEMY_SMOKE_DECK)
    dss.Text.Command(f'Compile "{SYSTEMY_SMOKE_DECK}"')
    order = [s.upper() for s in dss.Circuit.YNodeOrder()]
    data, indices, indptr = dss.YMatrix.getYsparse()
    n = len(order)
    D = np.asarray(sp.csc_matrix((data, indices, indptr), shape=(n, n)).todense())

    def block_rows(bus: str, terminals: list[str]) -> list[tuple[int, int]]:
        pairs = []
        for t in terminals:
            label = f"{bus}.{t}"
            j = res.get(label)
            if label in order and isinstance(j, int):
                pairs.append((order.index(label), j))
        return pairs

    src = block_rows("SOURCEBUS", ["1", "2", "3"])
    load = block_rows("LOADBUS", ["1", "2", "3", "4"])
    if not src or not load:
        return ["sourcebus/loadbus nodes not found in OpenDSS's YNodeOrder"]
    dss_block = np.array([[D[k1, k2] for _, k2 in load] for k1, _ in src])
    pio_block = np.array([[P[j1, j2] for _, j2 in load] for j1, _ in src])
    diff = float(np.max(np.abs(dss_block - pio_block)))
    print(
        f"\n===== SystemY smoke: sourcebus x loadbus block on {SYSTEMY_SMOKE_DECK.name}\n"
        f"  max abs diff = {diff:.3e}"
    )
    if diff >= SYSTEMY_SMOKE_TOL:
        return [f"sourcebus x loadbus block diff {diff:.3e} >= {SYSTEMY_SMOKE_TOL:.0e}"]
    return []


def main() -> int:
    args = sys.argv[1:]
    decks = discover_decks()
    if "--count" in args:
        print(len(decks))
        return 0

    exit_code = 0
    for deck in decks:
        rel = deck.relative_to(REPO_ROOT).as_posix()
        try:
            mark, failures = compare_deck(deck)
        except Exception as err:  # noqa: BLE001
            mark, failures = "FAIL", [str(err)]
        append_result(rel, "opendss_yprim", mark)
        if failures:
            if mark == "FAIL":
                exit_code = 1
            print(f"  {mark}: {rel}")
            for failure in failures:
                print(f"    {failure}")

    try:
        smoke_failures = systemy_smoke()
    except Exception as err:  # noqa: BLE001
        smoke_failures = [str(err)]
    append_result(
        SYSTEMY_SMOKE_DECK.relative_to(REPO_ROOT).as_posix(),
        "opendss_systemy_smoke",
        "ok" if not smoke_failures else "FAIL",
    )
    if smoke_failures:
        exit_code = 1
        for failure in smoke_failures:
            print(f"  FAILED: {failure}")

    return exit_code


if __name__ == "__main__":
    sys.exit(main())
