"""Observe native shared geometry and per-Line cache history (one-phase probes)."""

import argparse
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK

DEFINITIONS = """new wiredata.w rac=0.0002 radius=0.01 radunits=m runits=m
new linegeometry.g nconds=1 nphases=1 cond=1 wire=w x=0 h=10 units=m
"""
LINES = """new line.a bus1=sourcebus.1 bus2=a.1 geometry=g length=100 units=m earthmodel=carson
new line.b bus1=sourcebus.1 bus2=b.1 geometry=g length=100 units=m earthmodel=deri
set controlmode=off
solve
"""
BASE = "clear\nnew circuit.c basekv=12.47\n" + DEFINITIONS + LINES


def cases():
    result = {
        "wire_geometry_line": (
            BASE,
            [
                "edit wiredata.w rac=0.0004",
                "edit linegeometry.g h=11",
                "edit line.a length=100",
                "edit line.b geometry=g",
            ],
        ),
        "earth_reassign": (
            BASE,
            [
                "edit line.a earthmodel=deri",
                "edit line.a geometry=g",
                "edit linegeometry.g h=11",
                "edit line.a geometry=g",
            ],
        ),
        "length_units": (
            BASE,
            [
                "edit line.a length=200",
                "edit line.a units=ft",
                "edit line.a geometry=g",
            ],
        ),
        "frequency": (BASE, ["set frequency=50", "set frequency=60"]),
        "rho_shared": (
            BASE,
            ["edit line.b rho=37", "edit line.a geometry=g", "edit line.b geometry=g"],
        ),
        "copy_after_solve": (
            BASE,
            ["new linegeometry.h like=g", "edit line.a geometry=h"],
        ),
        "line_copy": (
            BASE,
            [
                "edit line.b length=200 geometry=g",
                "edit line.a like=b",
                "edit line.a length=150",
            ],
        ),
        "sequence_override": (
            BASE,
            ["edit line.a r1=0.02 x1=0.01 c1=0", "edit line.a geometry=g"],
        ),
        "linecode_override": (
            BASE,
            [
                "new linecode.lc nphases=1 rmatrix=[0.02] xmatrix=[0.01] cmatrix=[0] units=km\nedit line.a linecode=lc",
                "edit line.a geometry=g",
            ],
        ),
    }
    result["copy_before_line"] = (
        "clear\nnew circuit.c basekv=12.47\n"
        + DEFINITIONS
        + "new linegeometry.h like=g\nedit wiredata.w rac=0.0004\n"
        + LINES.replace("geometry=g", "geometry=h"),
        ["edit linegeometry.h h=11", "edit line.a geometry=h"],
    )
    result["clear_active_earth"] = (
        BASE.replace("earthmodel=deri", "earthmodel=carson"),
        [
            "clear\nnew circuit.d basekv=12.47\n"
            + DEFINITIONS
            + "new linegeometry.h like=g\n"
            + LINES.replace("geometry=g", "geometry=h").replace(
                "earthmodel=carson", "earthmodel=deri"
            )
        ],
    )
    result["default_earth"] = (
        "clear\nnew circuit.c basekv=12.47\nset earthmodel=carson\n"
        + DEFINITIONS
        + LINES.replace(" earthmodel=carson", ""),
        ["set earthmodel=fullcarson", "edit line.a geometry=g"],
    )
    cn = (
        "new cndata.c rac=.0002 radius=.01 radunits=m runits=m k=13 "
        "diastrand=.0015 rstrand=.0008 inslayer=.004 diains=.028 diacable=.033\n"
    )
    cn_base = (
        "clear\nnew circuit.c basekv=12.47\n"
        + cn
        + "new linegeometry.g nconds=1 nphases=1 cond=1 cncable=c x=0 h=-1 units=m\n"
        + LINES
    )
    result["cn_live_definition"] = (
        cn_base,
        [
            "edit cndata.c rac=.0004 diastrand=.002",
            "edit linegeometry.g h=-1.1",
            "edit line.a geometry=g",
            "edit line.b geometry=g",
        ],
    )
    result["cn_frequency_and_rho"] = (
        cn_base,
        [
            "set frequency=50",
            "edit line.b rho=37",
            "edit line.a geometry=g",
            "set frequency=60",
        ],
    )
    result["cn_reset_same_size"] = (
        cn_base,
        [
            "edit linegeometry.g nconds=1 cond=1 cncable=c x=0 h=-2 units=m",
            "edit line.a geometry=g",
            "edit line.b geometry=g",
        ],
    )
    result["overhead_to_cn"] = (
        BASE,
        [
            cn + "edit linegeometry.g cond=1 cncable=c h=-1",
            "edit line.a geometry=g",
            "edit line.b geometry=g",
        ],
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
            primitives = {}
            for line in ("a", "b"):
                engine.Circuit.SetActiveElement("Line." + line)
                primitives[line] = list(engine.CktElement.YPrim())
            observations.append(dict(command=command, primitives=primitives))
        results[name] = dict(
            base=base, engine=engine.Basic.Version(), observations=observations
        )
    (args.output / "observations.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        f"Captured {len(results)} native histories; run the Rust comparison separately."
    )


if __name__ == "__main__":
    main()
