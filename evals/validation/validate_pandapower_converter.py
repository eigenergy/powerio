#!/usr/bin/env python3
"""Validate powerio's pandapower JSON writer against pandapower itself.

For each MATPOWER case, powerio writes pandapower JSON, pandapower reads it back,
and the script compares core element counts plus the full Y_bus against
powerio's own matrix builder. Appends `<stem>\tpp-json\t<mark>` to
`$PIO_RESULTS_TSV` when set.
"""

import os
import sys
import warnings
from pathlib import Path

import numpy as np

warnings.filterwarnings("ignore")

import pandapower as pp
import powerio
from pandapower.file_io import from_json_string

YTOL_ABS = 1e-6
YTOL_REL = 1e-7


def check_case(path: Path) -> str:
    case = powerio.parse_file(path)
    conv = case.to_format("pandapower-json")
    net = from_json_string(conv.text)

    problems = []
    if len(net.bus) != case.n_buses:
        problems.append(f"bus count {len(net.bus)} != {case.n_buses}")
    if len(net.line) + len(net.trafo) != case.n_branches:
        problems.append(f"branch count {len(net.line) + len(net.trafo)} != {case.n_branches}")
    if len(net.gen) + len(net.ext_grid) != case.n_gens:
        problems.append(f"generator count {len(net.gen) + len(net.ext_grid)} != {case.n_gens}")
    if len(net.load) != case.n_loads:
        problems.append(f"load count {len(net.load)} != {case.n_loads}")
    # The writer maps MATPOWER transformer line charging b onto one bus shunt
    # per terminal (pandapower's trafo magnetizing model is inductive only),
    # and writes any branch across two voltage levels as a trafo.
    kv = {b["id"]: (b["base_kv"] if b["base_kv"] > 0 else 1.0) for b in case.buses}
    charging = sum(
        2
        for b in case.branches
        if b["b"] != 0.0
        and (b["tap"] != 0.0 or b["shift"] != 0.0 or kv[b["from_id"]] != kv[b["to_id"]])
    )
    if len(net.shunt) != case.n_shunts + charging:
        problems.append(
            f"shunt count {len(net.shunt)} != {case.n_shunts} + {charging} trafo charging"
        )

    if not problems:
        try:
            pp.runpp(net, init="flat", calculate_voltage_angles=True, numba=False)
            y_pp = net._ppc["internal"]["Ybus"]
            y_pio = case.ybus().tocsr()
            y_pp = y_pp.tocsr()
            if y_pp.shape != y_pio.shape:
                problems.append(f"ybus shape {y_pp.shape} != {y_pio.shape}")
            else:
                d = (y_pio - y_pp).tocoo()
                if d.nnz:
                    ref_mag = np.abs(np.asarray(y_pio[d.row, d.col]).ravel())
                    bad = np.abs(d.data) > (YTOL_ABS + YTOL_REL * ref_mag)
                    if bad.any():
                        i = int(np.argmax(np.abs(d.data)))
                        problems.append(
                            f"ybus {int(bad.sum())} entries differ; worst |Δ|={abs(d.data[i]):.3e}"
                        )
        except Exception as exc:  # noqa: BLE001
            problems.append(f"pandapower Y_bus failed: {type(exc).__name__}: {exc}")

    if problems:
        print(f"MISMATCH: {path.name} pandapower JSON")
        for p in problems[:30]:
            print("  ", p)
        return "FAIL"
    print(f"MATCH: {path.name} pandapower JSON counts and Y_bus")
    return "ok"


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: validate_pandapower_converter.py <case.m> [case.m ...]", file=sys.stderr)
        return 2
    results = os.environ.get("PIO_RESULTS_TSV")
    fails = 0
    for arg in sys.argv[1:]:
        mark = check_case(Path(arg))
        if mark == "FAIL":
            fails += 1
        if results:
            with open(results, "a") as fh:
                fh.write(f"{Path(arg).stem}\tpp-json\t{mark}\n")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
