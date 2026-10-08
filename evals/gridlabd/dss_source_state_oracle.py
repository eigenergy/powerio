"""Capture pinned native Vsource states for the Rust assignment-order resolver.

Generated synthetic cases exercise constructor/edit recalculation, sequence
specifications, phasors, source/reference bus strings and snapshot frequency.
The native primitive comes from YPrim; emf comes from measured port voltages
and currents plus that primitive. No PowerIO interpretation supplies results.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK

EXPECTED_REJECTIONS = {
    "mva_then_r1_edit",
    "z_then_mva",
    "mva_then_isc",
    "pu_then_mva",
}


def cases():
    specs = {
        "default": "",
        "r_x": "r1=0.8 x1=1.7 r0=2.1 x0=3.4",
        "z_equal": "z1=[0.8 1.7] z0=[2.1 3.4]",
        "z_distinct": "z1=[0.8 1.7] z2=[1.2 2.3] z0=[2.1 3.4]",
        "pu_z": "basemva=5 puz1=[0.08 0.17] puz0=[0.21 0.34] puz2=[0.12 0.23]",
        "mva": "mvasc3=25 mvasc1=30 x1r1=5 x0r0=4",
        "current": "isc3=1000 isc1=1200 x1r1=5 x0r0=4",
        "quasi": "model=ideal puzideal=[0.01 0.02]",
    }
    for phases in (1, 2, 3, 4):
        for label, properties in specs.items():
            for f in (50, 60):
                yield (
                    f"{label}/{phases}/{f}",
                    f,
                    f,
                    "source",
                    f"New Circuit.c phases={phases} bus1=feed basekv=12.47 {properties}\n",
                )
    sequences = {"negative": "negative", "zero": "zero", "positive": "positive"}
    for name, sequence in sequences.items():
        yield (
            name,
            60,
            60,
            "source",
            f"New Circuit.c phases=3 bus1=feed basekv=0.48 angle=13 pu=0.97 sequence={sequence} z1=[1 2]\n",
        )
    edits = {
        "z0_before_z1": "z0=[4 5] z1=[1 2]",
        "z2_before_z1": "z2=[4 5] z1=[1 2]",
        "z2_then_r1": "z1=[1 2] z2=[4 5] r1=7",
        "z2_then_x1": "z1=[1 2] z2=[4 5] x1=7",
        "pu_then_r1": "basemva=2 puz1=[0.02 0.03]\nEdit Vsource.source r1=7",
        "pu0_only": "puz0=[0.02 0.03]",
        "mva_then_r1_same": "mvasc3=25 r1=7",
        "mva_then_r1_edit": "mvasc3=25\nEdit Vsource.source r1=7",
        "z_then_mva": "z1=[1 2] z2=[4 5] mvasc3=25",
        "mva_then_isc": "mvasc3=25\nEdit Vsource.source isc1=1200",
        "isc_then_mva": "isc1=1200\nEdit Vsource.source mvasc3=25",
        "pu_then_mva": "puz1=[0.02 0.03] mvasc3=25\nEdit Vsource.source r1=7",
        "kv_after_pu": "puz1=[0.02 0.03] basekv=4.16",
        "kv_edit_after_pu": "puz1=[0.02 0.03]\nEdit Vsource.source basekv=4.16",
        "negative_pu": "pu=-0.5 z1=[1 2]",
        "zero_pu": "pu=0 z1=[1 2]",
        "return_then_bus1": "bus2=return.3.2.1 bus1=feed.1.3.2 z1=[1 2]",
        "bus1_then_phases": "bus1=feed phases=4 z1=[1 2]",
        "explicit_basefreq": "basefreq=50 z1=[1 2]",
        "frequency_mismatch": "frequency=50 z1=[1 2]",
        "frequency_near": "frequency=60.0005 z1=[1 2]",
    }
    for name, body in edits.items():
        yield (
            name,
            60,
            60,
            "source",
            f"New Circuit.c phases=3 bus1=feed basekv=12.47 {body}\n",
        )
    # Keep the invalid intermediate-state cases above: setters in a later
    # Edit cannot repair a New that already failed native recalculation.
    for name in ("mva_then_r1_edit", "z_then_mva", "mva_then_isc", "pu_then_mva"):
        body = edits[name].replace("mvasc3=25", "mvasc3=25 mvasc1=30")
        yield (
            "valid_" + name,
            60,
            60,
            "source",
            f"New Circuit.c phases=3 bus1=feed basekv=12.47 {body}\n",
        )
    for operating in (50, 70):
        yield (
            f"quasi_frequency/{operating}",
            60,
            operating,
            "source",
            "New Circuit.c phases=3 bus1=feed basekv=12.47 model=ideal puzideal=[0.01 0.02]\n",
        )
    yield (
        "like_state",
        60,
        60,
        "copy",
        "New Circuit.c phases=3 bus1=feed basekv=12.47 puz1=[0.02 0.03]\nNew Vsource.copy like=source bus1=copyfeed r1=7\n",
    )
    for key, before in (("z_flag", "z2=[9 9]"), ("pu_flag", "puz1=[0.1 0.2]")):
        yield (
            "like_reset_" + key,
            60,
            60,
            "copy",
            f"New Circuit.c phases=3 bus1=feed basekv=12.47 z1=[1 2]\nNew Vsource.copy {before} like=source bus1=copyfeed\n",
        )
    replay = {
        "constructor_frequency": (
            50,
            "New Circuit.c bus1=feed z1=[1 2]\nSet DefaultBaseFrequency=50\nNew Vsource.copy phases=1 bus1=copyfeed basekv=0.24 z1=[1 2]\n",
        ),
        "like_constructor_frequency": (
            60,
            "New Circuit.c bus1=feed z1=[1 2]\nSet DefaultBaseFrequency=50\nNew Vsource.copy like=source bus1=copyfeed\n",
        ),
        "like_nested": (
            60,
            "New Circuit.c bus1=feed z1=[1 2]\nNew Vsource.mid puz1=[0.1 0.2] like=source bus1=middle\nNew Vsource.copy z2=[9 9] like=mid bus1=copyfeed\n",
        ),
        "like_empty": (
            60,
            "New Circuit.c\nNew Vsource.copy z2=[9 9] like=source bus1=copyfeed\n",
        ),
        "like_empty_after_edit": (
            60,
            "New Circuit.c\nNew Vsource.copy z2=[9 9] bus1=copyfeed\nEdit Vsource.copy like=source bus1=copyfeed\n",
        ),
        "like_snapshot": (
            60,
            "New Circuit.c bus1=feed z1=[1 2]\nNew Vsource.copy like=source bus1=copyfeed\nEdit Vsource.source z1=[3 4]\n",
        ),
    }
    for name, (operating, text) in replay.items():
        yield name, 60, operating, "copy", text
    yield (
        "like_self_in_edit",
        60,
        60,
        "source",
        "New Circuit.c bus1=feed z1=[1 2]\nEdit Vsource.source r1=0 x1=0 like=source z1=[1 2]\n",
    )


def complex_array(raw):
    return np.array(raw[::2]) + 1j * np.array(raw[1::2])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}
    for name, constructor, operating, target, script in cases():
        d = dss.NewContext()
        d.Basic.AllowChangeDir(False)
        version = d.Basic.Version()
        if LOCK["dss"]["version"] not in version:
            raise ValueError("unpinned native engine")
        deck = f"clear\nset defaultbasefrequency={constructor}\n" + script
        result = {
            "script": script,
            "constructor_frequency": constructor,
            "snapshot_frequency": operating,
            "target": target,
            "version": version,
            "deck_sha256": hashlib.sha256(deck.encode()).hexdigest(),
        }
        if name in EXPECTED_REJECTIONS:
            result["expected_error"] = "cannot calculate a real-valued R0"
        try:
            for line in deck.splitlines():
                d.Text.Command(line)
            d.Circuit.SetActiveElement("vsource." + target)
            buses = d.CktElement.BusNames()
            n = d.CktElement.NumPhases()
            # Anchor reference conductors that would otherwise float. EMFs
            # are recovered from both measured ports even when bus1/bus2
            # overlap, or another source shares a return conductor.
            base = buses[1].split(".")[0]
            suffix = [int(v) for v in buses[1].split(".")[1:]]
            for i, node in enumerate(suffix):
                if node:
                    d.Text.Command(
                        f"New Line.anchor{i} phases=1 bus1={base}.{node} bus2=ground.0 r1=1 x1=0 r0=1 x0=0 c1=0 c0=0 length=1 units=none"
                    )
            d.Text.Command(
                f"set frequency={operating} controlmode=off tolerance=1e-12 maxiterations=100"
            )
            d.Text.Command("solve")
            if not d.Solution.Converged():
                raise ValueError("native source circuit did not converge")
            d.Circuit.SetActiveElement("vsource." + target)
            y = complex_array(d.CktElement.YPrim()).reshape(2 * n, 2 * n, order="F")
            z = np.linalg.inv(y[:n, :n])
            volts = complex_array(d.CktElement.Voltages())
            current = complex_array(d.CktElement.Currents())
            emf = volts[:n] - volts[n:] - z @ current[:n]

            def pairs(v):
                return np.stack((v.real, v.imag), axis=-1).tolist()

            result.update(
                bus1=buses[0],
                bus2=buses[1],
                emf=pairs(emf),
                impedance=pairs(z),
                phases=n,
            )
        except Exception as error:
            result["error"] = str(error)
        report["cases"][name] = result
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        len(report["cases"]),
        "states;",
        sum("error" in v for v in report["cases"].values()),
        "native errors",
    )
    return int(
        any(
            v["expected_error"] not in v.get("error", "")
            if "expected_error" in v
            else "error" in v
            for v in report["cases"].values()
        )
    )


if __name__ == "__main__":
    raise SystemExit(main())
