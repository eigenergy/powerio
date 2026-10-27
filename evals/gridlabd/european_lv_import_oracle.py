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
from physical_ports import observe


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


def node_inventory(engine, buses):
    result = {}
    for bus in buses:
        engine.Circuit.SetActiveBus(bus)
        result[bus] = list(engine.Bus.Nodes())
    return result


def source_inventory(engine):
    engine.Circuit.SetActiveElement("vsource.source")
    count = engine.CktElement.NumConductors()
    return dict(bus=engine.CktElement.BusNames()[0].split(".", 1)[0].lower(),
                nodes=list(engine.CktElement.NodeOrder()[:count]))


def source_boundary(engine, expected):
    # Imported source equivalents can gain another internal source/branch on
    # reimport. Select one declared compiler interface at the original port;
    # never substitute an arbitrary incident feeder line or sum of bus flows.
    candidates = []
    individual = {node: [] for node in expected["nodes"]}
    for name in engine.Circuit.AllElementNames():
        key = name.lower()
        if key != "vsource.source" and not key.startswith(("line._pio_vsource_", "vsource._pio_coupled_", "vsource._pio_ideal_")):
            continue
        engine.Circuit.SetActiveElement(name)
        if not engine.CktElement.Enabled():
            continue
        count = engine.CktElement.NumConductors()
        for terminal, bus in enumerate(engine.CktElement.BusNames()):
            if key == "vsource.source" and terminal != 0:
                continue
            nodes = engine.CktElement.NodeOrder()[terminal * count : (terminal + 1) * count]
            if bus.split(".", 1)[0].lower() != expected["bus"]:
                continue
            # Reimport of a general coupled source inserts one ideal voltage
            # constraint per original conductor. Observe those actual terminal
            # currents, not inferred KCL sums or currents at auxiliary nodes.
            if key.startswith("vsource._pio_ideal_") and count == 1:
                if nodes[0] in individual:
                    individual[nodes[0]].append((name, terminal, 0))
                continue
            if sorted(nodes) != sorted(expected["nodes"]):
                continue
            if nodes == expected["nodes"]:
                order = list(range(count))
            elif len(set(nodes)) == count:
                order = [nodes.index(node) for node in expected["nodes"]]
            else:
                raise ValueError("ambiguous source conductor permutation")
            candidates.append((name, terminal, order))
    if len(candidates) == 1 and not any(individual.values()):
        name, terminal, order = candidates[0]
        return [(name, terminal, index) for index in order]
    if not candidates and len(individual) == len(expected["nodes"]) and all(len(ports) == 1 for ports in individual.values()):
        return [individual[node][0] for node in expected["nodes"]]
    raise ValueError(f"ambiguous or missing source interface at {expected}: {candidates}, {individual}")


def snapshot(engine, buses, elements, *, canonical, aliases=None, port_maps=None, bus_nodes=None, source_port=None):
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
            if bus_nodes is None or node in bus_nodes[bus]:
                add(result, f"bus.{bus}.{node}.voltage", "V", voltage)
    for name in elements:
        if canonical and name in (port_maps or {}):
            ports, loss = observe(engine, port_maps[name])
            for terminal, port in enumerate(ports, 1):
                for quantity, unit, values in [("current", "A", port["currents"]), ("power", "VA", port["powers"])]:
                    for conductor, (node, value) in enumerate(zip(port["nodes"], values, strict=True), 1):
                        add(result, f"{name}.terminal{terminal}.conductor{conductor}.node{node}.{quantity}", unit, value)
            add(result, f"{name}.loss", "VA", loss)
            continue
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
    if canonical and source_port is not None:
        ports = source_boundary(engine, source_port)
        bus, nodes = source_port["bus"], source_port["nodes"]
    else:
        equivalents = ("line._pio_vsource_source_impedance", "vsource._pio_coupled__pio_vsource_source_impedance")
        equivalent = next((name for name in equivalents if canonical and name in known_elements), None)
        source = equivalent or "vsource.source"
        terminal = 1 if equivalent else 0
        if source not in known_elements:
            return result
        engine.Circuit.SetActiveElement(source)
        count = engine.CktElement.NumConductors()
        bus = engine.CktElement.BusNames()[terminal].split(".", 1)[0].lower()
        nodes = engine.CktElement.NodeOrder()[terminal * count : (terminal + 1) * count]
        order = list(range(count))
        ports = [(source, terminal, index) for index in order]
    for i, (node, (source, terminal, index)) in enumerate(zip(nodes, ports, strict=True), 1):
        engine.Circuit.SetActiveElement(source)
        count = engine.CktElement.NumConductors()
        for quantity, unit, values in [
            ("current", "A", engine.CktElement.Currents()),
            ("power", "VA", [1000 * x for x in engine.CktElement.Powers()]),
        ]:
            value = pairs(values)[terminal * count + index]
            add(result, f"source.feeder.{bus}.conductor{i}.node{node}.{quantity}", unit, value)
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
    bus_nodes = node_inventory(native, buses)
    source_port = source_inventory(native)
    canonical = run(imported / "dss/case.dss")
    actual = snapshot(canonical, buses, elements, canonical=True, bus_nodes=bus_nodes, source_port=source_port)
    # A second canonical import must also solve and match the original.
    reimported = output / "reimported"
    command(
        [str(bridge.resolve()), str(imported / "dss/case.dss"), str(reimported)], output
    )
    again = snapshot(run(reimported / "dss/case.dss"), buses, elements, canonical=True, bus_nodes=bus_nodes, source_port=source_port)
    canonical.Text.Command("edit line.line1 length=20")
    canonical.Text.Command("solve")
    if not canonical.Solution.Converged():
        raise ValueError("negative-control circuit did not converge")
    wrong = snapshot(canonical, buses, elements, canonical=True, bus_nodes=bus_nodes, source_port=source_port)
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
