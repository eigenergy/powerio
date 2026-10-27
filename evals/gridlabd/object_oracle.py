"""Qualify explicit GLM identities, nested parents and inline references.

Compare PowerIO's source-object index with initialized native object graphs, and
check the circuit's electrical observables independently. This is frontend
validation, not canonical network conversion. Unnamed range/count declarations
are checked by class inventory; named graph edges are checked by identity.
"""

import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path

from oracle import analytic, cases, command, compare, glm, read_glm, run_glm

REFERENCE_ASSERTIONS = {"from": "source", "to": "sink", "configuration": "cfg"}


def deck() -> str:
    text = glm(*cases()["single_phase"])
    text = text.replace("module tape;", "module tape;\nmodule assert;")
    configuration = "object line_configuration { name cfg;\nz11 1+0.5j;\n}"
    text = text.replace(configuration, "")
    text = text.replace("configuration cfg;", "configuration " + configuration + ";")
    text = text.replace("object node {", "object node:17 {")
    text = text.replace("object load {", "object load:901 {")
    text = text.replace("from source; to sink;", "from node:17; to id:901;")
    recorder = 'object recorder { parent sink; property voltage_A; interval 1; file "sink.csv"; }'
    if recorder not in text:
        raise ValueError("oracle recorder template changed")
    text = text.replace(recorder, "")
    text = text.replace(
        "constant_impedance_A 20+5j;",
        "constant_impedance_A 20+5j;\n"
        + recorder.replace("parent sink;", "name sink_recorder;")
        + ";",
    )
    text = text.replace(
        "object recorder { parent source;",
        "object recorder { name source_recorder; parent source;",
    )
    text = text.replace(
        "object recorder { parent line;",
        "object recorder { name line_recorder; parent line;",
    )
    text += "object line_configuration:200..202 { z11 1+0.5j; }\nobject line_configuration:..2 { z11 1+0.5j; }\n"
    text += "object line_configuration { name cfg_other; z11 1+0.5j; }\n"
    for prop, expected in REFERENCE_ASSERTIONS.items():
        text += f"object assert {{ name check_{prop}; parent line; target {prop}; relation ==; value {expected}; }}\n"
    return "#set savefile=objects.glm\n" + text


def unquote(value: str) -> str:
    return json.loads(value) if value.startswith('"') else value


def native_graph(path: Path) -> tuple[dict, dict]:
    records = []
    for header, body in re.findall(
        r"^object ([\w.:]+) \{(.*?)^\}", path.read_text(), re.M | re.S
    ):
        fields = {
            key: unquote(value)
            for key, value in re.findall(
                r"^\s*(name|parent|from|to|configuration) ([^;]+);", body, re.M
            )
            if value.strip()
        }
        records.append((header, fields))
    aliases = {header: fields.get("name", header) for header, fields in records}
    graph = {}
    classes = Counter()
    for header, fields in records:
        cls = header.split(":")[0].split(".")[-1]
        classes[cls] += 1
        # Anonymous objects cannot serve as a cross-engine identity here.
        name = fields.get("name")
        if name is None or re.fullmatch(r"\w+:\d+", name):
            continue
        if name in graph:
            raise ValueError(f"duplicate native object name: {name}")
        graph[name] = {"class": cls, "parent": None, "references": {}}
        for prop in ("parent", "from", "to", "configuration"):
            target = fields.get(prop)
            if target in (None, "root"):
                continue
            target = aliases.get(target, target)
            if prop == "parent":
                graph[name][prop] = target
            else:
                graph[name]["references"][prop] = target
    return graph, dict(classes)


def source_graph(records: list) -> tuple[dict, dict]:
    names = {record["key"]: record["name"] for record in records}
    if len(names) != len(records):
        raise ValueError("duplicate source object key")
    nonempty_names = [name for name in names.values() if name is not None]
    if len(nonempty_names) != len(set(nonempty_names)):
        raise ValueError("duplicate source object name")

    def target_name(key):
        if key is None:
            return None
        if key not in names or names[key] is None:
            raise ValueError(
                "this named-graph oracle requires every referenced target to have a name"
            )
        return names[key]

    graph = {}
    classes = Counter()
    for record in records:
        cls = record["class"].split(".")[-1]
        classes[cls] += 1
        if record["name"] is None:
            continue
        graph[record["name"]] = {
            "class": cls,
            "parent": target_name(record["parent"]),
            "references": {
                prop: target_name(key) for prop, key in record["references"].items()
            },
        }
    return graph, dict(classes)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--indexer", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    text = deck()
    report = {
        "scope": __doc__,
        "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        directory = output / profile
        directory.mkdir()
        (directory / "model.glm").write_text(text)
        records = json.loads(
            command(
                [str(args.indexer.resolve()), str(directory / "model.glm"), profile],
                directory,
            )
        )
        expected_graph, expected_classes = source_graph(records)
        version = run_glm(
            profile,
            directory,
            args.pnnl_root.resolve(),
            ("source.csv", "sink.csv", "line.csv", "impedance.xml", "objects.glm"),
        )
        actual_graph, actual_classes = native_graph(directory / "objects.glm")
        # PNNL's saved GLM omits inherited line fields. Read the published
        # object-valued properties using native equality assertions. The tape
        # writer truncates these strings, so cannot qualify full identities.
        actual_graph["line"]["references"] = dict(REFERENCE_ASSERTIONS)
        if expected_graph != actual_graph or expected_classes != actual_classes:
            raise ValueError(
                f"{profile} graph mismatch: expected {expected_graph, expected_classes}, actual {actual_graph, actual_classes}"
            )
        electrical = compare(analytic(*cases()["single_phase"]), read_glm(directory, 1))
        if not electrical["passed"]:
            raise ValueError(f"{profile} electrical mismatch: {electrical}")
        negative = output / (profile + "-negative")
        negative.mkdir()
        wrong = text.replace(
            "target configuration; relation ==; value cfg;",
            "target configuration; relation ==; value cfg_other;",
        )
        if wrong == text:
            raise ValueError("negative assertion was not injected")
        (negative / "model.glm").write_text(wrong)
        try:
            run_glm(profile, negative, args.pnnl_root.resolve())
        except RuntimeError as error:
            if "assert failed" not in str(error) or "check_configuration" not in str(
                error
            ):
                raise
        else:
            raise ValueError("native wrong-configuration assertion was not detected")
        report["cases"][profile] = {
            "engine": version,
            "objects": records,
            "named_graph": actual_graph,
            "class_inventory": actual_classes,
            "native_reference_assertions": REFERENCE_ASSERTIONS,
            "wrong_configuration_rejected": True,
            "electrical": electrical,
            "passed": True,
        }
        print(
            profile,
            "PASS object graph, class inventory and analytic electrical reference",
        )
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
