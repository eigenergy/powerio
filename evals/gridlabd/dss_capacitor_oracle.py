"""Pinned common-neutral capacitor primitives and complete loaded port results.

Covers the connection used by the IEEE Neutral-to-Earth Voltage feeder. Public
inputs remain external; these small cases isolate phase ordering and frequency.
"""

import argparse
import hashlib
import json
from pathlib import Path

from ieee4_import_oracle import evaluate, run


def pairs(values):
    return [complex(a, b) for a, b in zip(values[::2], values[1::2], strict=True)]


def case(nodes, frequency, base):
    count = len(nodes)
    hot = ".".join(map(str, nodes))
    neutral = ".".join(["10"] * count)
    text = (f"clear\nnew circuit.c basekv=12.47 frequency={frequency} basefreq={frequency} "
            "r1=.1 x1=.2 r0=.2 x0=.3\n"
            "new line.line1 phases=4 bus1=sourcebus.1.2.3.0 bus2=end.1.2.3.10 length=1 units=km "
            "rmatrix=[.4|0 .4|0 0 .4|0 0 0 .1] xmatrix=[.2|0 .2|0 0 .2|0 0 0 .05] cmatrix=[0|0 0|0 0 0|0 0 0 0]\n")
    for phase in range(1, 4):
        text += f"new load.l{phase} phases=1 bus1=end.{phase}.10 kv=7.199557856794634 kw={phase * 30} kvar={phase * 8}\n"
    kv = 7.199557856794634 if count == 1 else 12.47
    text += (f"new capacitor.c bus1=end.{hot} bus2=end.{neutral} phases={count} conn=wye "
             f"kvar={count * 20} kv={kv} basefreq={base}\n"
             f"set frequency={frequency} controlmode=off tolerance=1e-11 maxiterations=1000\nsolve\n")
    return text


def primitives(native, canonical, shunt):
    for engine in (native, canonical):
        if engine.Circuit.SetActiveElement("capacitor.c") < 0:
            raise ValueError("canonical capacitor identity missing")
    expected = pairs(native.CktElement.YPrim())
    actual = pairs(canonical.CktElement.YPrim())
    if native.CktElement.NodeOrder() != canonical.CktElement.NodeOrder():
        raise ValueError("capacitor terminal order changed")
    size = native.CktElement.NumConductors() * native.CktElement.NumTerminals()
    node_indices = [shunt["terminal_map"].index(str(n)) for n in native.CktElement.NodeOrder()]
    n = len(shunt["terminal_map"])
    compressed = [[0j] * n for _ in range(n)]
    for i, row in enumerate(node_indices):
        for j, col in enumerate(node_indices):
            compressed[row][col] += expected[i + size * j]
    typed = [[complex(g, b) for g, b in zip(gr, br, strict=True)]
             for gr, br in zip(shunt["g"], shunt["b"], strict=True)]
    typed_error = max(abs(a - b) for ar, br in zip(compressed, typed, strict=True)
                      for a, b in zip(ar, br, strict=True))
    canonical_error = max(abs(a - b) for a, b in zip(expected, actual, strict=True))
    # This control targets the return node, independently of the existing
    # changed-Line control in the full snapshot harness.
    canonical.Text.Command("edit capacitor.c bus2=end.0.0.0")
    canonical.Text.Command("solve")
    canonical.Circuit.SetActiveElement("capacitor.c")
    changed = pairs(canonical.CktElement.Currents())
    original = pairs(native.CktElement.Currents())
    wrong_return_error = max(abs(a - b) for a, b in zip(changed, original, strict=True))
    return dict(typed_max_error_S=typed_error, canonical_max_error_S=canonical_error,
                wrong_return_current_error_A=wrong_return_error,
                passed=typed_error < 1e-12 and canonical_error < 1e-12 and wrong_return_error > 1e-5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    rows = {}
    for nodes in ([2], [3, 1], [1, 2, 3], [3, 1, 2]):
        for frequency, base in ((60, 60), (50, 60), (60, 50)):
            name = f"nodes{''.join(map(str, nodes))}-f{frequency}-base{base}"
            folder = args.output / name
            folder.mkdir()
            source = folder / "input.dss"
            source.write_text(case(nodes, frequency, base))
            row = evaluate(source, args.bridge.resolve(), folder, 1e-11)
            if row["passed"]:
                network = json.loads((folder / "imported/bridge.json").read_text())["network"]
                shunt = next(s for s in network["shunts"] if s["name"] == "c")
                row["primitive"] = primitives(run(source, 1e-11),
                    run(folder / "imported/dss/case.dss", 1e-11), shunt)
                row["passed"] &= row["primitive"]["passed"]
            rows[name] = row
            print(name, "PASS" if row["passed"] else "FAIL", row.get("error", ""), flush=True)
    report = dict(cases=rows, passed=all(row["passed"] for row in rows.values()),
                  bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
