"""Pinned IEEE4 cases through import, IR and repeated canonical DSS.

Reports every failure. Definition-only, retained-source or nonconverged results
never count as electrical qualification. Public dataset bytes remain external.
"""

import argparse
import hashlib
import json
import os
import re
from pathlib import Path

import opendssdirect as dss
from european_lv_import_oracle import snapshot
from oracle import HERE, LOCK, command, compare
from physical_ports import interfaces


def run(path, tolerance):
    path = path.resolve()
    engine = dss.NewContext()
    for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
        getattr(engine.Basic, flag)(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("unpinned engine")
    engine.Basic.DataPath(str(path.parent.resolve()))
    # UUIDs uses the process directory even when native AllowChangeDir is off.
    previous = Path.cwd()
    try:
        os.chdir(path.parent)
        engine.Text.Command(f'redirect "{path.resolve()}"')
    finally:
        os.chdir(previous)
    engine.Text.Command(
        f"set mode=snapshot controlmode=off tolerance={tolerance} maxiterations=1000"
    )
    engine.Text.Command("solve")
    if not engine.Solution.Converged():
        raise ValueError("OpenDSS did not converge")
    return engine


def imported(bridge, source, folder, acquisition_root=None):
    args = [str(bridge), str(source), str(folder)]
    if acquisition_root is not None:
        args.append(str(acquisition_root))
    result = json.loads(command(args, folder.parent))
    (folder / "bridge.json").write_text(json.dumps(result, indent=2) + "\n")
    if not result["ir_network_equal"] or not result["targets"]["dss"]["ok"]:
        raise ValueError("IR equality or canonical DSS emission failed")
    if any(f["severity"] == "Blocker" for f in result["readiness"]):
        raise ValueError("import retains electrical readiness blockers")
    return folder / "dss/case.dss"


def evaluate(source, bridge, folder, tolerance, acquisition_root=None):
    row = {"passed": False, "solver_tolerance": tolerance}
    try:
        row["stage"] = "import"
        canonical_path = imported(bridge, source, folder / "imported", acquisition_root)
        network = json.loads((folder / "imported/bridge.json").read_text())["network"]
        aliases = {
            line["extras"]["dss_original_element"].lower(): "line."
            + line["name"].lower()
            for line in network["lines"]
            if "dss_original_element" in line.get("extras", {})
        }
        aliases.update({
            load["extras"]["dss_original_element"].lower(): "storage." + load["name"].lower()
            for load in network["loads"]
            if load["voltage_model"]["model"] == "bounded_constant_power"
            and "dss_original_element" in load.get("extras", {})
        })
        row["physical_port_aliases"] = aliases
        row["stage"] = "native solve"
        native = run(source, tolerance)
        row["engine"] = native.Basic.Version()
        port_maps = interfaces(native, network, aliases)
        row["physical_port_maps"] = port_maps
        buses = [b.lower() for b in native.Circuit.AllBusNames()]
        elements = [
            n.lower()
            for n in native.Circuit.AllElementNames()
            if n.lower().startswith(
                (
                    "line.",
                    "transformer.",
                    "load.",
                    "autotrans.",
                    "capacitor.",
                    "reactor.",
                    "generator.",
                    "pvsystem.",
                    "storage.",
                )
            )
        ]
        expected = snapshot(native, buses, elements, canonical=False)
        (folder / "native.json").write_text(json.dumps(expected, indent=2) + "\n")
        row["stage"] = "canonical solve"
        canonical = run(canonical_path, tolerance)
        actual = snapshot(canonical, buses, elements, canonical=True, aliases=aliases, port_maps=port_maps)
        row["comparison"] = compare(expected, actual)
        (folder / "canonical.json").write_text(json.dumps(actual, indent=2) + "\n")
        row["stage"] = "reimport"
        reimported_path = imported(bridge, canonical_path, folder / "reimported")
        repeated = snapshot(
            run(reimported_path, tolerance),
            buses,
            elements,
            canonical=True,
            aliases=aliases,
            port_maps=port_maps,
        )
        row["reimport_comparison"] = compare(expected, repeated)
        (folder / "reimported.json").write_text(json.dumps(repeated, indent=2) + "\n")
        row["stage"] = "wrong line negative control"
        canonical.Lines.Name(
            "line1"
            if "line.line1" in elements
            else next(
                n.removeprefix("line.") for n in elements if n.startswith("line.")
            )
        )
        line = canonical.Lines.Name()
        length = canonical.Lines.Length()
        # Initialize the changed model independently, avoiding the previous
        # solution's floating-node state as a hidden input to the control.
        text = canonical_path.read_text()
        pattern = (
            r"(?im)^(\s*new\s+line\." + re.escape(line) + r"\s+[^\n]*?\blength=)(\S+)"
        )
        text, count = re.subn(pattern, lambda m: m[1] + f"{length * 1.25:.17g}", text)
        if count != 1:
            raise ValueError(
                "negative control could not identify exactly one Line declaration"
            )
        wrong_path = canonical_path.with_name("wrong-line.dss")
        wrong_path.write_text(text)
        wrong = snapshot(
            run(wrong_path, tolerance), buses, elements, canonical=True, aliases=aliases, port_maps=port_maps
        )
        negative = compare(expected, wrong)
        row["wrong_line_negative"] = {
            "passed": not negative["passed"],
            "comparison": negative,
        }
        (folder / "wrong-line.json").write_text(json.dumps(wrong, indent=2) + "\n")
        row["passed"] = (
            row["comparison"]["passed"]
            and row["reimport_comparison"]["passed"]
            and not negative["passed"]
        )
        row["stage"] = "compared"
    except Exception as error:
        row["error"] = str(error)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ieee4-base", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    pin = next(
        s
        for f in json.loads((HERE / "ieee-corpus.json").read_text())["families"]
        if f["id"] == "ieee4"
        for s in f["sources"]
        if s["repository"].endswith("/CIMHub")
    )
    report = {
        "scope": __doc__,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    verified = {}
    for record in pin["files"]:
        name = Path(record["path"]).name
        data = (args.ieee4_base / name).read_bytes()
        if (
            len(data) != record["bytes"]
            or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
            != record["git_blob_sha1"]
        ):
            raise ValueError(f"source pin mismatch: {name}")
        verified[name] = (record, data)
    report["companions"] = {
        name: dict(record, sha256=hashlib.sha256(data).hexdigest())
        for name, (record, data) in verified.items()
        if not name.lower().endswith(".dss")
    }
    for name, (record, data) in verified.items():
        if not name.lower().endswith(".dss"):
            continue
        folder = args.output / Path(name).stem
        folder.mkdir()
        source = folder / "native.dss"
        source.write_bytes(data)
        for companion in report["companions"]:
            (folder / companion).write_bytes(verified[companion][1])
        # Floating delta cases and edited controls can stall below 1e-11.
        # Use one pinned solve criterion, stricter than the files' 1e-8;
        # observable acceptance in oracle.compare remains unchanged.
        tolerance = 1e-11
        row = evaluate(
            source.resolve(), args.bridge.resolve(), folder.resolve(), tolerance
        )
        row["source"] = dict(
            repository=pin["repository"], commit=pin["commit"], **record
        )
        row["source_sha256"] = hashlib.sha256(data).hexdigest()
        report["cases"][name] = row
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["qualified_cases"] = sum(c["passed"] for c in report["cases"].values())
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Qualified {report['qualified_cases']}/{len(report['cases'])} cases.")


if __name__ == "__main__":
    main()
