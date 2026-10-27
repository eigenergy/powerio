"""Native direct Line.spacing history, electrical primitives and current ratings."""

import argparse
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK

DEFINITIONS = """new wiredata.w rac=.0002 radius=.01 radunits=m runits=m normamps=100
new wiredata.n rac=.0004 radius=.005 radunits=m runits=m normamps=70
new linespacing.s nconds=2 nphases=1 units=m x=[0 1] h=[10 9]
new cndata.c rac=.0002 radius=.01 radunits=m runits=m normamps=150 k=13 diastrand=.0015 rstrand=.0008 inslayer=.004 diains=.028 diacable=.033
"""
LINES = """new line.a bus1=sourcebus.1 bus2=a.1 spacing=s wires=[w n] length=100 units=m earthmodel=carson
new line.b bus1=sourcebus.1 bus2=b.1 spacing=s wires=[w n] length=100 units=m earthmodel=deri
set controlmode=off
solve
"""
BASE = "clear\nnew circuit.c basekv=12.47\n" + DEFINITIONS + LINES


def cases():
    result = {
        "overhead": (BASE, []),
        "live_wire": (
            BASE,
            [
                "edit wiredata.w rac=.0005 normamps=200",
                "edit line.a length=200",
                "set frequency=50",
            ],
        ),
        "live_spacing": (
            BASE,
            [
                "edit linespacing.s x=[0 2] h=[15 14]",
                "edit line.a spacing=s wires=[w n]",
                "set frequency=50",
            ],
        ),
        "wire_reassign": (BASE, ["edit line.a wires=[n w]", "set frequency=50"]),
        "spacing_reassign": (
            BASE,
            [
                "new linespacing.t like=s h=[20 19]\nedit line.a spacing=t wires=[w n]",
                "set frequency=50",
            ],
        ),
        "rho": (
            BASE,
            ["edit line.a rho=37", "edit line.a length=101", "set frequency=50"],
        ),
        "earth": (
            BASE,
            [
                "edit line.a earthmodel=fullcarson",
                "set frequency=50",
                "set frequency=60",
            ],
        ),
        "units": (BASE, ["edit line.a units=ft", "set frequency=50"]),
        "ratings": (
            BASE,
            [
                "edit line.a normamps=300 emergamps=500",
                "set frequency=50",
                "edit line.a wires=[w n]",
                "set frequency=60",
            ],
        ),
        "line_copy": (
            BASE,
            [
                "edit line.b length=200",
                "edit line.a like=b",
                "edit line.a length=150",
                "set frequency=50",
            ],
        ),
        "sequence_override": (
            BASE,
            ["edit line.a r1=.02 x1=.01 c1=0", "edit line.a spacing=s wires=[w n]"],
        ),
        "linecode_override": (
            BASE,
            [
                "new linecode.c nphases=1 rmatrix=[.02] xmatrix=[.01] cmatrix=[0] units=km\nedit line.a linecode=c",
                "edit line.a spacing=s wires=[w n]",
            ],
        ),
    }
    for rho in (100, 37):
        result[f"initial_rho_{rho}"] = (
            BASE.replace("length=100", f"rho={rho} length=100"),
            [],
        )
    result["live_before_first_calculation"] = (
        BASE.replace(
            "set controlmode=off",
            "edit wiredata.w rac=.0006\nedit linespacing.s h=[12 11]\nset controlmode=off",
        ),
        [],
    )
    result["geometry_to_spacing"] = (
        BASE.replace(
            "new line.a",
            "new linegeometry.g nconds=1 nphases=1 cond=1 wire=w x=0 h=10 units=m\nnew line.a",
        ).replace("bus2=a.1 spacing=s wires=[w n]", "bus2=a.1 geometry=g"),
        ["edit line.a spacing=s wires=[w n]"],
    )
    result["spacing_to_geometry"] = (
        BASE,
        [
            "new linegeometry.g nconds=1 nphases=1 cond=1 wire=w x=0 h=10 units=m\nedit line.a geometry=g",
            "edit line.a spacing=s wires=[w n]",
        ],
    )
    cn = BASE.replace("x=[0 1] h=[10 9]", "x=[0 .1] h=[-1 -1]").replace(
        "wires=[w n]", "cncables=[c] wires=[n]"
    )
    result["cn_bare_neutral"] = (
        cn,
        ["edit cndata.c rstrand=.001 normamps=250", "set frequency=50"],
    )
    result["cn_full_array_then_neutral"] = (
        cn.replace("cncables=[c]", "cncables=[c c]"),
        [],
    )
    result["cn_only"] = (
        cn.replace("nconds=2", "nconds=1")
        .replace("x=[0 .1] h=[-1 -1]", "x=[0] h=[-1]")
        .replace(" wires=[n]", ""),
        ["set frequency=50"],
    )
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    results = {}
    for name, (base, commands) in cases().items():
        engine = dss.NewContext()
        for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
            getattr(engine.Basic, flag)(False)
        if LOCK["dss"]["version"] not in engine.Basic.Version():
            raise ValueError("unpinned engine")
        engine.Basic.DataPath(str(args.output.resolve()))
        engine.Text.Commands(base)
        observations = []
        for command in ["initial", *commands]:
            if command != "initial":
                engine.Text.Commands(command + "\nsolve\n")
            if not engine.Solution.Converged():
                raise ValueError(f"native solve failed: {name}/{command}")
            primitives, ratings = {}, {}
            for line in ("a", "b"):
                engine.Lines.Name(line)
                primitives[line] = list(engine.CktElement.YPrim())
                ratings[line] = [engine.Lines.NormAmps(), engine.Lines.EmergAmps()]
            observations.append(
                dict(command=command, primitives=primitives, ratings=ratings)
            )
        results[name] = dict(
            base=base, engine=engine.Basic.Version(), observations=observations
        )
    (args.output / "observations.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        f"Captured {len(results)} native direct-spacing histories; Rust comparison is required."
    )


if __name__ == "__main__":
    main()
