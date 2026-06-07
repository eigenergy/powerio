#!/usr/bin/env python3
"""Validate caseio's parse and Y_bus against pandapower on a MATPOWER case.

pandapower is an independent MATPOWER reader (via matpowercaseframes) and carries
PYPOWER's `makeYbus`, the same admittance kernel MATPOWER uses. We compare the two
where they must agree:

- bus / branch / gen counts;
- per-branch r, x, b, tap, shift (raw MATPOWER values, file order);
- per-bus demand and shunt (Pd, Qd, Gs, Bs in MW/MVAr);
- the full bus admittance matrix Y_bus, element for element.

We read pandapower's ppc directly with `_m2ppc` (the raw MATPOWER-per-unit case)
rather than `from_mpc`, which builds a `net`: `from_mpc` reorders buses, adds
auxiliary buses, and raises on dclines / parallel branches inside `from_ppc`.
`_m2ppc` runs before any of that, so it works on every case (pegase included) and
keeps the bus order aligned with caseio's file order.

    python benchmarks/validate_pandapower.py tests/data/case14.m

Exit 0 on a full match, 1 on any mismatch. Needs the `casemat` package and
pandapower (`pip install 'casemat[bench]'`).
"""

import logging
import sys
import warnings
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore", category=FutureWarning)
warnings.filterwarnings("ignore", category=UserWarning)
logging.getLogger("pandapower").setLevel(logging.ERROR)

import casemat
from pandapower.converter.matpower.from_mpc import _m2ppc
from pandapower.pypower.idx_brch import BR_B, BR_B_ASYM, BR_R, BR_X, F_BUS, SHIFT, T_BUS, TAP
from pandapower.pypower.idx_bus import BS, BUS_I, GS, PD, QD
from pandapower.pypower.idx_gen import GEN_BUS, PG, PMAX, PMIN
from pandapower.pypower.makeYbus import makeYbus

ATOL = 1e-6
RTOL = 1e-6
YTOL_ABS = 1e-6
YTOL_REL = 1e-7


def eff_tap(t):
    """MATPOWER tap==0 means 1; both sides use the rule, so normalize first."""
    return np.where(np.asarray(t) == 0.0, 1.0, t)


def main():
    if len(sys.argv) != 2:
        print("usage: validate_pandapower.py <case.m>", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    name = path.name

    case = casemat.parse_matpower(str(path))
    ppc = _m2ppc(str(path), "mpc")
    bus, branch, gen = ppc["bus"], ppc["branch"], ppc["gen"]
    base_mva = float(ppc["baseMVA"])

    problems = []

    # --- counts ---------------------------------------------------------
    n = len(case.buses)
    m = len(case.branches)
    ng = len(case.gens)
    if n != bus.shape[0]:
        problems.append(f"bus count: caseio={n} pandapower={bus.shape[0]}")
    if m != branch.shape[0]:
        problems.append(f"branch count: caseio={m} pandapower={branch.shape[0]}")
    if ng != gen.shape[0]:
        problems.append(f"gen count: caseio={ng} pandapower={gen.shape[0]}")
    if problems:
        report(name, problems)
        return 1

    # --- bus-id alignment (both should be file order) -------------------
    caseio_ids = [b["id"] for b in case.buses]
    pp_ids = (bus[:, BUS_I].astype(int) + 1).tolist()
    if sorted(caseio_ids) != sorted(pp_ids):
        problems.append("bus id sets differ between caseio and pandapower")
        report(name, problems)
        return 1
    pp_row_of_id = {bid: r for r, bid in enumerate(pp_ids)}
    order = np.array([pp_row_of_id[bid] for bid in caseio_ids])  # pp rows, caseio order

    # --- per-bus demand / shunt -----------------------------------------
    cb = case.buses
    check_vec(problems, "bus.pd", [b["pd"] for b in cb], bus[order, PD])
    check_vec(problems, "bus.qd", [b["qd"] for b in cb], bus[order, QD])
    check_vec(problems, "bus.gs", [b["gs"] for b in cb], bus[order, GS])
    check_vec(problems, "bus.bs", [b["bs"] for b in cb], bus[order, BS])

    # --- per-branch r/x/b/tap/shift (file order, row by row) ------------
    br = case.branches
    for k in range(m):
        cf, ct = br[k]["from_id"], br[k]["to_id"]
        pf, pt = int(branch[k, F_BUS]) + 1, int(branch[k, T_BUS]) + 1
        if (cf, ct) != (pf, pt):
            problems.append(f"branch[{k}] endpoints: caseio=({cf},{ct}) pandapower=({pf},{pt})")
    check_vec(problems, "branch.r", [b["r"] for b in br], branch[:, BR_R])
    check_vec(problems, "branch.x", [b["x"] for b in br], branch[:, BR_X])
    check_vec(problems, "branch.b", [b["b"] for b in br], branch[:, BR_B])
    check_vec(problems, "branch.tap", eff_tap([b["tap"] for b in br]), eff_tap(branch[:, TAP]))
    check_vec(problems, "branch.shift", [b["shift"] for b in br], branch[:, SHIFT])

    # --- generators (file order) ----------------------------------------
    gn = case.gens
    for k in range(ng):
        cgb = gn[k]["bus_id"]
        pgb = int(gen[k, GEN_BUS]) + 1
        if cgb != pgb:
            problems.append(f"gen[{k}] bus: caseio={cgb} pandapower={pgb}")
    check_vec(problems, "gen.pg", [g["pg"] for g in gn], gen[:, PG])
    check_vec(problems, "gen.pmax", [g["pmax"] for g in gn], gen[:, PMAX])
    check_vec(problems, "gen.pmin", [g["pmin"] for g in gn], gen[:, PMIN])

    # --- Y_bus, element for element -------------------------------------
    yc = case.ybus().tocsr()
    # makeYbus uses branch endpoints directly as dense row indices, so it needs
    # buses numbered 0..nb-1. _m2ppc keeps the raw MATPOWER ids (id-1), which are
    # gappy on pegase-style cases, so renumber endpoints to ppc row positions.
    # It also reads extended columns (asymmetric impedance, BR_G) the raw ppc
    # doesn't carry; pad them with zeros to recover the standard kernel.
    pos_of_id0 = {int(v): r for r, v in enumerate(bus[:, BUS_I])}
    by = np.zeros((branch.shape[0], BR_B_ASYM + 1))
    by[:, : branch.shape[1]] = branch
    by[:, F_BUS] = [pos_of_id0[int(v)] for v in branch[:, F_BUS]]
    by[:, T_BUS] = [pos_of_id0[int(v)] for v in branch[:, T_BUS]]
    yp, _, _ = makeYbus(base_mva, bus, by)
    yp = yp.tocsr()[order][:, order]  # pandapower Ybus in caseio bus order
    if yc.shape != yp.shape:
        problems.append(f"ybus shape: caseio={yc.shape} pandapower={yp.shape}")
    else:
        d = (yc - yp).tocoo()
        err = float(np.abs(d.data).max()) if d.nnz else 0.0
        scale = float(np.abs(yc.tocoo().data).max()) if yc.nnz else 1.0
        if err > YTOL_ABS + YTOL_REL * scale:
            problems.append(f"ybus max|Δ|={err:.3e} (scale {scale:.3e})")

    report(name, problems)
    return 1 if problems else 0


def check_vec(problems, label, a, b, atol=ATOL, rtol=RTOL):
    a = np.asarray(a, dtype=float)
    b = np.asarray(b, dtype=float)
    if a.shape != b.shape:
        problems.append(f"{label}: shape {a.shape} vs {b.shape}")
        return
    bad = ~np.isclose(a, b, atol=atol, rtol=rtol, equal_nan=True)
    if bad.any():
        i = int(np.argmax(bad))
        problems.append(f"{label}: {int(bad.sum())} differ, first at {i} caseio={a[i]} pandapower={b[i]}")


def report(name, problems):
    if not problems:
        print(f"MATCH: {name} — counts, branch/bus values, and Y_bus identical")
    else:
        print(f"MISMATCH: {name} ({len(problems)})")
        for p in problems[:40]:
            print("  ", p)


if __name__ == "__main__":
    sys.exit(main())
