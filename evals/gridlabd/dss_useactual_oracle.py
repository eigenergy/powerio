"""Native static LoadShape UseActual assignment and canonical power-flow checks."""
import argparse
import hashlib
import json
import struct
from pathlib import Path

from ieee4_import_oracle import evaluate, run

SHAPE = "new loadshape.s npts=3 mult=[5 7 3] qmult=[9 2 99] useactual=yes\n"
LOAD = "new load.l phases=1 bus1=end.1.0 kv=.24 kw=4 kvar=1 "


def cases(folder):
    for suffix in ("daily=s", "yearly=s", "duty=s", "pf=.9 daily=s",
                   "daily=s pf=.9", "pf=.9 kvar=1 daily=s", "daily=s kw=6", "daily=s kvar=3"):
        yield suffix.replace("=", "").replace(" ", "-"), SHAPE + LOAD + suffix + "\n"
    for label, shape in {
        "negative": "mult=[5 -7 3] qmult=[9 -2 99]",
        "tie": "mult=[-7 7 3] qmult=[2 4 9]",
        "zero": "mult=[0 0 0] qmult=[2 4 9]",
        "short-inline": "mult=[5 7] qmult=[9 2]",
        "q-before-p": "qmult=[9 2 99] mult=[5 7 3]",
        "no-q": "mult=[5 7 3]",
        "qmax": "mult=[5 7 3] qmult=[9 2 99] qmax=4",
        "pmax-with-data": "mult=[5 7 3] qmult=[9 2 99] pmax=12",
        "only-peaks": "pmax=12 qmax=4",
        "not-actual": "mult=[5 7 3] useactual=no",
        "rpn-vars": "mult=[5 (@p 2 *) 3] qmult=[9 2 99]",
    }.items():
        yield label, "var @p=3.5\nnew loadshape.s npts=3 useactual=yes " + shape + "\n" + LOAD + "daily=s\n"
    for label, tail in {
        "shape-edited-after": "edit loadshape.s mult=[10 20 30] qmult=[2 4 6]",
        "shape-rebound": "edit loadshape.s mult=[10 20 30] qmult=[2 4 6]\nedit load.l daily=s",
        "second-shape": "new loadshape.t npts=3 mult=[10 20 30] qmult=[2 4 6] useactual=yes\nedit load.l yearly=t",
        "copy-load-fresh": "new load.copy like=l bus1=end.1.0\nedit load.copy pf=.8 daily=s",
        "copy-load-pf-target": "new load.copy phases=1 bus1=end.1.0 kv=.24 pf=.8 like=l daily=s",
        "copy-load-after-shape-edit": "edit loadshape.s mult=[10 20 30] qmult=[2 4 6]\nnew load.copy like=l bus1=end.1.0",
        "copy-shape-fresh": "new loadshape.t like=s\nedit load.l daily=t",
        "copy-shape-qmax-target": "new loadshape.t qmax=4 like=s\nedit load.l daily=t",
        "copy-shape-qmax-source": "edit loadshape.s qmax=4\nnew loadshape.t like=s\nedit load.l daily=t",
        "copy-self": "edit load.l kw=5 like=l\nedit loadshape.s like=s\nedit load.l daily=s",
        "edit-boundaries": "edit load.l pf=.8\nedit load.l daily=s",
        "batch-reference": "batchedit load.l pf=.8 daily=s",
    }.items():
        yield label, SHAPE + LOAD + "daily=s\n" + tail + "\n"
    for label, script in {
        "batch-shape-disable": SHAPE + "batchedit loadshape..* useactual=no\n" + LOAD + "daily=s\n",
        "batch-shape-enable": SHAPE.replace("useactual=yes", "useactual=no") + "batchedit loadshape..* usea=yes\n" + LOAD + "daily=s\n",
        "batch-shape-selection": SHAPE + "new loadshape.t npts=3 mult=[10 20 30] qmult=[2 4 6] useactual=yes\nbatchedit loadshape.^s$ useactual=no\n" + LOAD + "daily=t\n",
        "batch-shape-after-bound": SHAPE + LOAD + "daily=s\nbatchedit loadshape..* mult=[10 20 30] qmult=[2 4 6]\n",
        "batch-shape-rebound": SHAPE + LOAD + "daily=s\nbatchedit loadshape.^s$ npts=3 1 [10 20 30] qmult=[2 4 6] usea=yes\nedit load.l daily=s\n",
        "shape-position-and-abbreviation": "new loadshape.s 3 1 [5 7 3] qmult=[9 2 99] usea=yes\n" + LOAD + "daily=s\n",
    }.items():
        yield label, script
    for kind, fmt in (("dblfile", "d"), ("sngfile", "f")):
        (folder / (kind + "-p.bin")).write_bytes(struct.pack("<3" + fmt, 5, 7.123456789, 3))
        (folder / (kind + "-q.bin")).write_bytes(struct.pack("<3" + fmt, 9, 2.123456789, 99))
        yield kind, (f"new loadshape.s npts=3 mult=[{kind}=../{kind}-p.bin] qmult=[{kind}=../{kind}-q.bin] useactual=yes\n"
                     + LOAD + "daily=s\n")
        yield kind + "-direct", (f"new loadshape.s npts=3 {kind}=../{kind}-p.bin useactual=yes\n"
                                + LOAD + "daily=s\n")
        (folder / (kind + "-time.bin")).write_bytes(struct.pack("<6" + fmt, 0, 5, 1, 7.123456789, 2, 3))
        yield kind + "-variable", (f"new loadshape.s npts=3 interval=0 {kind}=../{kind}-time.bin useactual=yes\n"
                                  + LOAD + "daily=s\n")
    for restore in (False, True, "hour"):
        yield f"single-copy-retained-q-{restore}", (
            "new loadshape.s npts=3 sngfile=../sngfile-p.bin useactual=yes\n"
            "new loadshape.t npts=3 mult=[1 2 3] qmult=[10 20 30] useactual=yes like=s\n"
            + ("edit loadshape.t hour=[0 1 2]\n" if restore == "hour" else "edit loadshape.t mult=[5 7 3]\n" if restore else "")
            + LOAD + "daily=t\n")
    (folder / "bom-p.bin").write_bytes(bytes.fromhex("efbbbf0000001c40") + struct.pack("<d", 3))
    yield "binary-bom-is-data", "new loadshape.s npts=2 mult=[dblfile=../bom-p.bin] qmult=[2 9] useactual=yes\n" + LOAD + "daily=s\n"
    for label, rating in {
        "scalar-secondary": "wdg=1 kva=75 wdg=2 kva=50",
        "scalar-primary-last": "wdg=2 kva=50 wdg=1 kva=75",
        "rating-array": "kvas=[75 50]",
        "array-then-scalar": "kvas=[75 50] wdg=2 kva=60",
    }.items():
        yield "transformer-" + label, (
            "new transformer.t phases=1 buses=[end.1 loadbus.1] kvs=[.24 .12] "
            + rating + " xhl=1.44 %rs=[.59 .59] ppm_antifloat=0\n"
            + SHAPE + LOAD.replace("bus1=end.1.0", "bus1=loadbus.1.0").replace("kv=.24", "kv=.12") + "daily=s\n")
    (folder / "pq.csv").write_text("5,9\n7.123456789,2.123456789\n3,99\n")
    (folder / "header.csv").write_text("P,Q\n5,9\n7.123456789,2.123456789\n3,99\n")
    yield "csv", "new loadshape.s npts=3 mult=[file=../header.csv column=1 header=yes] qmult=[file=../header.csv column=2 header=yes] useactual=yes\n" + LOAD + "daily=s\n"
    yield "pqcsv", "new loadshape.s npts=3 pqcsvfile=../pq.csv useactual=yes\n" + LOAD + "daily=s\n"

    (folder / "time.csv").write_text("0,5,9\n1,7.123456789,2.123456789\n2,3,99\n")
    for kind in ("csvfile", "pqcsvfile"):
        yield kind + "-variable", f"new loadshape.s npts=3 interval=0 {kind}=../time.csv useactual=yes\n" + LOAD + "daily=s\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    rows = {}
    prefix = ("clear\nnew circuit.c phases=3 bus1=sourcebus basekv=.41569219381653055 r1=.001 x1=.001 r0=.001 x0=.001\n"
              "new line.line1 phases=1 bus1=sourcebus.1 bus2=end.1 r1=.02 x1=.01 r0=.02 x0=.01 c1=0 c0=0 length=1 units=m\n")
    for name, body in cases(root):
        folder = root / name
        folder.mkdir()
        source = folder / "input.dss"
        source.write_text(prefix + body + "set controlmode=off tolerance=1e-11\nsolve\n")
        row = evaluate(source, args.bridge.resolve(), folder, 1e-11, root)
        if row.get("comparison"):
            engine = run(source, 1e-11)
            network = json.loads((folder / "imported/bridge.json").read_text())["network"]
            errors = {}
            for load in network["loads"]:
                engine.Loads.Name(load["name"])
                errors[load["name"]] = max(abs(engine.Loads.kW() - sum(load["p_nom"]) / 1000),
                                          abs(engine.Loads.kvar() - sum(load["q_nom"]) / 1000))
            row["nominal_power_error_kW_kvar"] = errors
            row["passed"] &= max(errors.values()) < 1e-10
        rows[name] = row
        print(name, "PASS" if row["passed"] else "FAIL", row.get("error", ""), flush=True)
    report = dict(cases=rows, passed=all(row["passed"] for row in rows.values()),
                  bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest())
    (root / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
