"""Check overhead definition ordering separately from shared calculation caches."""

import argparse
import hashlib
import json
import math
import re
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK

WIRE = "new wiredata.a rac=0.0002 radius=0.01 radunits=m runits=m normamps=100\nnew wiredata.b rac=0.0004 radius=0.005 radunits=m runits=m emergamps=90\n"
GEOMETRY = "new linegeometry.g nconds=2 nphases=1 reduce=yes\n~ cond=1 wire=a units=m x=0 h=10\n~ cond=2 wire=b x=1 h=9\n"


def scripts():
    variants = {
        "base": "",
        "gmr_only": "new wiredata.g rac=0.0002 gmrac=0.007 gmrunits=m runits=m\nedit linegeometry.g cond=1 wire=g\n",
        "sticky_units": "edit linegeometry.g cond=1 units=ft cond=2 x=2\n",
        "unset_units": "edit linegeometry.g nconds=2 wires=[a b] h=10 x=1 cond=1 h=11 x=0\n",
        "reset_same_size": "edit linegeometry.g nconds=2 cond=1 wire=a x=0 h=30 cond=2 wire=b x=2 h=28\n",
        "reset_resize": "edit linegeometry.g nconds=3 cond=1 wire=a x=0 h=30 cond=2 wire=a x=2 h=30 cond=3 wire=b x=1 h=28\n",
        "wire_array": "edit linegeometry.g wires=[b a] x=2 h=8\n",
        "wire_edit": "edit wiredata.a radius=0.02 rac=0.0003 normamps=200\n",
        "wire_rebind": "edit wiredata.a normamps=200 emergamps=300\nedit linegeometry.g normamps=0 emergamps=0 cond=1 wire=a\n",
        "reduce_no": "edit linegeometry.g reduce=no\n",
        "copy": "new linegeometry.c like=g\nedit linegeometry.g cond=1 x=2\n",
        "copy_units": "new linegeometry.c like=g\nedit linegeometry.c nconds=2 cond=1 wire=a x=0 h=30 cond=2 wire=b x=2 h=28\n",
        "copy_wire_live": "new linegeometry.c like=g\nedit wiredata.a rac=0.0003 radius=0.02\n",
        "historical_rating": "new wiredata.c like=a\nedit wiredata.a normamps=500\nedit linegeometry.g normamps=0 cond=1 wire=c\n",
        "geometry_abbreviations": "edit linegeometry.g cond=1 wir=b uni=ft h=30 norm=120 emerg=180 red=no\n",
    }
    spacing = "new linespacing.s nconds=2 nphases=1 units=m x=[0 2] h=[12 11]\n"
    variants.update(
        {
            "spacing": spacing + "edit linegeometry.g spacing=s\n",
            "spacing_later_edit": spacing
            + "edit linegeometry.g spacing=s\nedit linespacing.s x=[3 4] h=[13 14]\n",
            "spacing_reassign": spacing
            + "edit linegeometry.g spacing=s\nedit linespacing.s x=[3 4] h=[13 14]\nedit linegeometry.g spacing=s\n",
            "spacing_partial": spacing
            + "edit linespacing.s x=[3]\nedit linegeometry.g spacing=s\n",
            "spacing_same_size": spacing
            + "edit linespacing.s nconds=2\nedit linegeometry.g spacing=s\n",
            "spacing_shrink": "new linespacing.s nconds=3 units=m x=[0 2 3] h=[12 11 13]\nedit linespacing.s nconds=2\nedit linegeometry.g spacing=s\n",
            "spacing_grow_initialized": "new linespacing.s nconds=1 x=[0] h=[12]\nedit linespacing.s nconds=2 x=[0 2] h=[12 11]\nedit linegeometry.g spacing=s\n",
            "spacing_self_like": spacing
            + "edit linespacing.s like=s\nedit linegeometry.g spacing=s\n",
            "spacing_like": spacing
            + "new linespacing.t like=s\nedit linespacing.s x=[3 4]\nedit linegeometry.g spacing=t\n",
            "spacing_selector": spacing
            + "edit linegeometry.g cond=2 spacing=s x=3 cond=1 h=14\n",
            "spacing_keeps_geometry_phases": spacing
            + "edit linegeometry.g nphases=2 reduce=no spacing=s\n",
            "spacing_abbreviations": "new linespacing.s nc=2 np=1 x=[0 2] h=[12 11] u=m\nedit linegeometry.g spac=s\n",
        }
    )
    return {
        name: "clear\nnew circuit.c\n" + WIRE + GEOMETRY + edit
        for name, edit in variants.items()
    }


def observe(script, matrices="fresh", data_path=None):
    engine = dss.NewContext()
    for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
        getattr(engine.Basic, flag)(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("unpinned engine")
    if data_path is not None:
        engine.Basic.DataPath(str(data_path.resolve()))
    engine.Text.Commands(script)
    geometries = {}
    api = engine.LineGeometries
    for name in api.AllNames():
        api.Name(name)
        # OpenDSSDirect's enum conversion refuses native -1 (unset units).
        # Use the same C API getter without converting those integers.
        api._check_for_error(api._lib.LineGeometries_Get_Units_GR())
        units = api._get_int32_gr_array()
        data = dict(
            units=[int(v) for v in units],
            wires=[
                v.removeprefix("WireData.")
                .removeprefix("wiredata.")
                .removeprefix("CNData.")
                .removeprefix("cndata.")
                for v in api.Conductors()
            ],
            x=list(api.Xcoords()),
            height=list(api.Ycoords()),
            phases=api.Phases(),
            reduce=api.Reduce(),
            normamps=api.NormAmps(),
            emergamps=api.EmergAmps(),
        )
        engine.Text.Command(f"? linegeometry.{name}.cond")
        data["active"] = int(engine.Text.Result())
        if matrices:
            # Each synthetic case has one geometry and no earlier calculation.
            # MakeLike cases are definition-only: MakeLike itself computes and
            # shares cached values, which this comparison does not qualify.
            if matrices == "fresh":
                engine.Text.Command(
                    f"new line._oracle bus1=sourcebus bus2=end geometry={name} length=1 units=m earthmodel=carson"
                )
                engine.Text.Command("set controlmode=off")
                engine.Text.Command("solve")
                if not engine.Solution.Converged():
                    raise ValueError("native geometry circuit did not converge")
            api.Name(name)
            n = data["phases"] if data["reduce"] else len(data["wires"])
            data["matrices"] = {
                "r": np.array(api.Rmatrix(60, 1, 4))
                .reshape((n, n), order="F")
                .tolist(),
                "x": np.array(api.Xmatrix(60, 1, 4))
                .reshape((n, n), order="F")
                .tolist(),
                "b": (
                    np.array(api.Cmatrix(60, 1, 4)).reshape((n, n), order="F")
                    * 1e-9
                    * math.tau
                    * 60
                ).tolist(),
            }
        geometries[name] = data
    return dict(
        script=script,
        script_sha256=hashlib.sha256(script.encode()).hexdigest(),
        engine=engine.Basic.Version(),
        geometries=geometries,
    )


def public_ieee4(base, output):
    catalog = json.loads(Path(__file__).with_name("ieee-corpus.json").read_text())
    family = next(f for f in catalog["families"] if f["id"] == "ieee4")
    source = next(s for s in family["sources"] if s["repository"].endswith("/CIMHub"))
    results = {}
    for record in source["files"]:
        if not record["path"].lower().endswith(".dss"):
            continue
        path = base / Path(record["path"]).name
        content = path.read_bytes()
        blob = hashlib.sha1(f"blob {len(content)}\0".encode() + content).hexdigest()
        if len(content) != record["bytes"] or blob != record["git_blob_sha1"]:
            raise ValueError(f"public source differs from pinned catalog: {path}")
        script = content.decode("utf-8-sig")
        if not re.search(r"(?im)^\s*new\s+linegeometry\.", script):
            continue
        data_path = output / path.stem
        data_path.mkdir()
        result = observe(script, matrices="existing", data_path=data_path)
        result["source"] = dict(
            repository=source["repository"], commit=source["commit"], **record
        )
        results[path.name] = result
    if not results:
        raise ValueError("no public geometry definitions observed")
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--ieee4-base",
        type=Path,
        help="external pinned CIMHub ieee4/base directory; select public geometry cases instead of synthetic cases",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    if args.ieee4_base is not None:
        results = public_ieee4(args.ieee4_base, args.output)
    else:
        results = {
            name: observe(script, matrices="fresh" if "copy" not in name else None)
            for name, script in scripts().items()
        }
    (args.output / "definitions.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        f"Captured {len(results)} native geometry definition cases; Rust comparison is required separately."
    )


if __name__ == "__main__":
    main()
