#!/usr/bin/env python3
"""Untimed dataset inventory and fresh/replay checks for the CIM bench adapter.

Run with the release wheel, psutil, and pypowsybl installed. Large fixtures stay
in the independently cloned benchmark repository. Output is a reviewable JSON
packet; performance measurements belong to cim-bench's pytest generator.
"""

import argparse
import hashlib
import json
import math
import platform
import subprocess
import sys
import tempfile
import time
import uuid
import xml.etree.ElementTree as ET
import zipfile
from collections import defaultdict
from pathlib import Path

RDF = "{http://www.w3.org/1999/02/22-rdf-syntax-ns#}"


def inventory(files):
    classes = defaultdict(set)
    manifest = []
    for name, data in files:
        manifest.append(
            {
                "name": name,
                "bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        )
        for element in ET.fromstring(data):
            identity = element.get(RDF + "ID") or element.get(RDF + "about")
            if identity:
                classes[element.tag.split("}")[-1]].add(identity.removeprefix("#"))
    return manifest, {k: len(v) for k, v in sorted(classes.items())}


def emitted_id(kind, identity):
    try:
        uuid.UUID(identity)
        return identity
    except ValueError:
        namespace = uuid.uuid5(uuid.NAMESPACE_URL, "https://powerio.dev/cgmes")
        return str(uuid.uuid5(namespace, f"{kind}:{identity}"))


def bus_identities(network):
    identities = {bus["id"]: emitted_id("bus", bus["uid"]) for bus in network.buses}
    detailed = network.detailed_connectivity or {}
    records = {
        (r["component"]["component_type"], r["component"]["local_id"]): r
        for r in detailed.get("component_metadata", [])
    }
    seen = set()
    namespace = uuid.uuid5(uuid.NAMESPACE_URL, "https://powerio.dev/cgmes")
    for bus in detailed.get("bus_breaker_buses", []):
        at = bus.get("calculated_bus")
        if at is None or at in seen:
            continue
        seen.add(at)
        component = bus["component"]
        kind, local = component["component_type"], component["local_id"]
        identity = str(uuid.uuid5(namespace, f"{kind}:{kind}/{local}"))
        for external in records.get((kind, local), {}).get("external_identifiers", []):
            if str(external.get("authority", "")).upper() == "CGMES":
                try:
                    uuid.UUID(external["value"])
                    identity = external["value"]
                    break
                except ValueError:
                    pass
        identities[at] = identity
    return identities


def rows_by_identity(network, table, buses):
    # Fresh CGMES requires UUID mRIDs. Non-UUID source identities receive a
    # deterministic replacement reported by EMIT.CGMES.VALUE_SUBSTITUTED.
    kinds = {
        "buses": "bus",
        "branches": "branch",
        "loads": "load",
        "generators": "generator",
        "shunts": "shunt",
    }

    def normalize(value):
        if isinstance(value, dict):
            if "component_type" in value and "local_id" in value:
                return {
                    **value,
                    "local_id": emitted_id(value["component_type"], value["local_id"]),
                }
            return {k: normalize(v) for k, v in value.items()}
        if isinstance(value, list):
            return [normalize(v) for v in value]
        return value

    result = {}
    for original_row in getattr(network, table):
        row = normalize(dict(original_row))
        for key in ("bus", "from_id", "to_id", "regulated_bus"):
            if row.get(key) is not None:
                row[key] = buses[row[key]]
        identity = (
            buses[row.pop("id")]
            if table == "buses"
            else emitted_id(kinds[table], row["uid"])
        )
        row["uid"] = identity
        if identity in result:
            raise AssertionError(f"duplicate {table} identity {identity}")
        result[identity] = row
    return result


def differences(left, right, path=""):
    if isinstance(left, dict) and isinstance(right, dict):
        return [
            change
            for k in sorted(left.keys() | right.keys())
            for change in differences(left.get(k), right.get(k), f"{path}/{k}")
        ]
    if isinstance(left, list) and isinstance(right, list) and len(left) == len(right):
        return [
            change
            for i, (a, b) in enumerate(zip(left, right))
            for change in differences(a, b, f"{path}/{i}")
        ]
    if isinstance(left, (float, int)) and isinstance(right, (float, int)):
        if math.isclose(left, right, rel_tol=1e-9, abs_tol=1e-9):
            return []
    if left == right:
        return []
    return [{"path": path, "before": left, "after": right}]


def diagnostic_summary(records):
    groups = defaultdict(list)
    for record in records:
        message = str(record)
        groups[message.split(":", 1)[0]].append(message)
    return {
        code: {"count": len(messages), "samples": messages[:3]}
        for code, messages in sorted(groups.items())
    }


def classify_differences(key, changes, emit_diagnostics):
    accepted, unexpected = [], []
    for table, entries in changes.items():
        for change in entries:
            identity, _, field = change["path"].strip("/").partition("/")
            item = {"table": table, **change}
            if (
                table == "buses"
                and field in ("vm", "va")
                and change["after"] == (1.0 if field == "vm" else 0.0)
                and any(
                    identity in str(d) and "different modeling authority" in str(d)
                    for d in emit_diagnostics
                )
            ):
                accepted.append(
                    {**item, "reason": "diagnosed multi-authority SvVoltage omission"}
                )
            elif (
                key == "svedala_igm_cgmes_3"
                and table == "branches"
                and field == "rate_a"
                and math.isclose(
                    change["before"], change["after"], rel_tol=5e-6, abs_tol=1e-9
                )
            ):
                accepted.append(
                    {
                        **item,
                        "reason": "known Svedala rating precision difference, <=5 ppm",
                    }
                )
            else:
                unexpected.append(item)
    return accepted, unexpected


def verify(key, oracle):
    from datasets import DATASETS
    from powerio_adapter import PowerIOAdapter

    import powerio

    dataset = DATASETS[key]
    if "ZIP" in dataset:
        with zipfile.ZipFile(dataset["ZIP"]) as archive:
            files = [
                (name, archive.read(name))
                for name in archive.namelist()
                if name.lower().endswith(".xml")
            ]
    else:
        files = [
            (path.name, path.read_bytes())
            for k, path in dataset.items()
            if k != "_metadata"
        ]
    manifest, classes = inventory(files)
    adapter = PowerIOAdapter()
    start = time.perf_counter()
    loaded = adapter.load(key)
    report = {
        "dataset": key,
        "inputs": manifest,
        "source_classes": classes,
        "native_counts": loaded.counts,
        "load_seconds_observation": time.perf_counter() - start,
        "parse_diagnostics": diagnostic_summary(loaded.module.diagnostics),
    }
    report.update(adapter.prepare_export(loaded))
    with tempfile.TemporaryDirectory(prefix="powerio-cim-verify-") as tmp:
        destination = Path(tmp) / "fresh"
        start = time.perf_counter()
        emitted = powerio.emit(loaded.fresh, "cgmes", destination)
        report["fresh_seconds_observation"] = time.perf_counter() - start
        assert emitted.fidelity == "canonical"
        fresh_files = [
            (p.name, p.read_bytes()) for p in sorted(destination.glob("*.xml"))
        ]
        assert len(fresh_files) == 4
        report["fresh_files"], report["fresh_classes"] = inventory(fresh_files)
        report["emit_diagnostics"] = diagnostic_summary(emitted.diagnostics)
        reparsed = powerio.parse(destination, format="cgmes").value
        assert reparsed.component_counts() == loaded.counts
        original = loaded.module.value
        original_buses, reparsed_buses = (
            bus_identities(original),
            bus_identities(reparsed),
        )
        report["roundtrip_differences"] = {
            table: differences(
                rows_by_identity(original, table, original_buses),
                rows_by_identity(reparsed, table, reparsed_buses),
            )
            for table in ("buses", "branches", "loads", "generators", "shunts")
        }
        report["accepted_differences"], report["unexpected_differences"] = (
            classify_differences(
                key, report["roundtrip_differences"], emitted.diagnostics
            )
        )
        replay = powerio.emit(loaded.module, "cgmes")
        assert replay.fidelity == "exact_same_format"
        actual = sorted(hashlib.sha256(a.data).hexdigest() for a in replay.artifacts)
        expected = (
            [hashlib.sha256(dataset["ZIP"].read_bytes()).hexdigest()]
            if "ZIP" in dataset
            else sorted(item["sha256"] for item in manifest)
        )
        assert actual == expected
        report["replay_byte_exact"] = True
        if oracle:
            import pypowsybl.network as pn

            archive_path = Path(tmp) / "fresh.zip"
            with zipfile.ZipFile(archive_path, "w") as archive:
                for name, data in fresh_files:
                    archive.writestr(name, data)
            network = pn.load(archive_path)
            report["powsybl_counts"] = {
                "lines": len(network.get_lines()),
                "generators": len(network.get_generators()),
                "loads": len(network.get_loads()),
                "substations": len(network.get_substations()),
            }
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cim-bench", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--powerio-revision",
        required=True,
        help="Commit used to build the installed wheel",
    )
    parser.add_argument("--no-oracle", action="store_true")
    args = parser.parse_args()
    sys.path.insert(0, str(args.cim_bench / "parsers"))
    from importlib.metadata import version

    from datasets import DATASETS

    report = {
        "platform": platform.platform(),
        "python": sys.version,
        "powerio": version("powerio"),
        "powerio_revision": args.powerio_revision,
        "cim_bench_revision": subprocess.check_output(
            ["git", "-C", str(args.cim_bench), "rev-parse", "HEAD"], text=True
        ).strip(),
        "datasets": [],
    }
    failures = []
    for key in DATASETS:
        print(f"Verifying {key}", flush=True)
        report["datasets"].append(verify(key, not args.no_oracle))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
        if report["datasets"][-1]["unexpected_differences"]:
            failures.append(key)
    if failures:
        raise SystemExit(
            f"Unexpected fresh-output differences: {', '.join(failures)}; see {args.output}"
        )


if __name__ == "__main__":
    main()
