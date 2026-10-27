"""Observe ordered WireData defaults and MakeLike in the pinned native engine.

This checks conductor definition state only; feeder electrical qualification
also needs LineGeometry binding and canonical circuit comparisons.
"""

import argparse
import hashlib
import itertools
import json
from pathlib import Path

import opendssdirect as dss
from oracle import LOCK

FIELDS = {
    "rdc": "Rdc",
    "rac": "Rac",
    "runits": "ResistanceUnits",
    "gmr": "GMRac",
    "gmrunits": "GMRUnits",
    "radius": "Radius",
    "radunits": "RadiusUnits",
    "capacitance_radius": "CapRadius",
    "normamps": "NormAmps",
    "emergamps": "EmergAmps",
}


def scripts():
    cases = {
        "empty": "new wiredata.a",
        "radius_only": "new wiredata.a radius=0.1",
        "gmr_only": "new wiredata.a gmrac=0.1",
        "rdc_first": "new wiredata.a rdc=0.2 rac=0.3 rdc=0.4",
        "rac_first": "new wiredata.a rac=0.3 rdc=0.2 rac=0.4",
        "rating_normal": "new wiredata.a normamps=100 emergamps=170 normamps=110",
        "rating_emergency": "new wiredata.a emergamps=150 normamps=110 emergamps=180",
        "diameter_sticky": "new wiredata.a diam=0.2 diam=0.4 radius=0.5",
        "explicit_cap": "new wiredata.a capradius=0.3 radius=0.2 radius=0.4",
        "copy": "new wiredata.a radius=0.2 rac=0.3 radunits=cm\nnew wiredata.b radius=4 like=a\nedit wiredata.a radius=0.5 rac=0.7\nedit wiredata.b radius=0.4",
        "empty_copy": "new wiredata.a\nnew wiredata.b radius=0.2 like=a rac=0.3",
        "self_copy": "new wiredata.a radius=0.2 rac=0.3\nedit wiredata.a like=a radius=0.4",
        "positional": "new wiredata.a 0.2 0.3 km 0.008 m 0.01 m 100 160 0.03 1 [120] 0.04",
        "abbreviated": "new wiredata.a rac=0.2 run=km gmra=0.008 gmru=m rad=0.01 radu=m norm=100 emerg=170 capr=0.012",
        "rpn": "new wiredata.a rac=(0.2 2 *) radius=(0.01 2 *) radunits=m",
        "negative_backfill": "new wiredata.a rac=-1 rdc=0.2 normamps=-1 emergamps=150",
    }
    setters = ("gmrac=0.007", "radius=0.01", "gmrunits=ft", "radunits=in")
    for i, order in enumerate(itertools.permutations(setters)):
        cases[f"order_{i}"] = "new wiredata.a " + " ".join(order)
    for unit in ("none", "mi", "kft", "km", "m", "ft", "in", "cm", "mm"):
        cases[f"unit_{unit}"] = (
            f"new wiredata.a rac=0.3 runits={unit} radius=0.01 radunits={unit}"
        )
    return {
        name: "clear\nnew circuit.c\n" + script + "\n" for name, script in cases.items()
    }


def observe(script):
    engine = dss.NewContext()
    for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
        getattr(engine.Basic, flag)(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("unpinned engine")
    engine.Text.Commands(script)
    wires = {}
    for name in engine.WireData.AllNames():
        engine.WireData.Name(name)
        wires[name] = {
            key: getattr(engine.WireData, getter)() for key, getter in FIELDS.items()
        }
    if not wires:
        raise ValueError("no native wires observed")
    return {
        "script": script,
        "script_sha256": hashlib.sha256(script.encode()).hexdigest(),
        "engine": engine.Basic.Version(),
        "wires": wires,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    results = {name: observe(script) for name, script in scripts().items()}
    (args.output / "wires.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        f"Captured {len(results)} native definition cases; run the Rust comparison separately."
    )


if __name__ == "__main__":
    main()
