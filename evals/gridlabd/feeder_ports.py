"""Device ports for the pinned IEEE13/IEEE34/IEEE37/IEEE123 feeders (not a general GLM reader).

Topology/phase identities come from the converted network. Electrical reference
values come only from native recorder output and native impedance dumps.
Currents enter both terminals; loss is the sum of terminal complex powers.
"""

import re
import xml.etree.ElementTree as ET

import numpy as np
from oracle import add, compare, parse_complex, recorded
from shunt_measurement import PAIRS, loads


def source_bus(network):
    if len(network["sources"]) != 1:
        raise ValueError("expected one feeder source")
    return network["sources"][0]["bus"]


def devices(network):
    for line in network["lines"]:
        yield (
            line["name"],
            "line",
            [
                (line["bus_" + side], line["terminal_map_" + side])
                for side in ("from", "to")
            ],
        )
    for transformer in network["transformers"]:
        yield (
            transformer["name"],
            "transformer",
            [
                (w["bus"], [t for t in w["terminal_map"] if t != "n"])
                for w in transformer["windings"]
            ],
        )


def load_objects(text):
    # Source hash is enforced by the caller; nested assertions begin after
    # these ordinary properties in the pinned fixture. No general parser.
    for body in re.findall(r"object load\s*\{([^{}]*)", text):
        props = dict(re.findall(r'(\w+)\s+"?([^;"\n]+)"?;', body))
        name = props["name"].strip()
        if "__glm_shunt_" not in name:
            yield name, props.get("parent", name).strip(), "D" in props["phases"]


def load_identity(name):
    return name.split("__glm_", 1)[0]


def instrument(text, network, original):
    outputs = []
    for i, (name, _, ends) in enumerate(devices(network)):
        base = name.rsplit("__", 1)[0]
        native_name = (
            base
            if original and re.search(rf'name\s+"?{re.escape(base)}"?\s*;', text)
            else name
        )
        props = (
            ",".join(
                f"{q}_{end}_{'ABC'[int(t) - 1]}"
                for t in ends[0][1]
                for end in ("in", "out")
                for q in ("current", "power")
            )
            + ",power_losses"
        )
        filename = f"port-{i}.csv"
        text += f'object recorder {{ parent "{native_name}"; property {props}; interval 1; file "{filename}"; }}\n'
        outputs.append(filename)
    text += 'object impedance_dump { filename "ports.xml"; }\n'
    outputs.append("ports.xml")
    for i, (name, _) in enumerate(loads(text)):
        props = ",".join(
            [f"voltage_{p}" for p in "ABC"] + [f"constant_impedance_{p}" for p in PAIRS]
        )
        filename = f"shunt-{i}.csv"
        text += f'object recorder {{ parent "{name}"; property {props}; interval 1; file "{filename}"; }}\n'
        outputs.append(filename)
    for i, (name, _, _) in enumerate(load_objects(text)):
        props = ",".join(
            [f"voltage_{p}" for p in "ABC"]
            + [
                f"constant_{law}_{p}"
                for law in ("power", "current", "impedance")
                for p in (*"ABC", *PAIRS)
            ]
        )
        filename = f"load-{i}.csv"
        text += f'object recorder {{ parent "{name}"; property {props}; interval 1; file "{filename}"; }}\n'
        outputs.append(filename)
    for i, (name, _) in enumerate(capacitor_objects(text)):
        props = ",".join(f"{q}_{p}" for q in ("voltage", "shunt") for p in "ABC")
        filename = f"capacitor-{i}.csv"
        text += f'object recorder {{ parent "{name}"; property {props}; interval 1; file "{filename}"; }}\n'
        outputs.append(filename)
    return text, tuple(outputs)


def port_values(result, name, ends, voltage, current):
    loss = 0j
    for side, (_, terminals), volts, amps in zip(
        ("from", "to"), ends, voltage, current, strict=True
    ):
        for terminal, v, a in zip(terminals, volts, amps, strict=True):
            phase = "ABC"[int(terminal) - 1]
            power = v * a.conjugate()
            add(result, f"{name}.{side}.{phase}.current", "A", a)
            add(result, f"{name}.{side}.{phase}.power", "VA", power)
            loss += power
    add(result, f"{name}.loss", "VA", loss)


def snapshot(directory, network, original):
    text = (directory / "model.glm").read_text()
    buses = {
        b["id"]: recorded(directory / f"bus-{i}.csv")
        for i, b in enumerate(network["buses"])
    }
    xml = ET.parse(directory / "ports.xml").getroot()
    native_links = {
        link.findtext("name"): link for link in xml if link.find("name") is not None
    }
    shunts = {}
    for i, (name, bus) in enumerate(loads(text)):
        row = recorded(directory / f"shunt-{i}.csv")
        amps = dict.fromkeys("ABC", 0j)
        for pair in PAIRS:
            z = row[f"constant_impedance_{pair}"]
            if z == 0:
                continue
            v = row[f"voltage_{pair[0]}"]
            if pair[1] != "N":
                v -= row[f"voltage_{pair[1]}"]
            current = v / z
            amps[pair[0]] += current
            if pair[1] != "N":
                amps[pair[1]] -= current
        link, side = name.rsplit("__glm_shunt_", 1)
        if side not in ("from", "to") or (link, side) in shunts:
            raise ValueError("unexpected generated shunt identity")
        shunts[link, side] = (bus, amps)
    result, raw = {}, {}
    reported_expected, reported_actual, physical_from = {}, {}, {}
    for i, (name, kind, ends) in enumerate(devices(network)):
        row = recorded(directory / f"port-{i}.csv")
        raw[name] = {k: [v.real, v.imag] for k, v in row.items()}
        volts = [
            np.array([buses[b][f"voltage_{'ABC'[int(t) - 1]}"] for t in ts])
            for b, ts in ends
        ]
        currents = [
            np.array([sign * row[f"current_{suffix}_{'ABC'[int(t) - 1]}"] for t in ts])
            for (_, ts), suffix, sign in zip(ends, ("in", "out"), (1, -1), strict=True)
        ]
        if (
            original
            and kind == "line"
            and native_links[name].tag in ("overhead_line", "underground_line")
        ):
            # Native reporting uses d=a even when Z and Y do not commute.
            # Reconstruct the physical pi ports from native a/b, independently
            # of PowerIO's linecode. Keep published currents/losses in raw.
            link = native_links[name]
            ts = ends[0][1]

            def matrix(m, link=link, ts=ts):
                return np.array(
                    [
                        [
                            parse_complex(link.findtext(f"{m}_matrix/{m}{p}{q}"))
                            for q in ts
                        ]
                        for p in ts
                    ]
                )

            z = matrix("b")
            yhalf = np.linalg.solve(z, matrix("a") - np.eye(len(ts)))
            series = np.linalg.solve(z, volts[0] - volts[1])
            currents = [series + yhalf @ volts[0], -series + yhalf @ volts[1]]
            reported = matrix("c") @ volts[1] + matrix("d") @ (-currents[1])
            for t, expected, physical in zip(ts, reported, currents[0], strict=True):
                phase = "ABC"[int(t) - 1]
                key = name + "." + phase
                add(reported_expected, key, "A", expected)
                add(reported_actual, key, "A", row[f"current_in_{phase}"])
                add(physical_from, key, "A", physical)
        for j, side in enumerate(("from", "to")):
            shunt = shunts.pop((name, side), None)
            if shunt:
                bus, amps = shunt
                if bus != ends[j][0]:
                    raise ValueError("shunt at wrong terminal")
                currents[j] += np.array([amps["ABC"[int(t) - 1]] for t in ends[j][1]])
        port_values(result, name, ends, volts, currents)
    if shunts:
        raise ValueError("unmeasured generated shunts")
    result.update(load_snapshot(directory))
    result.update(capacitor_snapshot(directory))
    # Canonical ports use observed series currents and native load coefficients.
    # Original native reporting is additionally checked against its own a/b/c/d
    # law; a mismatch with physical ports is retained rather than waived silently.
    report = {
        "reported_current_law": compare(reported_expected, reported_actual)
        if original
        else {"passed": True, "scope": "canonical ports measured directly"},
        "published_vs_physical_from": compare(physical_from, reported_actual)
        if original
        else None,
    }
    return result, raw, report


def read_dss(dss, network):
    result = {}
    for name, kind, ends in devices(network):
        element = kind + "." + name
        dss.Circuit.SetActiveElement(element)
        if (
            dss.CktElement.Name().lower() != element.lower()
            or dss.CktElement.NumTerminals() != 2
        ):
            raise ValueError("missing DSS device " + element)
        n = dss.CktElement.NumConductors()
        nodes = dss.CktElement.NodeOrder()

        def unpack(raw):
            return np.array(raw[::2]) + 1j * np.array(raw[1::2])

        current = unpack(dss.CktElement.Currents())
        voltage = unpack(dss.CktElement.Voltages())
        volts, amps = [], []
        for i, (_, terminals) in enumerate(ends):
            indices = [
                i * n + nodes[i * n : (i + 1) * n].index(int(t)) for t in terminals
            ]
            volts.append(voltage[indices])
            amps.append(current[indices])
        port_values(result, name, ends, volts, amps)
    groups = {}
    native_names = dss.Loads.AllNames()
    mapped = set()
    for load in network["loads"]:
        name = load["name"].lower()
        names = [
            n
            for n in native_names
            if n == name
            or (n.startswith(name + "_") and n[len(name) + 1 :] in ("1", "2", "3"))
        ]
        if not names:
            raise ValueError("missing DSS load " + name)
        group = load_identity(load["name"])
        amps, powers = groups.setdefault(
            group, (dict.fromkeys("ABC", 0j), dict.fromkeys("ABC", 0j))
        )
        for native in names:
            if native in mapped:
                raise ValueError("ambiguous DSS load identity")
            mapped.add(native)
            dss.Circuit.SetActiveElement("load." + native)
            nodes = dss.CktElement.NodeOrder()
            currents = unpack(dss.CktElement.Currents())
            voltages = unpack(dss.CktElement.Voltages())
            for node, v, current in zip(nodes, voltages, currents, strict=True):
                if node:
                    p = "ABC"[node - 1]
                    amps[p] += current
                    powers[p] += v * current.conjugate()
    if mapped != set(native_names):
        raise ValueError("unmeasured DSS loads")
    for name, (amps, powers) in groups.items():
        for p in "ABC":
            add(result, f"load.{name}.{p}.current", "A", amps[p])
            add(result, f"load.{name}.{p}.power", "VA", powers[p])
    result.update(capacitor_dss(dss, network))
    return result


def capacitor_objects(text):
    for body in re.findall(r"object capacitor\s*\{([^{}]*)", text):
        props = dict(re.findall(r'(\w+)\s+"?([^;"\n]+)"?;', body))
        name = props["name"].strip()
        yield name, props.get("parent", name).strip()


def capacitor_snapshot(directory):
    text = (directory / "model.glm").read_text()
    groups = {}
    for i, (name, _) in enumerate(capacitor_objects(text)):
        row = recorded(directory / f"capacitor-{i}.csv")
        amps, powers = groups.setdefault(
            load_identity(name), (dict.fromkeys("ABC", 0j), dict.fromkeys("ABC", 0j))
        )
        for p in "ABC":
            current = row[f"voltage_{p}"] * row[f"shunt_{p}"]
            amps[p] += current
            powers[p] += row[f"voltage_{p}"] * current.conjugate()
    result = {}
    for name, (amps, powers) in groups.items():
        for p in "ABC":
            add(result, f"capacitor.{name}.{p}.current", "A", amps[p])
            add(result, f"capacitor.{name}.{p}.power", "VA", powers[p])
    return result


def capacitor_dss(dss, network):
    groups = {}
    for shunt in network["shunts"]:
        name = "capacitor." + shunt["name"]
        dss.Circuit.SetActiveElement(name)
        if dss.CktElement.Name().lower() != name.lower():
            raise ValueError("missing DSS capacitor " + name)
        amps, powers = groups.setdefault(
            load_identity(shunt["name"]),
            (dict.fromkeys("ABC", 0j), dict.fromkeys("ABC", 0j)),
        )
        n = dss.CktElement.NumConductors()
        raw_i, raw_v = dss.CktElement.Currents(), dss.CktElement.Voltages()
        for i, node in enumerate(dss.CktElement.NodeOrder()[:n]):
            if node:
                p = "ABC"[node - 1]
                current = complex(*raw_i[2 * i : 2 * i + 2])
                voltage = complex(*raw_v[2 * i : 2 * i + 2])
                amps[p] += current
                powers[p] += voltage * current.conjugate()
    result = {}
    for name, (amps, powers) in groups.items():
        for p in "ABC":
            add(result, f"capacitor.{name}.{p}.current", "A", amps[p])
            add(result, f"capacitor.{name}.{p}.power", "VA", powers[p])
    return result


def load_snapshot(directory):
    groups = {}
    reference = dict(
        zip(
            "ABC",
            np.exp(1j * np.array([0, -2 * np.pi / 3, 2 * np.pi / 3])),
            strict=True,
        )
    )
    for i, (name, bus, delta) in enumerate(
        load_objects((directory / "model.glm").read_text())
    ):
        row = recorded(directory / f"load-{i}.csv")
        group = load_identity(name)
        amps, powers = groups.setdefault(
            group, (dict.fromkeys("ABC", 0j), dict.fromkeys("ABC", 0j))
        )
        channels = [
            (p, PAIRS[j] if delta else p + "N") for j, p in enumerate("ABC")
        ] + [(p, p) for p in PAIRS]
        for suffix, pair in channels:
            v = row[f"voltage_{pair[0]}"]
            vref = reference[pair[0]]
            if pair[1] != "N":
                v -= row[f"voltage_{pair[1]}"]
                vref -= reference[pair[1]]
            power, current, z = [
                row[f"constant_{law}_{suffix}"]
                for law in ("power", "current", "impedance")
            ]
            if power == current == z == 0:
                continue
            if v == 0:
                raise ValueError("energized load has zero voltage")
            current = (
                (power / v).conjugate()
                + current * (v / abs(v)) / (vref / abs(vref))
                + (v / z if z else 0)
            )
            for p, sign in ((pair[0], 1), (pair[1], -1)):
                if p != "N":
                    amps[p] += sign * current
                    powers[p] += row[f"voltage_{p}"] * (sign * current).conjugate()
    result = {}
    for name, (amps, powers) in groups.items():
        for p in "ABC":
            add(result, f"load.{name}.{p}.current", "A", amps[p])
            add(result, f"load.{name}.{p}.power", "VA", powers[p])
    return result


def kcl(snapshot, network):
    residual = {
        (b["id"], "ABC"[int(t) - 1]): 0j
        for b in network["buses"]
        for t in b["terminals"]
        if t != "n"
    }

    def value(key):
        return complex(*snapshot[key]["value"])

    for name, _, ends in devices(network):
        for side, (bus, terminals) in zip(("from", "to"), ends, strict=True):
            for t in terminals:
                p = "ABC"[int(t) - 1]
                residual[bus, p] += value(f"{name}.{side}.{p}.current")
    load_buses = {}
    for load in network["loads"]:
        name = load_identity(load["name"])
        if name in load_buses and load_buses[name] != load["bus"]:
            raise ValueError("ambiguous load bus")
        load_buses[name] = load["bus"]
    for name, bus in load_buses.items():
        for p in "ABC":
            amps = value(f"load.{name}.{p}.current")
            if (bus, p) in residual:
                residual[bus, p] += amps
            elif amps != 0:
                raise ValueError("load current on absent phase")
    capacitor_buses = {load_identity(sh["name"]): sh["bus"] for sh in network["shunts"]}
    for name, bus in capacitor_buses.items():
        for p in "ABC":
            amps = value(f"capacitor.{name}.{p}.current")
            if (bus, p) in residual:
                residual[bus, p] += amps
            elif amps != 0:
                raise ValueError("capacitor current on absent phase")
    for p in "ABC":
        residual[source_bus(network), p] -= value(f"source.measured_current_{p}")
    expected, actual = {}, {}
    for (bus, p), amps in residual.items():
        add(expected, bus + "." + p, "A", 0j)
        add(actual, bus + "." + p, "A", amps)
    return compare(expected, actual)
