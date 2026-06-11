#!/usr/bin/env python
"""Validate powerio's egret writer against the egret package (the oracle).

egret has no PowerModels reader, so PowerModels.jl can't check this leg the way it
checks the MATPOWER/PSS/E paths. Instead we use egret itself: load both sides as
`egret.data.model_data.ModelData` and compare the electrical core (bus/branch/gen/
load/shunt counts, demand and generation totals, shunt admittance totals, and the
generator cost coefficients summed by polynomial degree).

  ref  = a MATPOWER `.m` (loaded with egret's own matpower parser) or an egret JSON
  test = powerio's egret JSON output

  python validate_egret.py <ref.m|ref.json> <test.json>

Needs the egret package (`pip install -r benchmarks/requirements.txt`), a benchmark
dependency, not a powerio one. Importing ModelData is solver-free, so no pyomo or
solver stack is pulled in.
"""

import math
import os
import sys

from egret.data.model_data import ModelData
from egret.parsers.matpower_parser import create_ModelData


def load(path):
    if path.lower().endswith(".m"):
        return create_ModelData(path)
    return ModelData(path)


def core(md):
    d = md.data
    els = d["elements"]

    def items(et):
        return els.get(et, {})

    def total(et, field):
        s = 0.0
        for e in items(et).values():
            v = e.get(field, 0.0)
            if isinstance(v, (int, float)):
                s += v
        return round(s, 4)

    # Cost coefficients summed by polynomial degree across all generators; catches
    # a dropped or mis-scaled curve without matching generators one to one.
    cost = {}
    for g in items("generator").values():
        pc = g.get("p_cost")
        if isinstance(pc, dict) and pc.get("cost_curve_type") == "polynomial":
            # egret's in-memory matpower parse keys degrees by int; a JSON-loaded
            # ModelData keys them by string. Normalize so the two compare equal.
            for deg, coeff in pc.get("values", {}).items():
                key = str(deg)
                cost[key] = round(cost.get(key, 0.0) + float(coeff), 6)

    return {
        "baseMVA": round(d["system"].get("baseMVA", 0.0), 6),
        "n_bus": len(items("bus")),
        "n_branch": len(items("branch")),
        "n_gen": len(items("generator")),
        "n_load": len(items("load")),
        "n_shunt": len(items("shunt")),
        "sum_pload": total("load", "p_load"),
        "sum_qload": total("load", "q_load"),
        "sum_pmax": total("generator", "p_max"),
        "sum_gs": total("shunt", "gs"),
        "sum_bs": total("shunt", "bs"),
        "cost": cost,
    }


def core_pu(path):
    """The per-unit electrical core via the egret oracle, in the same field layout
    as core_json.jl (the PowerModels oracle): counts plus power totals divided by
    baseMVA. Lets a conversion's core be compared to its source's core regardless
    of which oracle read each side.
    """
    md = load(path)
    d = md.data
    els = d["elements"]
    base = d["system"].get("baseMVA", 1.0) or 1.0

    def items(et):
        return els.get(et, {})

    def total(et, field):
        s = 0.0
        for e in items(et).values():
            v = e.get(field, 0.0)
            if isinstance(v, (int, float)):
                s += v
        return round(s / base, 6)

    return {
        "n_bus": len(items("bus")),
        "n_branch": len(items("branch")),
        "n_gen": len(items("generator")),
        "n_load": len(items("load")),
        "n_shunt": len(items("shunt")),
        "sum_pd": total("load", "p_load"),
        "sum_qd": total("load", "q_load"),
        "sum_pg": total("generator", "pg"),
        "sum_gs": total("shunt", "gs"),
        "sum_bs": total("shunt", "bs"),
    }


def close(a, b):
    return math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-6)


def compare(ref_path, test_path, check_cost=True):
    """Problems (empty list == match) comparing two cases' cores via the egret
    oracle. `check_cost=False` skips the generator cost curve, for sources that
    don't carry cost (PSS/E, PowerWorld), whose egret output legitimately has none.
    """
    r, t = core(load(ref_path)), core(load(test_path))
    problems = []
    for k, rv in r.items():
        tv = t[k]
        if k == "cost":
            if not check_cost:
                continue
            for ck in set(rv) | set(tv):
                if not close(rv.get(ck, 0.0), tv.get(ck, 0.0)):
                    problems.append(f"cost[deg {ck}]: ref={rv.get(ck, 0.0)} test={tv.get(ck, 0.0)}")
        elif isinstance(rv, float):
            if not close(rv, tv):
                problems.append(f"{k}: ref={rv} test={tv}")
        elif rv != tv:
            problems.append(f"{k}: ref={rv} test={tv}")
    return problems


def main():
    args = [a for a in sys.argv[1:] if a != "--no-cost"]
    check_cost = "--no-cost" not in sys.argv[1:]
    ref_path, test_path = args[0], args[1]
    problems = compare(ref_path, test_path, check_cost)
    name = os.path.basename(ref_path)
    if problems:
        print(f"MISMATCH: {name}")
        for p in problems:
            print("  ", p)
        sys.exit(1)
    print(f"MATCH: {name} - core identical (egret oracle)")


if __name__ == "__main__":
    main()
