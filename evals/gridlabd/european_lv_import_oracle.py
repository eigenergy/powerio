"""Pinned IEEE European LV snapshot through import, IR and canonical OpenDSS.

The original master selects snapshot mode. All loadshape definitions have
useactual=no before loads are created. No time-series or active-control result
is claimed. Dataset bytes stay external; every input file is checked against
the catalog's Git blob pin before copying it to a fresh output directory.
"""

import argparse
import hashlib
import json
from pathlib import Path

import opendssdirect as dss
from oracle import HERE, LOCK, add, command, compare


def pairs(values):
    return [complex(a, b) for a, b in zip(values[::2], values[1::2], strict=True)]


def run(path):
    engine = dss.NewContext()
    for flag in ("AllowChangeDir", "AllowDOScmd", "AllowEditor", "AllowForms"):
        getattr(engine.Basic, flag)(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("unpinned OpenDSS engine")
    engine.Text.Command(f'redirect "{path.resolve()}"')
    engine.Text.Command(
        "set mode=snapshot controlmode=off tolerance=1e-12 maxiterations=1000"
    )
    engine.Text.Command("solve")
    if not engine.Solution.Converged():
        raise ValueError("OpenDSS did not converge")
    return engine


def snapshot(engine, buses, elements, *, canonical, aliases=None):
    result = {}
    known_buses = {bus.lower() for bus in engine.Circuit.AllBusNames()}
    known_elements = {name.lower() for name in engine.Circuit.AllElementNames()}
    for bus in buses:
        if bus not in known_buses:
            continue  # compare reports every missing original observable.
        engine.Circuit.SetActiveBus(bus)
        for node, voltage in zip(
            engine.Bus.Nodes(), pairs(engine.Bus.Voltages()), strict=True
        ):
            add(result, f"bus.{bus}.{node}.voltage", "V", voltage)
    for name in elements:
        physical = (aliases or {}).get(name, name) if canonical else name
        if physical not in known_elements:
            continue
        engine.Circuit.SetActiveElement(physical)
        if not engine.CktElement.Enabled():
            continue
        nodes = engine.CktElement.NodeOrder()
        nc = engine.CktElement.NumConductors()
        for quantity, unit, values in [
            ("current", "A", engine.CktElement.Currents()),
            ("power", "VA", [1000 * x for x in engine.CktElement.Powers()]),
        ]:
            for i, value in enumerate(pairs(values)):
                key = f"{name}.terminal{i // nc + 1}.conductor{i % nc + 1}.node{nodes[i]}.{quantity}"
                add(result, key, unit, value)
        add(result, f"{name}.loss", "VA", complex(*engine.CktElement.Losses()))
    # Observe the source's physical feeder port. Its internal equivalent has
    # auxiliary EMF buses; those are not original circuit observables.
    source = "line._pio_vsource_source_impedance" if canonical else "vsource.source"
    if source in known_elements:
        engine.Circuit.SetActiveElement(source)
        if engine.CktElement.NumConductors() != 3:
            raise ValueError("source conductor inventory changed")
        for quantity, unit, values in [
            ("current", "A", engine.CktElement.Currents()),
            ("power", "VA", [1000 * x for x in engine.CktElement.Powers()]),
        ]:
            port = pairs(values)[3:6] if canonical else pairs(values)[:3]
            for i, value in enumerate(port):
                add(result, f"source.feeder.{i + 1}.{quantity}", unit, value)
    return result


def evaluate(source, bridge, output):
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    pin = next(f for f in catalog["families"] if f["id"] == "european_lv")["sources"][0]
    prefix = "Version8/Distrib/IEEETestCases/LVTestCase/"
    output.mkdir(parents=True, exist_ok=False)
    hashes = {}
    for item in pin["files"]:
        relative = Path(item["path"].removeprefix(prefix))
        data = (source / relative).read_bytes()
        blob = hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
        if len(data) != item["bytes"] or blob != item["git_blob_sha1"]:
            raise ValueError(f"source pin mismatch: {relative}")
        destination = output / "native" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(data)
        hashes[str(relative)] = hashlib.sha256(data).hexdigest()
    imported = output / "imported"
    bridge_result = json.loads(
        command(
            [str(bridge.resolve()), str(output / "native/Master.dss"), str(imported)],
            output,
        )
    )
    (output / "bridge.json").write_text(json.dumps(bridge_result, indent=2) + "\n")
    if (
        not bridge_result["ir_network_equal"]
        or not bridge_result["targets"]["dss"]["ok"]
    ):
        raise ValueError("import/IR/canonical DSS failed")
    native = run(output / "native/Master.dss")
    buses = [bus.lower() for bus in native.Circuit.AllBusNames()]
    elements = [
        name.lower()
        for name in native.Circuit.AllElementNames()
        if name.lower().startswith(("line.", "transformer.", "load."))
    ]
    if len(buses) != 907 or len(elements) != 961:
        raise ValueError("native feeder inventory changed")
    expected = snapshot(native, buses, elements, canonical=False)
    canonical = run(imported / "dss/case.dss")
    actual = snapshot(canonical, buses, elements, canonical=True)
    # A second canonical import must also solve and match the original.
    reimported = output / "reimported"
    command(
        [str(bridge.resolve()), str(imported / "dss/case.dss"), str(reimported)], output
    )
    again = snapshot(run(reimported / "dss/case.dss"), buses, elements, canonical=True)
    canonical.Text.Command("edit line.line1 length=20")
    canonical.Text.Command("solve")
    if not canonical.Solution.Converged():
        raise ValueError("negative-control circuit did not converge")
    wrong = snapshot(canonical, buses, elements, canonical=True)
    comparison, repeated, negative = (
        compare(expected, values) for values in (actual, again, wrong)
    )
    report = {
        "scope": __doc__,
        "repository": pin["repository"],
        "commit": pin["commit"],
        "input_sha256": hashes,
        "bridge_sha256": hashlib.sha256(bridge.read_bytes()).hexdigest(),
        "engine": native.Basic.Version(),
        "buses": len(buses),
        "elements": len(elements),
        "comparison": comparison,
        "reimport_comparison": repeated,
        "wrong_line_negative": {
            "passed": not negative["passed"],
            "comparison": negative,
        },
        "passed": comparison["passed"]
        and repeated["passed"]
        and not negative["passed"],
    }
    for name, data in [
        ("native", expected),
        ("canonical", actual),
        ("reimported", again),
        ("wrong-line", wrong),
        ("report", report),
    ]:
        (output / f"{name}.json").write_text(json.dumps(data, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--bridge", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    report = evaluate(
        args.source.resolve(), args.bridge.resolve(), args.output.resolve()
    )
    print(json.dumps({key: report[key] for key in ("passed", "buses", "elements")}))
    if not report["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
