"""Pinned IEEE13/34 FBS import and IR-restored OpenDSS result qualification.

Keep native assertions, fixed taps, solver and topology. Reuse independently
observed native line coefficients for physical ports; compare raw reporting
laws and bus KCL separately. This does not qualify canonical GLM writing.
"""

import argparse
import hashlib
import json
import subprocess
from functools import partial
from pathlib import Path

import feeder_ports
from feeder_oracle import FEEDERS, instrument, read_dss, snapshot
from oracle import HERE, LOCK, TOLERANCES, add, compare, run_glm


def pinned(path, family):
    data = path.read_bytes()
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    source = next(
        s
        for f in catalog["families"]
        if f["id"] == family
        for s in f["sources"]
        if s["repository"].endswith("/gridlab-d/gridlab-d")
    )
    entry = next(f for f in source["files"] if f["path"].endswith(path.name))
    digest = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    if digest != entry["git_blob_sha1"] or len(data) != entry["bytes"]:
        raise ValueError("source blob pin mismatch")
    return data, {
        "repository": source["repository"],
        "commit": source["commit"],
        **entry,
        "sha256": hashlib.sha256(data).hexdigest(),
    }


def source_currents(values, network):
    for phase in "ABC":
        total = sum(
            complex(*values[f"{name}.{side}.{phase}.current"]["value"])
            for name, _, ends in feeder_ports.devices(network)
            for side, (bus, terminals) in zip(("from", "to"), ends, strict=True)
            if bus == feeder_ports.source_bus(network)
            and str("ABC".index(phase) + 1) in terminals
        )
        key = f"source.measured_current_{phase}"
        values.pop(key)  # Replace the separately retained raw meter reading.
        add(values, key, "A", total)


def reported_snapshot(physical, raw, network):
    """Native reported ports distinguish FBS's solve law from physical KCL."""
    result = {}
    for name, _, ends in feeder_ports.devices(network):
        volts, currents = [], []
        for (bus, terminals), suffix, sign in zip(
            ends, ("in", "out"), (1, -1), strict=True
        ):
            phases = ["ABC"[int(t) - 1] for t in terminals]
            volts.append(
                [complex(*physical[f"{bus}.voltage_{p}"]["value"]) for p in phases]
            )
            currents.append(
                [sign * complex(*raw[name][f"current_{suffix}_{p}"]) for p in phases]
            )
        feeder_ports.port_values(result, name, ends, volts, currents)
    result = {**physical, **result}
    source_currents(result, network)
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--case", choices=("ieee13", "ieee34"), required=True)
    for arg in ("source", "bridge", "pnnl-root", "output"):
        p.add_argument("--" + arg, type=Path, required=True)
    args = p.parse_args()
    data, pin = pinned(args.source, args.case)
    if not args.source.name.endswith("_FBS.glm"):
        raise ValueError("expected an original FBS input")
    native_text = data.decode()
    options = []
    dependency = None
    feeder = FEEDERS[args.case]
    if feeder["config"]:
        root = args.source.resolve().parent.parent
        config, dependency = pinned(root / feeder["config"], args.case)
        native_text = native_text.replace(
            f'#include "../{feeder["config"]}";', config.decode()
        )
        options = ["--acquisition-root", str(root)]
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "source": pin,
        "dependency": dependency,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        directory = args.output / profile
        directory.mkdir()
        proc = subprocess.run(
            [
                str(args.bridge),
                str(args.source),
                str(directory / "network.json"),
                profile,
                "--dss-output",
                str(directory / "dss"),
                *options,
            ],
            capture_output=True,
            text=True,
            check=True,
            timeout=120,
        )
        (directory / "bridge.json").write_text(proc.stdout)
        decoded = json.loads(proc.stdout)
        if not decoded["ir_network_equal"]:
            raise ValueError("IR changed the electrical network")
        network = decoded["network"]
        inventory = {
            k: len(network[k])
            for k in ("buses", "lines", "transformers", "loads", "switches")
        }
        expected_inventory = {**feeder["inventory"], "switches": 0}
        if args.case == "ieee13":
            expected_inventory.update(lines=11, switches=1)
        if inventory != expected_inventory:
            raise ValueError("pinned feeder inventory changed")
        text, outputs = instrument(native_text, network, True, feeder["stopping_V"])
        (directory / "model.glm").write_text(text)
        run_glm(profile, directory, args.pnnl_root, outputs=outputs)
        native = snapshot(directory, network)
        ports, raw, native_report = feeder_ports.snapshot(directory, network, True)
        raw["source_meter"] = {
            k: v for k, v in native.items() if k.startswith("source.")
        }
        native.update(ports)
        source_currents(native, network)
        native_reported = reported_snapshot(native, raw, network)
        actual = read_dss(directory / "dss/case.dss", network)["strict"]
        tolerances = dict(TOLERANCES)
        if args.case == "ieee34":
            maximum_voltage = max(
                v for source in network["sources"] for v in source["v_magnitude"]
            )
            tolerances["VA"] = (
                2 * maximum_voltage * TOLERANCES["A"][0],
                TOLERANCES["VA"][1],
            )
        check = partial(compare, tolerances=tolerances)
        regulator = next(
            t
            for t in network["transformers"]
            if t["extras"].get("rating_is_normalization_base")
        )
        wrong = read_dss(
            directory / "dss/case.dss",
            network,
            [
                f"edit transformer.{regulator['name']} wdg=1 tap={regulator['windings'][0]['tap'] * 1.01:.17g}"
            ],
        )["strict"]
        negative = check(native, wrong)
        checks = {
            "native_vs_import": check(native, actual),
            "native_physical_kcl": feeder_ports.kcl(native, network),
            "native_reported_kcl": feeder_ports.kcl(native_reported, network),
            "imported_kcl": feeder_ports.kcl(actual, network),
            "native_reporting": native_report["reported_current_law"],
            "wrong_tap_negative": {
                "passed": not negative["passed"],
                "mismatch": negative,
            },
        }
        report["cases"][profile] = {
            "inventory": inventory,
            "checks": checks,
            "comparison_tolerances": tolerances,
            "ir_network_equal": True,
            "passed": all(c["passed"] for c in checks.values()),
        }
        (directory / "measurements.json").write_text(
            json.dumps(
                {
                    "native": native,
                    "imported": actual,
                    "raw_native": raw,
                    "native_reported": native_reported,
                },
                indent=2,
            )
            + "\n"
        )
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(
            profile, {name: item["passed"] for name, item in checks.items()}, flush=True
        )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
