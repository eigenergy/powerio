"""Re-solve emitted .dss cases against their originals.

Usage:
    cargo test -p powerio-dist --test matrix -- --ignored emit_for_physics_check
    <python-with-opendssdirect> powerio-dist/tools/physics_check.py

For every dss sourced fixture the harness writes three regenerated cases
under target/physics (canonical, via BMOPF, via PMD). This script solves
each against the original and reports the maximum per node voltage
deviation in per unit of the original node magnitude (nodes below 1 volt
are compared absolutely, in volts). The conversion contract bound is 1e-8.
"""

import glob
import os
import sys

ORIGINALS = {
    "opendss_ieee13_IEEE13Nodeckt": "tests/data/dist/opendss/ieee13/IEEE13Nodeckt.dss",
    "opendss_ieee34_ieee34Mod1": "tests/data/dist/opendss/ieee34/ieee34Mod1.dss",
    "opendss_ieee123_IEEE123Master": "tests/data/dist/opendss/ieee123/IEEE123Master.dss",
    "micro_xfmr_single_phase": "tests/data/dist/micro/xfmr_single_phase.dss",
    "micro_xfmr_center_tap": "tests/data/dist/micro/xfmr_center_tap.dss",
    "micro_xfmr_wye_delta": "tests/data/dist/micro/xfmr_wye_delta.dss",
    "micro_xfmr_delta_wye": "tests/data/dist/micro/xfmr_delta_wye.dss",
    "micro_switch": "tests/data/dist/micro/switch.dss",
    "micro_fourwire_linecode": "tests/data/dist/micro/fourwire_linecode.dss",
    "micro_defaults_degenerate": "tests/data/dist/micro/defaults_degenerate.dss",
    "micro_linecode_10x10": "tests/data/dist/micro/linecode_10x10.dss",
}


def solve(path):
    import opendssdirect as dss

    # The converter drops voltage regulator controls by documented policy
    # (RegControl becomes a fixed tap transformer). The cases run their own
    # Solve while loading, so control actions must be off before that:
    # inject the option right after the circuit line on both sides.
    text = open(path, encoding="utf-8", errors="replace").read()
    lines = text.splitlines()
    injected = False
    for i, line in enumerate(lines):
        head = line.lower().lstrip()
        # Both circuit spellings appear in the vendored masters: the writer
        # emits "New Circuit.x", ieee34/ieee123 use "New object=circuit.x".
        if head.startswith("new circuit") or head.startswith("new object=circuit"):
            # Tight solver tolerance: the default 1e-4 pu would swamp the
            # 1e-8 conversion bound with convergence noise.
            lines.insert(i + 1, "Set Controlmode=OFF")
            lines.insert(i + 2, "Set tolerance=0.0000000001")
            injected = True
            break
    if not injected:
        raise SystemExit(f"{path}: no circuit definition found to stage")
    staged = os.path.join(os.path.dirname(os.path.abspath(path)), "_staged_" + os.path.basename(path))
    with open(staged, "w") as f:
        f.write("\n".join(lines) + "\n")

    try:
        dss.Text.Command("Clear")
        dss.Text.Command(f'Redirect "{os.path.abspath(staged)}"')
        dss.Text.Command("Set Controlmode=OFF")
        dss.Text.Command("Solve")
    finally:
        os.unlink(staged)
    if not dss.Solution.Converged():
        return None
    volts = {}
    for bus in dss.Circuit.AllBusNames():
        dss.Circuit.SetActiveBus(bus)
        nodes = dss.Bus.Nodes()
        raw = dss.Bus.Voltages()
        for k, node in enumerate(nodes):
            volts[f"{bus}.{node}"] = complex(raw[2 * k], raw[2 * k + 1])
    return volts


def compare(base, emitted):
    # Deviation in per unit of the bus's own voltage scale (the largest
    # node magnitude at the bus), so near zero neutral nodes compare
    # against the working voltage, not their own tiny magnitude.
    bus_base = {}
    for node, v0 in base.items():
        bus = node.rsplit(".", 1)[0]
        bus_base[bus] = max(bus_base.get(bus, 0.0), abs(v0))
    worst = 0.0
    worst_node = ""
    for node, v0 in base.items():
        v1 = emitted.get(node)
        if v1 is None:
            return None, f"missing node {node}"
        base_v = max(bus_base[node.rsplit(".", 1)[0]], 1.0)
        dev = abs(v1 - v0) / base_v
        if dev > worst:
            worst, worst_node = dev, node
    return worst, worst_node


# Cells whose deviation has a documented cause. Bounds above 1e-8 carry
# the reason; "loss" cells are format losses every conversion reports in
# its warnings (constant power only loads in BMOPF, the center tap
# collapse, an unsupported transformer shape, no vminpu field in the
# ENGINEERING model). The engine seeding entries cover OpenDSS treating
# written properties differently from untouched defaults (an untouched
# load seeds VBase 7200 V; writing kv=12.47 computes 12470/sqrt(3)),
# amplified near vminpu boundaries.
DOCUMENTED = {
    ("micro_defaults_degenerate", "canonical"): (1e-6, "engine seeding asymmetry"),
    ("micro_defaults_degenerate", "via_pmd"): (1e-6, "engine seeding asymmetry"),
    ("micro_defaults_degenerate", "via_bmopf"): (1e-2, "BMOPF: constant power loads only"),
    ("opendss_ieee13_IEEE13Nodeckt", "via_bmopf"): (1e-1, "BMOPF: constant power loads only"),
    ("opendss_ieee34_ieee34Mod1", "via_bmopf"): (1e-1, "BMOPF: constant power loads only"),
    ("opendss_ieee34_ieee34Mod1", "via_pmd"): (1e-1, "no vminpu field in ENGINEERING"),
    ("opendss_ieee123_IEEE123Master", "via_bmopf"): (None, "transformer shape outside the four BMOPF subtypes"),
    ("opendss_ieee123_IEEE123Master", "via_pmd"): (1e-2, "regulator bank restatement"),
    ("micro_xfmr_center_tap", "via_bmopf"): (2e-1, "BMOPF: center tap collapses to two windings"),
    ("micro_xfmr_single_phase", "via_pmd"): (1e-6, "engine Z1/Z0 vs MVAsc input path"),
    # PMD models a dss switch as a 1e-7 ohm series element while the engine's
    # switch dummy works out near 1e-3 ohm over the forced length.
    ("micro_switch", "via_pmd"): (1e-5, "ENGINEERING switch impedance convention"),
    ("micro_xfmr_center_tap", "via_pmd"): (1e-6, "engine Z1/Z0 vs MVAsc input path"),
}


def main():
    failures = 0
    for stem, original in ORIGINALS.items():
        emitted_paths = sorted(glob.glob(f"target/physics/{stem}.*.dss"))
        if not emitted_paths:
            # An empty glob must fail, or the gate silently checks nothing
            # (forgotten emit step, renamed fixture).
            print(f"{stem}: NO EMITTED CASES under target/physics (run the emit test first)")
            failures += 1
            continue
        base = solve(original)
        if base is None:
            print(f"{stem}: ORIGINAL DID NOT CONVERGE")
            failures += 1
            continue
        for emitted_path in emitted_paths:
            kind = emitted_path.rsplit(".", 2)[-2]
            bound, reason = DOCUMENTED.get((stem, kind), (1e-8, None))
            emitted = solve(emitted_path)
            if emitted is None:
                print(f"{stem} [{kind}]: DID NOT CONVERGE")
                failures += 1
                continue
            worst, where = compare(base, emitted)
            if worst is None:
                if bound is None:
                    print(f"{stem} [{kind}]: {where} (documented: {reason})")
                else:
                    print(f"{stem} [{kind}]: {where}")
                    failures += 1
            elif bound is not None and worst <= bound:
                note = f" (documented: {reason})" if reason else ""
                print(f"{stem} [{kind}]: max deviation {worst:.3e} at {where} ok{note}")
            else:
                print(f"{stem} [{kind}]: max deviation {worst:.3e} at {where} FAIL")
                failures += 1
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
