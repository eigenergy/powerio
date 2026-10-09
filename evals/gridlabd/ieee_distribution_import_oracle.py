"""Public IEEE and low-voltage DSS feeders at their written, fixed device states.

Every solve and voltage-base calculation in the selected entry files is
preceded by ControlMode=Off. Original bytes are pinned and retained separately;
this is an explicit static profile, not a claim about active regulator results.
"""

import argparse
import hashlib
import json
import os
import re
from pathlib import Path

from european_lv_import_oracle import snapshot
from ieee4_import_oracle import evaluate, run
from oracle import HERE, LOCK

PREFIX = "Version8/Distrib/IEEETestCases/"
CASES = {
    "ieee13": "13Bus/IEEE13Nodeckt.dss",
    "ieee34_mod1": "34Bus/ieee34Mod1.dss",
    "ieee34_mod2": "34Bus/ieee34Mod2.dss",
    "ieee37": "37Bus/ieee37.dss",
    "ieee123": "123Bus/IEEE123Master.dss",
    "ieee123_switches": "123Bus/IEEE123Switches.dss",
    "ieee8500_balanced": "8500-Node/Master.dss",
    "ieee8500_unbalanced": "8500-Node/Master-unbal.dss",
    "ieee9500": "ieee9500dss/ieee9500_base.dss",
    "north_american": "LVTestCaseNorthAmerican/Master.dss",
    "north_american_secpar": "LVTestCaseNorthAmerican/SecPar.dss",
    "neutral_earth_voltage": "NEVTestCase/NEVMASTER.DSS",
    "four_wire_indmotor": "4wire-Delta/Kersting4wireIndMotor.dss",
    "four_wire_lagging": "4wire-Delta/Kersting4wire_Lagging.dss",
    "four_wire_leading": "4wire-Delta/Kersting4wire_Leading.dss",
    "secondary_circuit": "SecondaryTestCircuit_modified/Master.DSS",
}


def motor_reference(source, folder):
    """Record the original native baseline even when typed import is blocked."""
    try:
        engine = run(source, 1e-11)
        elements = [name.lower() for name in engine.Circuit.AllElementNames()
                    if name.lower().startswith(("line.", "transformer.", "load.", "generator."))]
        values = snapshot(engine, engine.Circuit.AllBusNames(), elements, canonical=False)
        (folder / "native-reference.json").write_text(json.dumps(values, indent=2) + "\n")
        motors = {}
        for element in elements:
            if element.startswith("generator."):
                engine.Circuit.SetActiveElement(element)
                motors[element] = dict(zip(engine.CktElement.AllVariableNames(), engine.CktElement.AllVariableValues(), strict=True))
        return dict(converged=True, engine=engine.Basic.Version(), solver_tolerance=1e-11,
                    observables=len(values), variables=motors,
                    qualification="Native baseline only; typed conversion must independently pass.")
    except Exception as error:
        return dict(converged=False, error=str(error))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--source-root",
        type=Path,
        required=True,
        help="pinned electricdss-tst checkout",
    )
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=CASES, help="run only selected entries; repeat to select several")
    parser.add_argument("--indmach-manifest", type=Path, help="manifest from build_indmach_oracle.py; start Python with its build directory in DYLD_LIBRARY_PATH")
    args = parser.parse_args()
    motor = None
    if args.indmach_manifest is not None:
        motor = json.loads(args.indmach_manifest.read_text())
        library = Path(motor["library"])
        if (motor["commit"] != LOCK["indmach012a"]["commit"]
                or motor["engine_version"] != LOCK["dss"]["version"]
                or hashlib.sha256(library.read_bytes()).hexdigest() != motor["library_sha256"]
                or str(library.parent) != os.environ.get("DYLD_LIBRARY_PATH", "").split(os.pathsep)[0]
                or (library.parent / "IndMach012a").resolve() != library.resolve()):
            raise ValueError("motor oracle pin or loader-directory mismatch")
    selected = set(args.case or CASES)
    args.output.mkdir(parents=True, exist_ok=False)
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    records = {}
    commits = set()
    for family in catalog["families"]:
        if family["id"] not in (
            "ieee13",
            "ieee34",
            "ieee37",
            "ieee123",
            "north_american_lv_network",
            "neutral_earth_voltage",
            "four_wire_delta",
            "secondary_circuit",
            "ieee8500",
            "ieee9500",
        ):
            continue
        for pin in family["sources"]:
            if not pin["repository"].endswith("/electricdss-tst"):
                continue
            commits.add(pin["commit"])
            for record in pin["files"]:
                if record["path"] in records and records[record["path"]] != record:
                    raise ValueError("inconsistent source pins")
                records[record["path"]] = record
    if len(commits) != 1:
        raise ValueError("expected one pinned source revision")
    original = args.output / "original"
    fixed = args.output / "fixed"
    patches = {}
    hashes = {}
    for relative, record in records.items():
        data = (args.source_root / relative).read_bytes()
        if (
            len(data) != record["bytes"]
            or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
            != record["git_blob_sha1"]
        ):
            raise ValueError(f"source pin mismatch: {relative}")
        a, b = original / relative, fixed / relative
        a.parent.mkdir(parents=True, exist_ok=True)
        b.parent.mkdir(parents=True, exist_ok=True)
        a.write_bytes(data)
        adjusted = data
        if relative in {PREFIX + value for value in CASES.values()}:
            lines = data.decode("utf-8-sig").splitlines(keepends=True)
            result = []
            edits = []
            for number, line in enumerate(lines, 1):
                code = re.split(r"!|//", line)[0].strip()
                if re.match(r"(?i)^(solve|calcv|calcvoltagebases)(?:\s|$)", code):
                    result.append("Set ControlMode=Off\n")
                    edits.append(
                        dict(
                            before_line=number,
                            original_command=code,
                            insertion="Set ControlMode=Off",
                        )
                    )
                result.append(line)
            if not edits:
                raise ValueError(
                    f"selected feeder has no recognized calculation: {relative}"
                )
            adjusted = "".join(result).encode()
            patches[relative] = edits
        # This public Windows dataset names binary profiles with backslashes.
        # Normalize only its explicit profile-file references for the native
        # POSIX engine; pinned binary bytes and all electrical settings remain.
        if relative.startswith(PREFIX + "SecondaryTestCircuit_modified/") and relative.lower().endswith(".dss"):
            lines = adjusted.decode("utf-8-sig").splitlines(keepends=True)
            result = []
            for number, line in enumerate(lines, 1):
                normalized = re.sub(r"(?i)(\b(?:sngfile|dblfile)\s*=\s*Profiles)\\", r"\1/", line)
                if normalized != line:
                    patches.setdefault(relative, []).append(dict(
                        line=number, original_command=line.strip(), replacement=normalized.strip(),
                        reason="portable binary-profile path separator",
                    ))
                result.append(normalized)
            adjusted = "".join(result).encode()
        b.write_bytes(adjusted)
        hashes[relative] = dict(
            record,
            original_sha256=hashlib.sha256(data).hexdigest(),
            profile_sha256=hashlib.sha256(adjusted).hexdigest(),
        )
    report = dict(
        scope=__doc__,
        source_commit=commits.pop(),
        files=hashes,
        profile_patches=patches,
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
        indmach_oracle=motor,
    )
    for name, relative in CASES.items():
        if name not in selected:
            continue
        folder = args.output / name
        folder.mkdir()
        baseline = motor_reference((fixed / PREFIX / relative).resolve(), folder) if motor is not None and name.startswith("four_wire_") else None
        row = evaluate(
            (fixed / PREFIX / relative).resolve(),
            args.bridge.resolve(),
            folder.resolve(),
            1e-11,
            fixed.resolve(),
        )
        if baseline is not None:
            row["native_baseline"] = baseline
        row["entry"] = PREFIX + relative
        report["cases"][name] = row
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(name, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
    report["qualified_cases"] = sum(v["passed"] for v in report["cases"].values())
    report["passed"] = all(v["passed"] for v in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Qualified {report['qualified_cases']}/{len(report['cases'])} fixed-state cases.")


if __name__ == "__main__":
    main()
