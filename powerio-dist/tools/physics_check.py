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
    for i, line in enumerate(lines):
        if line.lower().lstrip().startswith("new circuit"):
            # Tight solver tolerance: the default 1e-4 pu would swamp the
            # 1e-8 conversion bound with convergence noise.
            lines.insert(i + 1, "Set Controlmode=OFF")
            lines.insert(i + 2, "Set tolerance=0.0000000001")
            break
    staged = os.path.join(os.path.dirname(os.path.abspath(path)), "_staged_" + os.path.basename(path))
    with open(staged, "w") as f:
        f.write("\n".join(lines) + "\n")

    dss.Text.Command("Clear")
    dss.Text.Command(f'Redirect "{os.path.abspath(staged)}"')
    dss.Text.Command("Set Controlmode=OFF")
    dss.Text.Command("Solve")
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


def main():
    failures = 0
    for stem, original in ORIGINALS.items():
        base = solve(original)
        if base is None:
            print(f"{stem}: ORIGINAL DID NOT CONVERGE")
            failures += 1
            continue
        for emitted_path in sorted(glob.glob(f"target/physics/{stem}.*.dss")):
            kind = emitted_path.rsplit(".", 2)[-2]
            emitted = solve(emitted_path)
            if emitted is None:
                print(f"{stem} [{kind}]: DID NOT CONVERGE")
                failures += 1
                continue
            worst, where = compare(base, emitted)
            if worst is None:
                print(f"{stem} [{kind}]: {where}")
                failures += 1
            else:
                status = "ok" if worst <= 1e-8 else "FAIL"
                if status == "FAIL":
                    failures += 1
                print(f"{stem} [{kind}]: max deviation {worst:.3e} at {where} {status}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
