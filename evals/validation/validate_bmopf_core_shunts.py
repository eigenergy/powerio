#!/usr/bin/env python3
"""Compare explicit BMOPF core shunts with OpenDSS terminal admittance stamps."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile

import numpy as np
import opendssdirect as dss


def fixtures():
    for connection in ("wye", "delta"):
        other = "delta" if connection == "wye" else "wye"
        source = "src.1.2.3" + (".0" if other == "wye" else "")
        target = "dst.1.2.3" + (".0" if connection == "wye" else "")
        yield f"{other}_{connection}", 3, f"windings=2 buses=({source} {target}) conns=({other} {connection}) kvs=(12.47 0.48) kvas=(75 75) taps=(1.025 1.05) %Rs=(1 1) xhl=2"
    yield "wye_wye", 3, "windings=2 buses=(src.1.2.3.0 dst.1.2.3.0) conns=(wye wye) kvs=(12.47 0.48) kvas=(75 75) taps=(1.025 1.05) %Rs=(1 1) xhl=2"
    yield "phase_pair", 1, "windings=2 buses=(src.1.2 dst.1.2) kvs=(12.47 0.48) kvas=(25 25) taps=(1.025 1.05) %Rs=(1 1) xhl=2"
    yield "center_tap", 1, "windings=3 buses=(src.1.0 dst.1.0 dst.0.2) kvs=(7.2 0.12 0.12) kvas=(25 25 25) taps=(1.025 1.05 1.05) %Rs=(1 1 1) xhl=2 xht=2 xlt=2"
    yield "n_winding", 3, "windings=4 buses=(src.1.2.3.0 dst.1.2.3 third.1.2.3.0 fourth.1.2.3.0) conns=(wye delta wye wye) kvs=(12.47 0.48 4.16 2.4) kvas=(1000 1000 1000 1000) taps=(1.025 1.05 1 1) %Rs=(1 1 1 1) xscarray=(2 2 2 2 2 2)"


def opendss_stamp(source):
    dss.Basic.ClearAll()
    for command in source.splitlines():
        dss.Text.Command(command)
    dss.Solution.Solve()
    dss.Circuit.SetActiveElement("Transformer.t")
    nodes = dss.CktElement.NodeOrder()
    ncond = dss.CktElement.NumConductors()
    values = np.asarray(dss.CktElement.YPrim()).reshape(-1, 2)
    matrix = (values[:, 0] + 1j * values[:, 1]).reshape(len(nodes), len(nodes), order="F")
    return matrix, nodes, ncond


def bmopf_stamp(document, nodes, ncond):
    expected = np.zeros((len(nodes), len(nodes)), dtype=complex)
    for subtype, table in document["transformer"].items():
        for transformer in table.values():
            shunt = transformer["no_load_shunt"]
            winding = shunt["winding"]
            if subtype == "n_winding":
                record = transformer["windings"][winding - 1]
                bus, terminals = record["bus"], record["terminal_map"]
                delta = record["configuration"] == "DELTA"
            else:
                side = "from" if winding == 1 else "to"
                bus, terminals = transformer["bus_" + side], transformer["terminal_map_" + side]
                delta = (subtype == "wye_delta" and winding == 2) or (subtype == "delta_wye" and winding == 1)
            grounded = document["bus"][bus].get("perfectly_grounded_terminals", [])
            terminal_nodes = [0 if t in grounded else int(t) for t in terminals]
            if subtype == "center_tap" and winding > 1:
                pairs = [(winding - 2, winding - 1)]
            elif delta:
                pairs = [(i, (i + 1) % len(terminals)) for i in range(len(terminals))]
            elif len(terminals) == 2:
                pairs = [(0, 1)]
            else:
                neutral = terminal_nodes.index(0)
                pairs = [(i, neutral) for i in range(len(terminals)) if i != neutral]
            winding_nodes = nodes[(winding - 1) * ncond:winding * ncond]
            for first, second in pairs:
                incidence = np.zeros(len(nodes))
                for position, sign in ((first, 1), (second, -1)):
                    local = winding_nodes.index(terminal_nodes[position])
                    incidence[(winding - 1) * ncond + local] += sign
                expected += (shunt["g"] + 1j * shunt["b"]) * np.outer(incidence, incidence)
    return expected


def run(powerio):
    results = []
    with tempfile.TemporaryDirectory(prefix="bmopf-core-shunts-") as directory:
        root = Path(directory)
        for name, phases, parameters in fixtures():
            source = f"Clear\nNew Circuit.core basekv=12.47 phases={phases} bus1=src.1.2.3.0\nNew Transformer.t phases={phases} {parameters} ppm_antifloat=0 %noloadloss=0.3 %imag=0.6\n"
            before, nodes, ncond = opendss_stamp(source.replace("%noloadloss=0.3 %imag=0.6", "%noloadloss=0 %imag=0"))
            after, _, _ = opendss_stamp(source)
            case, ir, output = root / (name + ".dss"), root / (name + ".pio.json"), root / (name + ".json")
            case.write_text(source)
            for arguments in (("serialize", str(case), "-o", str(ir)), ("convert", str(ir), "--to", "bmopf-json@0.2.0", "-o", str(output))):
                completed = subprocess.run([powerio, *arguments], text=True, capture_output=True)
                if completed.returncode:
                    raise RuntimeError(completed.stderr)
            document = json.loads(output.read_text())
            expected = bmopf_stamp(document, nodes, ncond)
            error = float(np.max(np.abs(after - before - expected)))
            scale = float(np.max(np.abs(expected)))
            tolerance = 1e-10 + 1e-9 * scale
            if error > tolerance:
                raise AssertionError(f"{name}: admittance error {error} exceeds {tolerance}")
            results.append({"case": name, "source_sha256": hashlib.sha256(source.encode()).hexdigest(), "matrix_dimension": len(nodes), "max_absolute_error_siemens": error, "absolute_tolerance_siemens": tolerance})
    return {"comparison": "OpenDSS Yprim(no-load on) minus Yprim(no-load off) versus BMOPF explicit coil stamps", "conversion": "OpenDSS -> PowerIO generation-2 IR -> explicit BMOPF proposal", "opendss": dss.Basic.Version(), "cases": results}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", required=True, help="PowerIO CLI executable")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = json.dumps(run(args.powerio), indent=2) + "\n"
    if args.output:
        args.output.write_text(result)
    else:
        print(result, end="")
