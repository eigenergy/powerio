#!/usr/bin/env python3
"""Validate powerio's parse and Y_bus against pandapower on MATPOWER cases.

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
keeps the bus order aligned with powerio's file order.

    python benchmarks/validate_pandapower.py tests/data/case14.m [case.m ...]

Imports pandapower once and loops every case (the validation matrix runs all
fixtures in one process to amortize the import). Per case the mark is `ok`, `FAIL`,
or `n/a` (pandapower's reader can't parse the case — an oracle limit, not a powerio
discrepancy). Appends `<stem>\tpp\t<mark>` to $PIO_RESULTS_TSV when set. Exits
nonzero only on a real mismatch. Needs the `powerio` package and pandapower.
"""

import logging
import os
import sys
import warnings
from collections import defaultdict
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore", category=FutureWarning)
warnings.filterwarnings("ignore", category=UserWarning)
logging.getLogger("pandapower").setLevel(logging.ERROR)

import powerio
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


def sum_by_bus(elems, ka, kb):
    """Sum two element fields onto their bus id. powerio models loads and shunts
    as first-class elements (potentially several per bus), so fold them back onto
    the bus to compare against pandapower's raw per-bus columns."""
    a, b = defaultdict(float), defaultdict(float)
    for e in elems:
        a[e["bus"]] += e[ka]
        b[e["bus"]] += e[kb]
    return a, b


def check_case(path):
    """Validate one case. Returns the mark: "ok", "FAIL", or "n/a"."""
    name = path.name
    case = powerio.parse_file(str(path))
    try:
        ppc = _m2ppc(str(path), "mpc")
    except OverflowError as exc:
        # pandapower's reader (matpowercaseframes) does int(float(tok)) and raises on
        # the `Inf` limit tokens MATPOWER uses for "unlimited" (e.g. pegase branch
        # limits). That's an oracle limitation, not a powerio discrepancy — mark it
        # n/a rather than report a false mismatch. powerio, PowerModels, and
        # ExaPowerIO all read the case, so it stays covered by the other validators.
        print(f"SKIP: {name} — pandapower's matpowercaseframes reader can't parse Inf limits ({exc})")
        return "n/a"
    bus, branch, gen = ppc["bus"], ppc["branch"], ppc["gen"]
    base_mva = float(ppc["baseMVA"])

    problems = []

    # --- counts ---------------------------------------------------------
    n = len(case.buses)
    m = len(case.branches)
    ng = len(case.gens)
    if n != bus.shape[0]:
        problems.append(f"bus count: powerio={n} pandapower={bus.shape[0]}")
    if m != branch.shape[0]:
        problems.append(f"branch count: powerio={m} pandapower={branch.shape[0]}")
    if ng != gen.shape[0]:
        problems.append(f"gen count: powerio={ng} pandapower={gen.shape[0]}")
    if problems:
        report(name, problems)
        return "FAIL"

    # --- bus-id alignment (both should be file order) -------------------
    powerio_ids = [b["id"] for b in case.buses]
    pp_ids = (bus[:, BUS_I].astype(int) + 1).tolist()
    if sorted(powerio_ids) != sorted(pp_ids):
        problems.append("bus id sets differ between powerio and pandapower")
        report(name, problems)
        return "FAIL"
    pp_row_of_id = {bid: r for r, bid in enumerate(pp_ids)}
    order = np.array([pp_row_of_id[bid] for bid in powerio_ids])  # pp rows, powerio order

    # --- per-bus demand / shunt -----------------------------------------
    cb = case.buses
    pd_by_id, qd_by_id = sum_by_bus(case.loads, "p", "q")
    gs_by_id, bs_by_id = sum_by_bus(case.shunts, "g", "b")
    check_vec(problems, "bus.pd", [pd_by_id.get(b["id"], 0.0) for b in cb], bus[order, PD])
    check_vec(problems, "bus.qd", [qd_by_id.get(b["id"], 0.0) for b in cb], bus[order, QD])
    check_vec(problems, "bus.gs", [gs_by_id.get(b["id"], 0.0) for b in cb], bus[order, GS])
    check_vec(problems, "bus.bs", [bs_by_id.get(b["id"], 0.0) for b in cb], bus[order, BS])

    # --- per-branch r/x/b/tap/shift (file order, row by row) ------------
    br = case.branches
    for k in range(m):
        cf, ct = br[k]["from_id"], br[k]["to_id"]
        pf, pt = int(branch[k, F_BUS]) + 1, int(branch[k, T_BUS]) + 1
        if (cf, ct) != (pf, pt):
            problems.append(f"branch[{k}] endpoints: powerio=({cf},{ct}) pandapower=({pf},{pt})")
    check_vec(problems, "branch.r", [b["r"] for b in br], branch[:, BR_R])
    check_vec(problems, "branch.x", [b["x"] for b in br], branch[:, BR_X])
    check_vec(problems, "branch.b", [b["b"] for b in br], branch[:, BR_B])
    check_vec(problems, "branch.tap", eff_tap([b["tap"] for b in br]), eff_tap(branch[:, TAP]))
    check_vec(problems, "branch.shift", [b["shift"] for b in br], branch[:, SHIFT])

    # --- generators (file order) ----------------------------------------
    gn = case.gens
    for k in range(ng):
        cgb = gn[k]["bus"]
        pgb = int(gen[k, GEN_BUS]) + 1
        if cgb != pgb:
            problems.append(f"gen[{k}] bus: powerio={cgb} pandapower={pgb}")
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
    yp = yp.tocsr()[order][:, order]  # pandapower Ybus in powerio bus order
    if yc.shape != yp.shape:
        problems.append(f"ybus shape: powerio={yc.shape} pandapower={yp.shape}")
    else:
        # Elementwise relative check, not a single global-max scale: a localized
        # error on a small admittance entry can't hide under the largest diagonal.
        d = (yc - yp).tocoo()
        if d.nnz:
            ref_mag = np.abs(np.asarray(yc.tocsr()[d.row, d.col]).ravel())
            tol = YTOL_ABS + YTOL_REL * ref_mag
            bad = np.abs(d.data) > tol
            if bad.any():
                i = int(np.argmax(np.abs(d.data) - tol))
                problems.append(
                    f"ybus: {int(bad.sum())} entries exceed tol, "
                    f"worst |Δ|={float(np.abs(d.data)[i]):.3e} at ref|{float(ref_mag[i]):.3e}|"
                )

    report(name, problems)
    return "FAIL" if problems else "ok"


def check_vec(problems, label, a, b, atol=ATOL, rtol=RTOL):
    a = np.asarray(a, dtype=float)
    b = np.asarray(b, dtype=float)
    if a.shape != b.shape:
        problems.append(f"{label}: shape {a.shape} vs {b.shape}")
        return
    bad = ~np.isclose(a, b, atol=atol, rtol=rtol, equal_nan=True)
    if bad.any():
        i = int(np.argmax(bad))
        problems.append(f"{label}: {int(bad.sum())} differ, first at {i} powerio={a[i]} pandapower={b[i]}")


def report(name, problems):
    if not problems:
        print(f"MATCH: {name} — counts, branch/bus values, and Y_bus identical")
    else:
        print(f"MISMATCH: {name} ({len(problems)})")
        for p in problems[:40]:
            print("  ", p)


def main():
    cases = sys.argv[1:]
    if not cases:
        print("usage: validate_pandapower.py <case.m> [case.m ...]", file=sys.stderr)
        return 2
    results = os.environ.get("PIO_RESULTS_TSV")
    fails = 0
    for arg in cases:
        path = Path(arg)
        print(f"--- pp {path.stem} ---")
        mark = check_case(path)
        if mark == "FAIL":
            fails += 1
        if results:
            with open(results, "a") as fh:
                fh.write(f"{path.stem}\tpp\t{mark}\n")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
