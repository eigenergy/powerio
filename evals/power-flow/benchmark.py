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

from solvers import HERE, ROOT, SOLVERS, solve_many

sys.path.insert(0, str(ROOT / "evals/validation"))
from native_fidelity import write_reactance_error

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
        "ravens-json": ".json", "cim-dist": ".xml", "gridlabd": None, "gridlabd-arras": None,
    },
}
TOLERANCE = {"voltage_pu": 1e-9, "voltage_V": 1e-3, "angle_deg": math.degrees(1e-9),
             "branch_active_MW": 1e-7, "branch_reactive_Mvar": 1e-7,
             "admittance_relative_2_norm": 1e-9}
DISPLAY = {"agree": "🟢", "different": "🔴", "unavailable": "—", "solve_failed": "🔴",
           "conversion_failed": "🔴", "not_comparable": "—", "not_applicable": "—",
           "partial": "◐"}
PAIRWISE = ("direct", "round_trip", "target_vs_return")
MEASURED = {"agree", "different"}
REFERENCE_CASES = ("matpower-case14", "opendss-ieee13", "mg-ravens-case3")


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
    if format in ("gridlabd", "gridlabd-arras") and path.is_dir():
        path = path / "case.glm"
    return cli(binary, "serialize", str(path), "--from", format, "-o", str(output))


def solver_input(binary, format, path, ir, operating_point, family, evidence):
    if format not in SOLVERS and family == "transmission" and ir.is_file():
        # Keep a separate solve for formats without a native reader. Record all
        # warnings from converting the parsed model to PowerModels JSON.
        bridge = ir.with_suffix(".powermodels.json")
        evidence["project_for_solver"] = cli(binary, "convert", str(ir), "--to", "powermodels-json",
                                             "-o", str(bridge))
        if evidence["project_for_solver"]["status"] == "ok":
            return ("powermodels-via-powerio", bridge, ir, operating_point)
    return (format, path, ir, operating_point)


def convert(binary, path, src, target, directory, extension, options=()):
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
        args = ["convert", str(path), "--to", target, "-o", str(output), *options]
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
    if format == "dss" and output.is_file() and (output.parent / "source").is_dir():
        matches = files == inventory(output.parent / "source")
    elif output.is_file() and len(files) == 1:
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
    elif isinstance(old, list) and isinstance(new, list) and len(old) == len(new):
        for index, (before, after) in enumerate(zip(old, new, strict=True)):
            yield from json_differences(before, after, f"{pointer}/{index}")
    elif old != new:
        yield pointer or "/"


def check_written_data(binary, original, output, format, path):
    parsed = to_ir(binary, output, format, path)
    if parsed["status"] != "ok":
        return parsed
    before = json.loads(original.read_text())["value"]
    after = json.loads(path.read_text())["value"]
    differences = list(json_differences(before, after))
    return record("different" if differences else "agree", changed_field_count=len(differences),
                  changed_fields=differences[:200], diagnostics=parsed["diagnostics"])


def unique_labels(points, aliases, candidates):
    return {name for name in points if {name, aliases.get(name)} & candidates}


def match_labels(source, target, original_ir, element_map):
    # Accept only an exact one-to-one correspondence through stable labels,
    # the writer's element map, or explicitly named buses in both files.
    expected, actual = source["voltage"], target["voltage"]
    if expected.keys() == actual.keys():
        return {name: name for name in expected}
    if not original_ir:
        return None
    value = json.loads(original_ir.read_text())["value"]
    original = value["data"]
    if not isinstance(original, dict) or "buses" not in original:
        return None
    if value["type"] == "powerio.MulticonductorNetwork":
        # Some tools expose internal source, switch, or transformer nodes.
        # Require all original ungrounded conductors before comparing a subset.
        physical = {f"{bus['id'].lower()}.{terminal}" for bus in original["buses"]
                    for terminal in bus["terminals"] if terminal not in bus["grounded"]}
        return {name: name for name in sorted(physical)} if physical <= expected.keys() & actual.keys() else None
    by_uid = json.loads(element_map.read_text()).get("bus", {}) if element_map else {}
    result = {}
    for bus in original["buses"]:
        labels = {str(bus[key]) for key in ("id", "uid", "name") if bus.get(key)}
        identity = unique_labels(expected, source.get("aliases", {}), labels)
        output = by_uid.get(bus["uid"])
        if output:
            other = unique_labels(actual, target.get("aliases", {}), {str(output.get("key"))})
        else:
            other = unique_labels(actual, target.get("aliases", {}), labels)
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
        missing = [f"{item['solver']}: {item.get('admittance_detail', 'matrix unavailable')}"
                   for item in (source, target) if not item.get("admittance")]
        return record("unavailable", "; ".join(missing))
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
                  output_scale=scale, matrix_methods=[item.get("method") for item in (a, b)])


def compare(source, target, original_ir=None, element_map=None, pairing=None):
    if target["status"] != "solved":
        return target
    if source["status"] != "solved":
        return record("unavailable", "The reference file did not have an external solution", output_solver=target["solver"])
    expected, actual = source["voltage"], target["voltage"]
    pairing = pairing or match_labels(source, target, original_ir, element_map)
    solvers = {"original_solver": source["solver"], "output_solver": target["solver"],
               "original_reader": source.get("reader", "External solver reads the original format"),
               "output_reader": target.get("reader", "External solver reads the output format"),
               "independent_readers": not (source.get("shared_powerio_reader") or target.get("shared_powerio_reader"))}
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
    if source.get("voltage_pu") and target.get("voltage_pu"):
        pu_a, pu_b = source["voltage_pu"], target["voltage_pu"]
        difference, worst = max((abs(pu_a[a] - pu_b[b]), a) for a, b in pairing.items())
        limit = TOLERANCE["voltage_pu"]
        metrics["voltage_pu"] = record("agree" if difference <= limit else "different",
                                       maximum_difference=difference, tolerance=limit, worst_node=worst,
                                       original_voltage=pu_a[worst], output_voltage=pu_b[pairing[worst]])
    metrics.update(compare_branch_powers(source, target, pairing))
    power_flow = {m["status"] for m in metrics.values()}
    metrics["admittance_relative_2_norm"] = compare_admittance(source, target, pairing)
    statuses = {m["status"] for m in metrics.values()}
    full_nodes = len(pairing) == len(expected) == len(actual)
    status = "different" if "different" in statuses else "agree" if full_nodes and statuses == {"agree"} else "partial"
    power_flow_status = ("different" if "different" in power_flow else
                         "agree" if full_nodes and power_flow == {"agree"} else "partial")
    return record(status, power_flow_status=power_flow_status, nodes_compared=len(pairing),
                  original_nodes=len(expected), output_nodes=len(actual),
                  unmatched_original_nodes=sorted(expected.keys() - pairing.keys()),
                  unmatched_output_nodes=sorted(actual.keys() - set(pairing.values())),
                  metrics=metrics, matched_labels=pairing, **solvers)


def prepare_case(binary, case, corpus, work, selected_targets, balanced_options=()):
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
    distribution = case["family"] == "distribution"
    solver_format = "ravens-distribution" if distribution and format == "ravens-json" else case.get("solver_format", format)
    operating_point = "nominal" if case.get("companion") else "as read"
    original = root / "source.pio.json"
    parsed = to_ir(binary, source, format, original)
    result["read_original"] = parsed
    result["recover_original_files"] = recover_original(binary, case, source, files, root)
    requests[f"{case['id']}/source"] = solver_input(
        binary, solver_format, source, original, operating_point, case["family"], result)
    if case["id"] == "matpower-case14":
        path = root / "case14_mutated.m"
        original_x = write_reactance_error(source, path)
        result["deliberate_error"] = {"branch": [1, 2], "field": "reactance_pu",
                                      "before": original_x, "after": 2 * original_x}
        requests[f"{case['id']}/deliberate-error"] = ("matpower", path, None, operating_point)
    if case["id"] == "opendss-ieee13":
        requests[f"{case['id']}/pmd-source"] = ("dss-pmd", source, original, operating_point)
    if solver_format == "pandapower-json":
        requests[f"{case['id']}/lightsim-source"] = ("pandapower-lightsim", source, original, operating_point)
    if companion := case.get("companion"):
        path = corpus / companion["external"]
        if path.is_file():
            if sha(path) != companion["sha256"]:
                raise ValueError(f"Digest changed for {path.name}")
            requests[f"{case['id']}/companion"] = ("dss", path, None, "nominal")
    if parsed["status"] != "ok":
        for pair in pairs.values():
            pair["direct"] = record("conversion_failed", "PowerIO could not read the source")
        return result, requests, ready

    for target, pair in pairs.items():
        cell = root / target
        input_path, input_format = (source, format) if target == "gridfm" else (original, None)
        forward, pair["write_target"], element_map = convert(
            binary, input_path, input_format, target, cell / "target", formats[target],
            () if distribution else balanced_options,
        )
        if pair["write_target"]["status"] != "ok":
            pair["direct"] = record("conversion_failed", "PowerIO could not write the target file")
            continue
        key = f"{case['id']}/{target}"
        target_ir = cell / "target.pio.json"
        pair["check_target_data"] = check_written_data(binary, original, forward, target, target_ir)
        output_solver_format = "ravens-distribution" if distribution and target == "ravens-json" else target
        requests[key + "/direct"] = solver_input(
            binary, output_solver_format, forward, target_ir, operating_point, case["family"], pair["check_target_data"])
        if case["id"] == "opendss-ieee13" and target == "dss":
            requests[key + "/pmd-direct"] = ("dss-pmd", forward, target_ir, operating_point)
        if target == "pandapower-json":
            requests[key + "/lightsim-direct"] = ("pandapower-lightsim", forward, target_ir, operating_point)
        ready[key + "/direct"] = (target, "direct", original, element_map)
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
            binary, input_path, input_format, format, cell / "return", formats[format],
        )
        if pair["write_source_format"]["status"] != "ok":
            pair["round_trip"] = record("conversion_failed", "PowerIO could not write the source format again")
            pair["target_vs_return"] = pair["round_trip"]
            continue
        returned_ir = cell / "return.pio.json"
        ready[key + "/round_trip"] = (target, "round_trip", original, None)
        pair["check_returned_data"] = check_written_data(binary, original, back, format, returned_ir)
        requests[key + "/round_trip"] = solver_input(
            binary, solver_format, back, returned_ir, operating_point, case["family"], pair["check_returned_data"])
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


def summary_markdown(report, changes):
    def reading(comparison, *metrics):
        for key, unit in metrics:
            value = comparison.get("metrics", {}).get(key, {})
            if value.get("status") not in MEASURED:
                continue
            error = value.get("maximum_difference", value.get("relative_error"))
            return f"{error:.3g}{unit} ({'pass' if value['status'] == 'agree' else 'differs'})"
        return "—"

    cases = report["cases"]
    comparisons = [pair[direction] for case in cases.values() for pair in case["conversions"].values()
                   for direction in PAIRWISE]
    voltage_checks = [c for c in comparisons if c.get("nodes_compared", 0) > 0]
    lines = ["## Original files and new files in the same format", "",
             "| Input | Original files recovered exactly | Grid data after writing and reading new files |",
             "| --- | --- | --- |"]
    for name in REFERENCE_CASES:
        if name in cases:
            case = cases[name]
            same = case["conversions"].get(case["source"]["format"], {})
            recovery = case.get("recover_original_files", case["input"])["status"]
            data = same.get("check_target_data", {"status": "unavailable"})["status"]
            lines.append(f"| {name} | {recovery} | {data} |")
    lines += ["", "## AC power flow and admittance", "",
             f"{sum(c['input']['status'] == 'ok' for c in cases.values())} of {len(cases)} source profiles have fixtures. "
             f"{sum(c['original_solution']['status'] == 'solved' for c in cases.values())} original cases were solved. "
             f"{len(voltage_checks)} comparisons measured bus or conductor voltages, including "
             f"{sum(c['independent_readers'] for c in voltage_checks)} with independent readers for both files. "
             f"{sum(c.get('metrics', {}).get('admittance_relative_2_norm', {}).get('status') in MEASURED for c in comparisons)} "
             "comparisons measured admittance.", ""]
    if changes is not None:
        reduced = sum(change["coverage_reduced"] for change in changes["changes"])
        lines += [f"Compared with `{changes['baseline_revision'][:12]}`, {reduced} checks lost coverage. "
                  "See `changes.json` for changes in measurements, inputs, and solver versions.", ""]
    else:
        lines += ["No earlier report was available for this run.", ""]
    lines += ["Maximum absolute voltage difference and relative admittance 2-norm", "",
              "| Input | Output | Original vs converted voltage | Original vs converted admittance | "
              "Original vs converted back voltage | Converted vs converted back voltage |",
              "| --- | --- | --- | --- | --- | --- |"]
    for name, case in cases.items():
        if name not in REFERENCE_CASES:
            continue
        for target, pair in case["conversions"].items():
            direct, returned, final = [pair[d] for d in PAIRWISE]
            volts = [reading(c, ("voltage_pu", " p.u."), ("voltage_V", " V")) for c in (direct, returned, final)]
            ybus = reading(direct, ("admittance_relative_2_norm", ""))
            if volts != ["—"] * 3 or ybus != "—":
                lines.append(f"| {name} | {target} | {volts[0]} | {ybus} | {volts[1]} | {volts[2]} |")
    for name, key, description in (
        ("mg-ravens-case3", "upstream_companion_comparison", "MG-RAVENS nominal load in Tellegen vs upstream OpenDSS example"),
        ("opendss-ieee13", "same_file_in_pmd", "IEEE 13-bus source solved in OpenDSS and PowerModelsDistribution"),
        ("pandapower-example", "same_file_in_lightsim", "pandapower source solved in pandapower and LightSim2Grid"),
    ):
        if name in cases and key in cases[name]:
            result = cases[name][key]
            value = reading(result, ("voltage_V", " V"), ("voltage_pu", " p.u."))
            detail = (result.get("detail", result["status"]) if value == "—" else
                      f"{value} across {result['nodes_compared']} matched nodes")
            detail = " ".join(detail.split())[:200].rstrip(".")
            lines += ["", f"- {description} — {detail}."]
    if control := cases.get("matpower-case14", {}).get("deliberate_error"):
        result = control["comparison"].get("metrics", {}).get("admittance_relative_2_norm", {})
        residual = result.get("relative_error")
        if residual is not None:
            lines += ["", f"- Deliberate error test {'passed' if control['detected'] else 'failed'}. "
                      f"Doubling branch 1–2 reactance gave a relative admittance difference of {residual:.3g}."]
    lightsim = (cases.get("matpower-case14", {}).get("conversions", {})
                .get("pandapower-json", {}).get("in_lightsim", {}))
    if value := lightsim.get("metrics", {}).get("voltage_pu"):
        lines += ["", f"- MATPOWER and LightSim2Grid on the converted case14 — "
                  f"{value['maximum_difference']:.3g} p.u. across {lightsim['nodes_compared']} buses."]
    lines += ["", "— means that quantity was not measured. The full matrix gives all three comparisons for "
              "every route. The JSON gives each metric, tolerance, solver, and reason for missing coverage."]
    return "\n".join(lines)


def compare_target_and_return(pair, converted, returned):
    a = pair["direct"].get("matched_labels", {})
    b = pair["round_trip"].get("matched_labels", {})
    common = {label: b[original] for original, label in a.items() if original in b}
    return compare(converted, returned, pairing=common or None)


def compare_on_nodes(original, companion, labels, scope):
    for solution in (original, companion):
        if solution["status"] != "solved":
            return solution
        missing = set(labels) - solution["voltage"].keys()
        if missing:
            return record("not_comparable", f"Missing physical phase nodes: {', '.join(sorted(missing))}")
    result = compare(original, companion, pairing={label: label for label in labels})
    result["scope"] = scope
    result["original_solver_nodes"] = len(original["voltage"])
    result["output_solver_nodes"] = len(companion["voltage"])
    return result


def report_changes(previous, current):
    def measurements(report):
        checks = {}

        def compare_metrics(case, target, direction, result):
            checks[(case, target, direction, "overall")] = result["status"]
            if "independent_readers" in result:
                checks[(case, target, direction, "independent_readers")] = (
                    "agree" if result["independent_readers"] else "not_applicable")
            for metric, reading in result.get("metrics", {}).items():
                error = reading.get("maximum_difference", reading.get("relative_error"))
                checks[(case, target, direction, metric)] = (
                    reading["status"], error, reading.get("branches_compared",
                                                          reading.get("order", result["nodes_compared"]))
                    if error is not None else None,
                )

        for case, spec in report["cases"].items():
            for check in ("recover_original_files", "upstream_companion_comparison", "same_file_in_pmd",
                          "same_file_in_lightsim"):
                if check in spec:
                    compare_metrics(case, spec["source"]["format"], check, spec[check])
            for target, cell in spec["conversions"].items():
                for check in (*PAIRWISE, "check_target_data", "check_returned_data", "same_solver_pmd",
                              "in_lightsim"):
                    if check in cell:
                        compare_metrics(case, target, check, cell[check])
        return checks

    a, b = measurements(previous), measurements(current)
    measured = {"agree", "different", "partial"}
    return {
        "baseline_revision": previous["git_revision"],
        "reason_to_review": {
            "tolerances_changed": previous["tolerances"] != current["tolerances"],
            "inputs_changed": {
                name: [previous["cases"].get(name, {}).get("input"), current["cases"].get(name, {}).get("input")]
                for name in sorted(previous["cases"].keys() | current["cases"].keys())
                if previous["cases"].get(name, {}).get("input") != current["cases"].get(name, {}).get("input")
            },
            "solvers_changed": previous["solvers"] != current["solvers"],
            "conversion_options_changed": previous.get("balanced_conversion_options", []) !=
                                          current.get("balanced_conversion_options", []),
        },
        "changes": [
            {"input": case, "output_format": target, "comparison": direction, "metric": metric,
             "before": a.get(key), "after": b.get(key),
             "coverage_reduced": (
                 isinstance(a.get(key), tuple) and a[key][0] in measured and (
                     not isinstance(b.get(key), tuple)
                     or b[key][0] not in measured
                     or (a[key][2] is not None and b[key][2] is not None and b[key][2] < a[key][2])
                 )
                 or isinstance(a.get(key), str) and a[key] in measured and b.get(key) not in measured
             )}
            for key in sorted(a.keys() | b.keys()) if a.get(key) != b.get(key)
            for case, target, direction, metric in [key]
        ],
    }


def measured_differences(report):
    findings = []
    for name, case in report["cases"].items():
        for target, pair in case["conversions"].items():
            for stage in PAIRWISE:
                result = pair[stage]
                differences = {key: value for key, value in result.get("metrics", {}).items()
                               if value["status"] == "different"}
                if not differences:
                    continue
                findings.append({
                    "input": name, "output_format": target, "comparison": stage,
                    "independent_readers": result["independent_readers"], "checks": differences,
                    "diagnostic_codes": sorted({item["code"] for key in ("read_original", "write_target",
                                                                        "write_source_format", "check_target_data")
                                                for item in pair.get(key, case.get(key, {})).get("diagnostics", [])
                                                if item.get("code")}),
                    "changed_fields": pair.get("check_target_data", {}).get("changed_fields", []),
                })
    return findings


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", type=Path, default=ROOT / "target/debug/powerio")
    parser.add_argument("--corpus", type=Path, default=ROOT / "target/ac-power-flow-corpus")
    parser.add_argument("--output", type=Path, required=True, help="directory for the reports and written cases")
    parser.add_argument("--source", action="append", help="reference case ID, repeat to select more than one")
    parser.add_argument("--target", action="append", help="target format, repeat to select more than one")
    parser.add_argument("--previous", type=Path, help="compare against an earlier report.json")
    parser.add_argument("--merge-buses", metavar="SPEC",
                        help="PowerIO bus merge policy for balanced conversions (for example, switches or exact)")
    parser.add_argument("--missing-gen-cost", choices=("preserve", "require", "zero", "quadratic"),
                        help="PowerIO policy for generators without a cost row in balanced conversions")
    parser.add_argument("--default-gen-cost", metavar="C2,C1,C0",
                        help="PowerIO default cost for the quadratic policy")
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
    if (args.missing_gen_cost == "quadratic") != (args.default_gen_cost is not None):
        parser.error("--default-gen-cost is required with --missing-gen-cost quadratic")
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    version = subprocess.check_output([binary, "--version"], text=True).strip()
    balanced_options = []
    if args.merge_buses is not None:
        balanced_options += ["--merge-buses", args.merge_buses]
    if args.missing_gen_cost is not None:
        balanced_options += ["--missing-gen-cost", args.missing_gen_cost]
    if args.default_gen_cost is not None:
        balanced_options += ["--default-gen-cost", args.default_gen_cost]
    with ThreadPoolExecutor(max_workers=4) as pool:
        measured = list(pool.map(lambda c: prepare_case(binary, c, corpus, work, args.target, balanced_options), selected))
    requests = {key: value for _, subset, _ in measured for key, value in subset.items()}
    print(f"PowerIO prepared {len(requests)} files for external tools", flush=True)
    solved = solve_many(requests, work)
    (work / "solutions.json").write_text(json.dumps(solved, allow_nan=False) + "\n")
    report = {"schema": "powerio-ac-power-flow", "version": 1, "git_revision": revision,
              "worktree_dirty": dirty, "powerio_version": version, "tolerances": TOLERANCE,
              "balanced_conversion_options": balanced_options,
              "solvers": sorted({result["solver"] for result in solved.values() if "solver" in result}),
              "cases": {}}
    for case, _, ready in measured:
        name = case["source"]["id"]
        reference = solved.get(f"{name}/source", record("unavailable", "Reference case is missing"))
        case["original_solution"] = reference
        if control := case.get("deliberate_error"):
            control["comparison"] = compare(reference, solved[f"{name}/deliberate-error"])
            ybus = control["comparison"].get("metrics", {}).get("admittance_relative_2_norm", {})
            control["detected"] = ybus.get("status") == "different"
        if companion := case["source"].get("companion"):
            case["upstream_companion_comparison"] = compare_on_nodes(
                reference, solved.get(f"{name}/companion", record("unavailable", "The companion file is missing")),
                companion["nodes"], "Nine physical phase nodes, excluding the internal source and grounded neutral")
        if f"{name}/pmd-source" in solved:
            case["same_file_in_pmd"] = compare_on_nodes(
                reference, solved[f"{name}/pmd-source"], reference.get("voltage", {}),
                "All physical nodes reported by OpenDSS. PMD's internal nodes are excluded.")
        if f"{name}/lightsim-source" in solved:
            case["same_file_in_lightsim"] = compare(reference, solved[f"{name}/lightsim-source"])
        for key, (target, direction, ir, element_map) in ready.items():
            case["conversions"][target][direction] = compare(reference, solved[key], ir, element_map)
        for target, pair in case["conversions"].items():
            route = f"{name}/{target}"
            if route + "/pmd-direct" in solved:
                pair["same_solver_pmd"] = compare(solved[f"{name}/pmd-source"], solved[route + "/pmd-direct"])
            if f"{name}/pmd-source" in solved and target in ("pmd-json", "pmd-math-json") and route + "/direct" in solved:
                pair["same_solver_pmd"] = compare(solved[f"{name}/pmd-source"], solved[route + "/direct"])
            if route + "/lightsim-direct" in solved:
                key = route + "/direct"
                _, _, ir, element_map = ready[key]
                pair["in_lightsim"] = compare(reference, solved[route + "/lightsim-direct"], ir, element_map)
            if route + "/direct" in solved and route + "/round_trip" in solved:
                pair["target_vs_return"] = compare_target_and_return(pair, solved[route + "/direct"], solved[route + "/round_trip"])
        report["cases"][name] = case
    (work / "report.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    (work / "measured-differences.json").write_text(json.dumps(measured_differences(report), indent=2) + "\n")
    (work / "matrix.md").write_text(markdown(report) + "\n")
    changes = None
    if args.previous:
        previous = json.loads(args.previous.read_text())
        if (previous.get("schema"), previous.get("version")) != (report["schema"], report["version"]):
            parser.error("The previous report uses a different schema")
        changes = report_changes(previous, report)
        (work / "changes.json").write_text(json.dumps(changes, indent=2) + "\n")
    (work / "summary.md").write_text(summary_markdown(report, changes) + "\n")
    for direction in PAIRWISE:
        counts = Counter(pair[direction]["status"] for c in report["cases"].values() for pair in c["conversions"].values())
        print(direction, dict(sorted(counts.items())))
    print(work / "matrix.md")


if __name__ == "__main__":
    sys.exit(main())
