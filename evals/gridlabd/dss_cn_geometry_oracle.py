"""Native CN cable matrix and definition evidence, before public feeder checks."""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import opendssdirect as dss
from dss_geometry_definition_oracle import observe
from dss_overhead_geometry_oracle import deck, wires
from oracle import LOCK


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
                (3, 2, True),
            ):
                conductors = wires(count, False)
                for i, wire in enumerate(conductors):
                    wire.update(x=i * 0.1, height=-1.0 - 0.05 * (i % 2))
                    if i < phases:
                        wire["cable"] = dict(
                            k=13 + i,
                            diastrand=0.0015,
                            gmrstrand=0.0006,
                            rstrand=0.0008,
                            epsr=2.3 + i * 0.1,
                            inslayer=0.004,
                            diains=0.026,
                            diacable=0.031,
                        )
                key = f"{model}-{frequency}-{count}-{phases}-{reduced}"
                geometry = dict(
                    wires=conductors,
                    phases=phases,
                    reduce=reduced,
                    earth_model=model,
                    rho=37.0,
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
                engine.LineGeometries.Name("g")
                n = phases if reduced else count
                data = dict(
                    # Matrix qualification does not require the unloaded circuit to solve.
                    native_unloaded_converged=engine.Solution.Converged(),
                    geometry=geometry,
                    frequency=frequency,
                    engine=engine.Basic.Version(),
                    script_sha256=hashlib.sha256(script.encode()).hexdigest(),
                )
                for name, getter, scale in (
                    ("r", engine.LineGeometries.Rmatrix, 1),
                    ("x", engine.LineGeometries.Xmatrix, 1),
                    ("b", engine.LineGeometries.Cmatrix, 1e-9 * math.tau * frequency),
                ):
                    data[name] = (
                        np.array(getter(frequency, 1, 4)).reshape((n, n), order="F")
                        * scale
                    ).tolist()
                cases[key] = data
    (output / "matrices.json").write_text(json.dumps(cases, indent=2) + "\n")
    definitions = (
        "new cndata.a rac=.1214 diam=1.124 gmrunits=ft runits=mi radunits=in normamps=615\n"
        "~ diacable=2.08 k=21 diastrand=.1285 inslayer=.25 diains=1.64 rstrand=.1809\n"
        "new cndata.b like=a gmrac=.04683 gmrstrand=.00417\n"
        "new wiredata.n radius=.005 rac=.0002 radunits=m runits=m\n"
        "new linespacing.s nconds=3 nphases=3 units=in x=[0 3 6] h=[-48 -48 -48]\n"
        "new linegeometry.g nconds=3 nphases=3 spacing=s cncables=[b b b]\n"
    )
    variants = {
        "base": "",
        "cn_defaults_and_sticky_units": "edit linegeometry.g cncables=[a a a]\n",
        "cn_copy_snapshot": "edit cndata.a rac=.3 diastrand=.15\n",
        "cn_self_copy": "edit cndata.b like=b\n",
        "cn_live_edit": "edit cndata.b rac=.3 diastrand=.15\n",
        "cn_array_selector": "edit linegeometry.g cond=2 cncables=[b a b] x=3.5\n",
        "cn_single_selector": "edit linegeometry.g cond=2 cncable=a x=3.5\n",
        "cn_ratings": "edit cndata.b normamps=700\nedit linegeometry.g normamps=0 cncables=[b b b]\n",
        "cn_copy": "new linegeometry.c like=g\n",
        "cn_bare_neutral": "edit linegeometry.g nconds=4 cond=1 cncable=b units=in x=0 h=-48 cond=2 cncable=b x=3 h=-48 cond=3 cncable=b x=6 h=-48 wires=[n] cond=4 x=9 h=-48 reduce=yes\n",
    }
    records, errors = {}, {}
    for name, edit in variants.items():
        script = "clear\nnew circuit.c\n" + definitions + edit
        try:
            records[name] = observe(script, matrices="fresh")
        except Exception as exc:
            if name != "cn_copy":
                raise
            errors[name] = dict(script=script, native_error=str(exc))
    (output / "native-errors.json").write_text(json.dumps(errors, indent=2) + "\n")
    (output / "definitions.json").write_text(json.dumps(records, indent=2) + "\n")
    print(
        f"Captured {len(cases)} matrices and {len(records)} definitions; Rust comparison is required."
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    generate(parser.parse_args().output)
