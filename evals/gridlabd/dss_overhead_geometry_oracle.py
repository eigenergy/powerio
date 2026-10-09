"""Pinned native overhead impedance/admittance matrices for the staged evaluator.

This is geometry-kernel evidence, not PowerIO feeder-import qualification.
Wire values are explicit SI data; ordered definition/default/cache behavior is
validated separately before the public reader can activate geometry support.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK


def wires(count, skin):
    return [
        dict(
            x=float(i * 2 - 3),
            height=10.0 - (i % 2),
            radius=0.008 + i * 0.0005,
            capacitance_radius=0.01 + i * 0.0005,
            gmr=0.006 + i * 0.0003,
            rdc=(1e-6 if skin else 0.0002) * (1 + i / 5),
            rac=0.00021 * (1 + i / 5),
        )
        for i in range(count)
    ]


def deck(geometry, frequency):
    text = [
        "clear",
        f"set defaultbasefrequency={frequency:.17g}",
        "new circuit.c basekv=12.47",
        f"set earthmodel={geometry['earth_model']}",
    ]
    for i, wire in enumerate(geometry["wires"]):
        text.append(
            f"new wiredata.w{i} runits=m gmrunits=m radunits=m "
            f"rdc={wire['rdc']:.17g} rac={wire['rac']:.17g} gmrac={wire['gmr']:.17g} "
            f"radius={wire['radius']:.17g} capradius={wire['capacitance_radius']:.17g}"
        )
    text.append(
        f"new linegeometry.g nconds={len(geometry['wires'])} nphases={geometry['phases']} reduce={'yes' if geometry['reduce'] else 'no'}"
    )
    for i, wire in enumerate(geometry["wires"]):
        text.append(
            f"~ cond={i + 1} wire=w{i} units=m x={wire['x']:.17g} h={wire['height']:.17g}"
        )
    text += [
        f"new line.l bus1=sourcebus bus2=end geometry=g length=1 units=m rho={geometry['rho']:.17g}",
        "set controlmode=off",
        "solve",
    ]
    return "\n".join(text) + "\n"


def generate(output):
    output.mkdir(parents=True, exist_ok=False)
    cases = {}
    for model in ("carson", "fullcarson", "deri"):
        for frequency in (40.0, 50.0, 60.0, 1000.0, 3000.0):
            for count, phases, reduced in (
                (1, 1, False),
                (3, 3, False),
                (4, 3, False),
                (4, 3, True),
                (5, 3, True),
            ):
                for skin in (False, True):
                    key = f"{model}-{frequency}-{count}-{phases}-{reduced}-{skin}"
                    geometry = dict(
                        wires=wires(count, skin),
                        phases=phases,
                        reduce=reduced,
                        earth_model=model,
                        rho=100.0 if not skin else 37.0,
                    )
                    script = deck(geometry, frequency)
                    path = output / f"{key}.dss"
                    path.write_text(script)
                    engine = dss.NewContext()
                    for flag in (
                        "AllowChangeDir",
                        "AllowDOScmd",
                        "AllowEditor",
                        "AllowForms",
                    ):
                        getattr(engine.Basic, flag)(False)
                    if LOCK["dss"]["version"] not in engine.Basic.Version():
                        raise ValueError("unpinned engine")
                    engine.Text.Command(f'redirect "{path.resolve()}"')
                    if not engine.Solution.Converged():
                        raise ValueError(
                            f"native geometry circuit did not converge: {key}"
                        )
                    engine.LineGeometries.Name("g")
                    n = phases if reduced else count
                    resistance = np.array(
                        engine.LineGeometries.Rmatrix(frequency, 1.0, 4)
                    ).reshape((n, n), order="F")
                    reactance = np.array(
                        engine.LineGeometries.Xmatrix(frequency, 1.0, 4)
                    ).reshape((n, n), order="F")
                    capacitance = np.array(
                        engine.LineGeometries.Cmatrix(frequency, 1.0, 4)
                    ).reshape((n, n), order="F")
                    cases[key] = dict(
                        geometry=geometry,
                        frequency=frequency,
                        r=resistance.tolist(),
                        x=reactance.tolist(),
                        b=(capacitance * 1e-9 * math.tau * frequency).tolist(),
                        engine=engine.Basic.Version(),
                        script_sha256=hashlib.sha256(script.encode()).hexdigest(),
                    )
    (output / "matrices.json").write_text(json.dumps(cases, indent=2) + "\n")
    print(
        f"Captured {len(cases)} pinned native matrices; Rust comparison is a separate required step."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    generate(args.output.resolve())


if __name__ == "__main__":
    main()
