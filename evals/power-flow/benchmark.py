"""Convert each reference case and compare solved AC voltages with the original."""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys

from solvers import HERE, ROOT, solve_many

# A file extension of None indicates a multi-file format. A target's
# availability here describes PowerIO's writer, independent of solver coverage.
TARGETS = {
    "transmission": {
        "matpower": ".m", "powermodels-json": ".json", "psse": ".raw",
        "psse34": ".raw", "psse35": ".raw", "psse-rawx": ".rawx",
        "powerworld": ".aux", "egret-json": ".json", "pandapower-json": ".json",
        "surge-json": ".surge.json", "powersystems-json": ".json", "sienna-json": ".json",
        "sienna-0-1-json": ".json", "sienna-0-2-json": ".json", "sienna-0-3-json": ".json",
        "sienna-0-4-json": ".json", "ravens-json": ".json", "pslf": ".epc",
        "xiidm": ".xiidm", "jiidm": ".jiidm", "cgmes": None, "ucte": ".uct",
        "pypsa-csv": None, "gridfm": None,
    },
    "distribution": {
        "dss": ".dss", "pmd-json": ".json", "pmd-math-json": ".json", "bmopf-json": ".json",
        "ravens-json": ".json", "cim-dist": ".xml", "gridlabd": ".glm", "gridlabd-arras": ".glm",
    },
}
TOLERANCE = {"voltage_pu": 1e-9, "voltage_V": 1e-3, "angle_deg": math.degrees(1e-9),
             "branch_active_MW": 1e-7, "branch_reactive_Mvar": 1e-7,
             "admittance_relative_2_norm": 1e-9}
DISPLAY = {"agree": "🟢", "different": "🔴", "unavailable": "—", "solve_failed": "🔴",
           "conversion_failed": "🔴", "not_comparable": "—", "not_applicable": "—",
           "partial": "◐"}


def record(status, detail=None, **fields):
    if detail:
        fields["detail"] = detail
    return {"status": status, **fields}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def inventory(path):
    if path.is_dir():
        return {p.relative_to(path).as_posix(): sha(p) for p in sorted(path.rglob("*")) if p.is_file()}
    return {path.name: sha(path)}


def input_files(case, path):
    if "files" in case:
        directory = path if path.is_dir() else path.parent
        return {name: sha(directory / name) for name in case["files"]}
    return inventory(path)


def cli(binary, *args):
    try:
        run = subprocess.run([binary, *args, "--diagnostics-format", "json"],
                             text=True, capture_output=True, timeout=60)
    except subprocess.TimeoutExpired:
        return record("conversion_failed", "PowerIO did not finish within 60 seconds", diagnostics=[])
    try:
        diagnostics = json.loads(run.stderr)
    except json.JSONDecodeError:
        diagnostics = [{"message": run.stderr.strip()}] if run.stderr.strip() else []
    return record("conversion_failed" if run.returncode else "ok", diagnostics=diagnostics)


def to_ir(binary, path, format, output):
    return cli(binary, "serialize", str(path), "--from", format, "-o", str(output))


def convert(binary, path, src, target, directory, extension):
    directory.mkdir(parents=True, exist_ok=True)
    # Egret takes the case name from the input filename. Use a stem that is
    # also a valid MATPOWER function name when the case is converted back.
    output = directory / ("network" + (extension or ""))
    id_map = directory / "element-map.json"
    if target == "gridfm":
        result = cli(binary, "gridfm", str(path), "--from", src, "-o", str(output))
        folders = list(output.glob("*")) if output.exists() else []
        if len(folders) == 1:
            output = folders[0]
    else:
        args = ["convert", str(path), "--to", target, "-o", str(output)]
        if src:
            args += ["--from", src]
        if target in TARGETS["transmission"]:
            args += ["--id-map", str(id_map)]
        result = cli(binary, *args)
    if result["status"] == "ok" and not output.exists():
        result.update(status="conversion_failed", detail="PowerIO returned no output file or directory")
    return output, result, id_map if id_map.exists() else None


def recover_original(binary, case, source, files, out):
    format = case["format"]
    if format == "gridfm":
        return record("not_applicable", "The GridFM writer generates scenario tables")
    extension = TARGETS[case["family"]].get(format, None if source.is_dir() else source.suffix)
    output, result, _ = convert(binary, source, format, format, out / "original-files", extension)
    if result["status"] != "ok":
        status = "unavailable" if not case.get("writes", True) else "conversion_failed"
        return record(status, "PowerIO could not return this source in its original format", **{"diagnostics": result["diagnostics"]})
    if output.is_file() and len(files) == 1:
        matches = sha(output) == next(iter(files.values()))
    elif output.is_dir():
        written = inventory(output)
        if "case.dss" in written and any(name.startswith("source/") for name in written):
            written = {n.removeprefix("source/"): d for n, d in written.items() if n.startswith("source/")}
        matches = files == written
    else:
        matches = False
    return record("agree" if matches else "different", file_count=len(files))


def json_differences(old, new, pointer=""):
    if isinstance(old, dict) and isinstance(new, dict):
        for key in sorted(old.keys() | new.keys()):
            token = key.replace("~", "~0").replace("/", "~1")
            location = f"{pointer}/{token}"
            if key not in old or key not in new:
                yield location
            else:
                yield from json_differences(old[key], new[key], location)
    elif old != new:
        yield pointer or "/"


def check_written_data(binary, original, output, format, path):
    parsed = to_ir(binary, output, format, path)
    if parsed["status"] != "ok":
        return parsed
    before = json.loads(original.read_text())["value"]
    after = json.loads(path.read_text())["value"]
    differences = list(json_differences(before, after))
    return record("different" if differences else "agree", changed_fields=differences)


def unique_labels(points, aliases, candidates):
    return {name for name in points if {name, aliases.get(name)} & candidates}


def match_labels(source, target, original_ir, element_map):
    # Accept only an exact one-to-one correspondence through stable labels,
    # the writer's element map, or explicitly named buses in both files.
    expected, actual = source["voltage"], target["voltage"]
    if expected.keys() == actual.keys():
        return {name: name for name in expected}
    if not original_ir or (not element_map and not target.get("aliases")):
        return None
    original = json.loads(original_ir.read_text())["value"]["data"]
    if not isinstance(original, dict) or "buses" not in original:
        return None
    by_uid = json.loads(element_map.read_text()).get("bus", {}) if element_map else {}
    result = {}
    for bus in original["buses"]:
        labels = {str(bus[key]) for key in ("id", "uid", "name") if bus.get(key)}
        identity = unique_labels(expected, source.get("aliases", {}), labels)
        output = by_uid.get(bus["uid"])
        if output:
            other = unique_labels(actual, target.get("aliases", {}), {str(output.get("key"))})
        else:
            other = set()
        if not other and bus.get("name"):
            other = unique_labels(actual, target.get("aliases", {}), {bus["name"].strip()})
        if len(identity) == len(other) == 1:
            result[identity.pop()] = other.pop()
    return result if result.keys() == expected.keys() and set(result.values()) == actual.keys() else None


def branch_powers(rows, labels):
    values = {}
    duplicated = set()
    for a, b, pf, qf, pt, qt in rows:
        if a not in labels or b not in labels:
            continue
        a, b = labels[a], labels[b]
        key = (a, b) if a <= b else (b, a)
        if key in values:
            duplicated.add(key)
        values[key] = (pf, qf, pt, qt) if a <= b else (pt, qt, pf, qf)
    return {key: value for key, value in values.items() if key not in duplicated}


def single_island(solution):
    if "one_island" in solution:
        return solution["one_island"]
    neighbors = {label: set() for label in solution["voltage"]}
    for a, b, *_ in solution.get("branch_power", []):
        if a in neighbors and b in neighbors:
            neighbors[a].add(b)
            neighbors[b].add(a)
    seen, todo = set(), [next(iter(neighbors))]
    while todo:
        node = todo.pop()
        if node not in seen:
            seen.add(node)
            todo.extend(neighbors[node] - seen)
    return len(seen) == len(neighbors)


def compare_branch_powers(source, target, pairing):
    expected = branch_powers(source.get("branch_power", []), {name: name for name in pairing})
    actual = branch_powers(target.get("branch_power", []), {b: a for a, b in pairing.items()})
    common = expected.keys() & actual.keys()
    fields = {}
    for column, key in enumerate(("branch_active_MW", "branch_reactive_Mvar")):
        if not common:
            fields[key] = record("unavailable", "No matching branch power measurements")
            continue
        difference, branch = max(
            (max(abs(expected[b][k] - actual[b][k]) for k in (column, column + 2)), b) for b in common
        )
        wanted = max(len(source["branch_power"]), len(target["branch_power"]))
        status = "different" if difference > TOLERANCE[key] else "agree" if len(common) == wanted else "partial"
        fields[key] = record(status, maximum_difference=difference, tolerance=TOLERANCE[key],
                             branches_compared=len(common), branches_reported=wanted, worst_branch=branch)
    return fields


def compare_admittance(source, target, pairing):
    import numpy as np
    from scipy.sparse import coo_matrix
    from scipy.sparse.linalg import ArpackNoConvergence, svds

    a, b = source.get("admittance"), target.get("admittance")
    if not a or not b:
        return record("unavailable", "One of the tools did not report an admittance matrix")
    if a["basis"] != b["basis"]:
        return record("not_comparable", "The admittance matrices use different definitions or bases")
    scale = 1.0
    if a["basis"] == "network-pu":
        base_a, base_b = a.get("base_mva"), b.get("base_mva")
        if not (base_a and base_b and base_a > 0 and base_b > 0):
            return record("not_comparable", "Missing system base MVA for an admittance matrix")
        scale = base_b / base_a
    labels = a["labels"]
    if (len(labels) != len(set(labels)) or any(label not in pairing for label in labels)
            or len(b["labels"]) != len(labels) or {pairing[label] for label in labels} != set(b["labels"])):
        return record("not_comparable", "The rows and columns of the admittance matrices could not be matched")

    def matrix(item, ordered_labels):
        rows = {name: i for i, name in enumerate(ordered_labels)}
        order = [rows[name] for name in item["labels"]]
        coordinates = ([], [])
        values = []
        for i, j, re, im in item["entries"]:
            coordinates[0].append(order[int(i)])
            coordinates[1].append(order[int(j)])
            values.append(complex(re, im))
        return coo_matrix((values, coordinates), shape=(len(order), len(order))).tocsr()

    def norm(value):
        if not value.nnz or not np.any(value.data):
            return 0.0
        if value.shape[0] <= 256:
            return float(np.linalg.norm(value.toarray(), ord=2))
        return float(svds(value, k=1, return_singular_vectors=False, rng=0)[0])

    expected = matrix(a, labels)
    actual = matrix(b, [pairing[label] for label in labels]) * scale
    if not np.isfinite(expected.data).all() or not np.isfinite(actual.data).all():
        return record("different", "Nonfinite admittance matrix entry")
    try:
        denominator = norm(expected)
        if denominator == 0:
            return record("not_comparable", "Relative error is undefined for a zero reference matrix")
        difference = norm(actual - expected) / denominator
    except ArpackNoConvergence:
        return record("unavailable", "The spectral norm calculation did not converge")
    tolerance = TOLERANCE["admittance_relative_2_norm"]
    return record("agree" if difference <= tolerance else "different",
                  relative_error=difference, tolerance=tolerance, order=len(labels), basis=a["basis"],
                  output_scale=scale)


def compare(source, target, original_ir=None, element_map=None, pairing=None):
    if target["status"] != "solved":
        return target
    if source["status"] != "solved":
        return record("unavailable", "The reference file did not have an external solution", output_solver=target["solver"])
    expected, actual = source["voltage"], target["voltage"]
    pairing = pairing or match_labels(source, target, original_ir, element_map)
    solvers = {"original_solver": source["solver"], "output_solver": target["solver"]}
    if source["units"] != target["units"]:
        return record("not_comparable", "Voltage units differ and no common voltage base is established", **solvers)
    if not pairing:
        return record("not_comparable", "Bus or conductor labels could not be matched unambiguously",
                      original_nodes=len(expected), output_nodes=len(actual), **solvers)
    # Voltage angle has one arbitrary offset for each connected component.
    # Until adapters establish matching components, compare angles only where
    # both identify one component. OpenDSS is one source-side reference here.
    reference = min(pairing)
    offset = actual[pairing[reference]][1] - expected[reference][1]
    errors = {name: (
        abs(expected[name][0] - actual[other][0]),
        abs(math.remainder(actual[other][1] - expected[name][1] - offset, 360)),
    ) for name, other in pairing.items()}
    metrics = {}
    names = ("voltage_" + source["units"], "angle_deg")
    for column, key in enumerate(names):
        if column == 1 and not (single_island(source) and single_island(target)):
            metrics[key] = record("unavailable", "A common angle reference is not established for all islands")
            continue
        worst = max(errors, key=lambda name: (errors[name][column], name))
        difference = errors[worst][column]
        metrics[key] = record("agree" if difference <= TOLERANCE[key] else "different",
                              maximum_difference=difference, tolerance=TOLERANCE[key], worst_node=worst,
                              original_voltage=expected[worst], output_voltage=actual[pairing[worst]])
    metrics.update(compare_branch_powers(source, target, pairing))
    power_flow = {m["status"] for m in metrics.values()}
    metrics["admittance_relative_2_norm"] = compare_admittance(source, target, pairing)
    statuses = {m["status"] for m in metrics.values()}
    status = "different" if "different" in statuses else "agree" if statuses == {"agree"} else "partial"
    power_flow_status = "different" if "different" in power_flow else "agree" if power_flow == {"agree"} else "partial"
    return record(status, power_flow_status=power_flow_status, nodes_compared=len(pairing),
                  metrics=metrics, matched_labels=pairing, **solvers)


def prepare_case(binary, case, corpus, work, selected_targets):
    formats = TARGETS[case["family"]]
    root = work / case["id"]
    root.mkdir()
    pairs = {}
    requests = {}
    ready = {}
    for target in formats:
        if not selected_targets or target in selected_targets:
            pairs[target] = {direction: record("unavailable")
                             for direction in ("direct", "round_trip", "target_vs_return")}
    source = ROOT / case["fixture"] if case.get("fixture") else corpus / case.get("external", "unavailable")
    result = {"source": case, "conversions": pairs}
    if not source.exists():
        result["input"] = record("unavailable", case.get("why_missing", f"Missing reference case {source.name}"))
        for pair in pairs.values():
            for comparison in pair.values():
                comparison["detail"] = result["input"]["detail"]
        return result, requests, ready
    files = input_files(case, source)
    if case.get("sha256") and next(iter(files.values())) != case["sha256"]:
        raise ValueError(f"Digest changed for {case['id']}")
    for file in case.get("downloads", []):
        if files.get(file["name"]) != file["sha256"]:
            raise ValueError(f"Digest changed for {case['id']}/{file['name']}")
    result["input"] = record("ok", sha256=files)
    format = case["format"]
    solver_format = case.get("solver_format", format)
    original = root / "source.pio.json"
    parsed = to_ir(binary, source, format, original)
    result["read_original"] = parsed
    result["recover_original_files"] = recover_original(binary, case, source, files, root)
    requests[f"{case['id']}/source"] = (solver_format, source)
    if parsed["status"] != "ok":
        for pair in pairs.values():
            pair["direct"] = record("conversion_failed", "PowerIO could not read the source")
        return result, requests, ready

    for target, pair in pairs.items():
        cell = root / target
        input_path, input_format = (source, format) if target == "gridfm" else (original, None)
        forward, pair["write_target"], element_map = convert(
            binary, input_path, input_format, target, cell / "target", formats[target]
        )
        if pair["write_target"]["status"] != "ok":
            pair["direct"] = record("conversion_failed", "PowerIO could not write the target file")
            continue
        key = f"{case['id']}/{target}"
        requests[key + "/direct"] = (target, forward)
        ready[key + "/direct"] = (target, "direct", original, element_map)
        target_ir = cell / "target.pio.json"
        pair["check_target_data"] = check_written_data(binary, original, forward, target, target_ir)
        if target == format:
            pair["round_trip"] = record("not_applicable", "The direct test already wrote the original format")
            pair["target_vs_return"] = pair["round_trip"]
            continue
        if not case.get("writes", True):
            pair["round_trip"] = record("not_applicable", f"PowerIO cannot generate a new {format} case")
            pair["target_vs_return"] = pair["round_trip"]
            continue
        if pair["check_target_data"]["status"] == "conversion_failed":
            pair["round_trip"] = record("conversion_failed", "PowerIO could not read the target file for the return conversion")
            pair["target_vs_return"] = pair["round_trip"]
            continue
        input_path, input_format = (forward, target) if format == "gridfm" else (target_ir, None)
        back, pair["write_source_format"], _ = convert(
            binary, input_path, input_format, format, cell / "return", formats[format]
        )
        if pair["write_source_format"]["status"] != "ok":
            pair["round_trip"] = record("conversion_failed", "PowerIO could not write the source format again")
            pair["target_vs_return"] = pair["round_trip"]
            continue
        requests[key + "/round_trip"] = (solver_format, back)
        ready[key + "/round_trip"] = (target, "round_trip", original, None)
        pair["check_returned_data"] = check_written_data(binary, original, back, format, cell / "return.pio.json")
    return result, requests, ready


def markdown(report):
    lines = ["# AC power flow after PowerIO conversion", "",
             "The three marks in each cell compare original with converted, original with converted-back,",
             "and converted with converted-back, in that order.",
             "The reference is the result of solving the original file. See the JSON for individual failures,",
             "numerical differences, and reasons a comparison could not run.", ""]
    lines += ["| Input | Original files recovered | Grid data in new files written in the original format |",
              "| --- | --- | --- |"]
    for name, case in report["cases"].items():
        recovery = case.get("recover_original_files", case["input"])
        own_format = case["conversions"].get(case["source"]["format"], {})
        grid = own_format.get("check_target_data", own_format.get("write_target", {"status": "unavailable"}))
        lines.append(f"| {name} | {recovery['status']} | {grid['status']} |")
    lines.append("")
    for family, targets in TARGETS.items():
        rows = [case for case in report["cases"].values() if case["source"]["family"] == family]
        if not rows:
            continue
        columns = [target for target in targets if any(target in case["conversions"] for case in rows)]
        lines += [f"## {family.capitalize()}", "", "| Input | " + " | ".join(columns) + " |",
                  "| --- | " + " | ".join(["---"] * len(columns)) + " |"]
        for case in rows:
            values = []
            for target in columns:
                cell = case["conversions"][target]
                values.append(" / ".join(DISPLAY[cell[direction]["status"]]
                                         for direction in ("direct", "round_trip", "target_vs_return")))
            lines.append(f"| {case['source']['id']} | " + " | ".join(values) + " |")
        lines += ["", "🟢 All comparisons passed. ◐ Only some quantities could be compared.",
                  "🔴 A measured difference or a failed conversion or solve. — See the JSON for the reason.", ""]
    return "\n".join(lines)


def compare_target_and_return(pair, converted, returned):
    a = pair["direct"].get("matched_labels", {})
    b = pair["round_trip"].get("matched_labels", {})
    common = {label: b[original] for original, label in a.items() if original in b}
    pairing = common if common.keys() == converted.get("voltage", {}).keys() and set(common.values()) == returned.get("voltage", {}).keys() else None
    return compare(converted, returned, pairing=pairing)


def report_changes(previous, current):
    def measurements(report):
        checks = {}
        for case, spec in report["cases"].items():
            for target, cell in spec["conversions"].items():
                for direction in ("direct", "round_trip", "target_vs_return"):
                    result = cell[direction]
                    checks[(case, target, direction, "overall")] = result["status"]
                    for metric, reading in result.get("metrics", {}).items():
                        checks[(case, target, direction, metric)] = (
                            reading["status"], reading.get("maximum_difference", reading.get("relative_error")),
                            reading.get("branches_compared", reading.get("order", result["nodes_compared"])),
                        )
        return checks

    a, b = measurements(previous), measurements(current)
    measured = {"agree", "different", "partial"}
    return {
        "reason_to_review": {
            "tolerances_changed": previous["tolerances"] != current["tolerances"],
            "inputs_changed": {
                name: [previous["cases"].get(name, {}).get("input"), current["cases"].get(name, {}).get("input")]
                for name in sorted(previous["cases"].keys() | current["cases"].keys())
                if previous["cases"].get(name, {}).get("input") != current["cases"].get(name, {}).get("input")
            },
            "solvers_changed": previous["solvers"] != current["solvers"],
        },
        "changes": [
            {"input": case, "output_format": target, "comparison": direction, "metric": metric,
             "before": a.get(key), "after": b.get(key),
             "coverage_reduced": (
                 isinstance(a.get(key), tuple) and (
                     not isinstance(b.get(key), tuple)
                     or (a[key][0] in measured and b[key][0] not in measured)
                     or (a[key][2] is not None and b[key][2] is not None and b[key][2] < a[key][2])
                 )
                 or isinstance(a.get(key), str) and a[key] in measured and b.get(key) not in measured
             )}
            for key in sorted(a.keys() | b.keys()) if a.get(key) != b.get(key)
            for case, target, direction, metric in [key]
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", type=Path, default=ROOT / "target/debug/powerio")
    parser.add_argument("--corpus", type=Path, default=ROOT / "target/ac-power-flow-corpus")
    parser.add_argument("--output", type=Path, required=True, help="directory for the reports and written cases")
    parser.add_argument("--source", action="append", help="reference case ID, repeat to select more than one")
    parser.add_argument("--target", action="append", help="target format, repeat to select more than one")
    parser.add_argument("--previous", type=Path, help="compare against an earlier report.json")
    args = parser.parse_args()
    # Resolve all paths before a simulator can change its process working directory.
    binary, corpus, work = args.powerio.resolve(), args.corpus.resolve(), args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    cases = json.loads((HERE / "corpus.json").read_text())
    selected = [c for c in cases if not args.source or c["id"] in args.source]
    if not selected or (args.source and set(args.source) - {c["id"] for c in selected}):
        parser.error("Unknown reference case ID")
    if args.target and set(args.target) - set().union(*TARGETS.values()):
        parser.error("Unknown target format")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    with ThreadPoolExecutor(max_workers=4) as pool:
        measured = list(pool.map(lambda c: prepare_case(binary, c, corpus, work, args.target), selected))
    requests = {key: value for _, subset, _ in measured for key, value in subset.items()}
    print(f"PowerIO prepared {len(requests)} files for external tools", flush=True)
    solved = solve_many(requests, work)
    (work / "solutions.json").write_text(json.dumps(solved, allow_nan=False) + "\n")
    report = {"schema": "powerio-ac-power-flow", "version": 1, "git_revision": revision,
              "worktree_dirty": dirty, "powerio_version": version, "tolerances": TOLERANCE,
              "solvers": sorted({result["solver"] for result in solved.values() if "solver" in result}),
              "cases": {}}
    for case, _, ready in measured:
        name = case["source"]["id"]
        reference = solved.get(f"{name}/source", record("unavailable", "Reference case is missing"))
        case["original_solution"] = reference
        for key, (target, direction, ir, element_map) in ready.items():
            case["conversions"][target][direction] = compare(reference, solved[key], ir, element_map)
        for target, pair in case["conversions"].items():
            route = f"{name}/{target}"
            if route + "/direct" in solved and route + "/round_trip" in solved:
                pair["target_vs_return"] = compare_target_and_return(pair, solved[route + "/direct"], solved[route + "/round_trip"])
        report["cases"][name] = case
    (work / "report.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    (work / "matrix.md").write_text(markdown(report) + "\n")
    if args.previous:
        previous = json.loads(args.previous.read_text())
        if (previous.get("schema"), previous.get("version")) != (report["schema"], report["version"]):
            parser.error("The previous report uses a different schema")
        (work / "changes.json").write_text(json.dumps(report_changes(previous, report), indent=2) + "\n")
    for direction in ("direct", "round_trip", "target_vs_return"):
        counts = Counter(pair[direction]["status"] for c in report["cases"].values() for pair in c["conversions"].values())
        print(direction, dict(sorted(counts.items())))
    print(work / "matrix.md")


if __name__ == "__main__":
    sys.exit(main())
