"""Declared full-shunt equivalent boundaries, checked independently in native DSS.

Only explicitly generated components contribute. No whole-bus KCL substitution,
internal losses, or intersection with available source ports is permitted.
"""

from physical_ports import _terminal


def interfaces(native, network):
    result = {}
    names = [b["id"].lower() for b in network["buses"]]
    for table in ("lines", "switches", "transformers", "shunts", "loads", "generators", "sources", "ibrs"):
        names.extend(item["name"].lower() for item in network.get(table, []))
    grounded = {b["id"].lower(): set(b["grounded"]) for b in network["buses"]}
    for index, shunt in enumerate(network["shunts"]):
        original = shunt.get("extras", {}).get("dss_original_element", "").lower()
        if not original.startswith("generator."):
            continue
        port = _terminal(native, original, 1)
        nodes = [0 if t in grounded[shunt["bus"].lower()] else int(t) for t in shunt["terminal_map"]]
        if (native.CktElement.NumTerminals() != 1 or port["bus"] != shunt["bus"].lower()
                or nodes != port["nodes"] or len(nodes) != 3 or 0 in nodes or len(set(nodes)) != 3):
            raise ValueError(f"unsupported full-shunt boundary: {original}")
        matrix = [[complex(g, b) for g, b in zip(gr, br, strict=True)]
                  for gr, br in zip(shunt["g"], shunt["b"], strict=True)]
        if all(matrix[i][j] == matrix[j][i] for i in range(3) for j in range(3)):
            raise ValueError("this boundary profile requires a nonreciprocal full shunt")
        prefix = f"_pio_shunt_{index}_"
        while any(prefix in name for name in names):
            prefix += "_"
        parts = []
        diagonal = [matrix[i][i] for i in range(3)]
        pair = 0
        for i in range(3):
            for j in range(i + 1, 3):
                if matrix[i][j] == matrix[j][i] == 0:
                    continue
                shift = 2 * (abs(matrix[i][j]) + abs(matrix[j][i]))
                diagonal[i] -= shift
                diagonal[j] -= shift
                auxiliary = f"{prefix}bus_{pair}"
                parts.extend([
                    dict(element=f"vsource._pio_coupled_{prefix}pair_{pair}",
                         terminals=[dict(bus=port["bus"], nodes=[nodes[i], nodes[j], 0]),
                                    dict(bus=auxiliary, nodes=[1, 2, 3])], external=[1]),
                    dict(element=f"line.{prefix}pair_{pair}",
                         terminals=[dict(bus=auxiliary, nodes=[1, 2]),
                                    dict(bus=port["bus"], nodes=[0, 0])], external=[2]),
                ])
                pair += 1
        for i, value in enumerate(diagonal):
            if value != 0:
                parts.append(dict(element=f"reactor.{prefix}diagonal_{i}",
                                  terminals=[dict(bus=port["bus"], nodes=[nodes[i]]),
                                             dict(bus=port["bus"], nodes=[0])], external=[1, 2]))
        if original in result:
            raise ValueError(f"duplicate full-shunt provenance {original}")
        result[original] = dict(kind="full_shunt", **port, parts=parts)
    return result


def observe(engine, interface):
    currents = {node: 0j for node in interface["nodes"]}
    powers = currents.copy()
    for part in interface["parts"]:
        for terminal, expected in enumerate(part["terminals"], 1):
            actual = _terminal(engine, part["element"], terminal)
            if actual != expected or engine.CktElement.NumTerminals() != len(part["terminals"]):
                raise ValueError(f"changed full-shunt equivalent boundary: {part}")
            if terminal not in part["external"]:
                continue
            count = engine.CktElement.NumConductors()
            start = (terminal - 1) * count
            for data, target, scale in ((engine.CktElement.Currents(), currents, 1),
                                        (engine.CktElement.Powers(), powers, 1000)):
                values = [complex(a, b) * scale for a, b in zip(data[::2], data[1::2], strict=True)]
                for offset, node in enumerate(actual["nodes"]):
                    if node != 0:
                        target[node] += values[start + offset]
    return [dict(nodes=interface["nodes"], currents=list(currents.values()),
                 powers=list(powers.values()))], sum(powers.values())


def reimport_interfaces(interfaces, network):
    """Follow the reader's explicit source-impedance branch at the same boundary.

    Reimport lowers each coupled Vsource into a typed branch and ideal voltage
    constraints. The branch's opposite terminal remains the original boundary.
    """
    import copy

    result = copy.deepcopy(interfaces)
    grounded = {b["id"].lower(): set(b["grounded"]) for b in network["buses"]}
    for interface in result.values():
        if not isinstance(interface, dict) or interface.get("kind") != "full_shunt":
            continue
        for part in interface["parts"]:
            if not part["element"].startswith("vsource."):
                continue
            name = "_pio_vsource_" + part["element"].removeprefix("vsource.") + "_impedance"
            lines = [line for line in network["lines"] if line["name"].lower() == name]
            if len(lines) != 1:
                raise ValueError(f"ambiguous reimported source impedance {name}")
            line = lines[0]
            terminals = []
            for side in ("from", "to"):
                bus = line[f"bus_{side}"].lower()
                terminals.append(dict(bus=bus, nodes=[0 if t in grounded[bus] else int(t)
                                                     for t in line[f"terminal_map_{side}"]]))
            if terminals[1] != part["terminals"][0]:
                raise ValueError(f"reimported source impedance moved boundary: {name}")
            part.update(element="vsource._pio_coupled_" + name, terminals=terminals, external=[2])
    return result
