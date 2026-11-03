"""Measure declared open-transformer equivalents at every original conductor.

Only the retained winding, its named closed contacts and its external diagonal
stabilization contribute. Internal winding losses are not external losses.
"""
from physical_ports import _terminal


def pairs(values):
    return [complex(a, b) for a, b in zip(values[::2], values[1::2], strict=True)]


def interfaces(native, network):
    result = {}
    switches = {s["name"].lower(): s for s in network["switches"]}
    shunts = {s["name"].lower(): s for s in network["shunts"]}
    grounded = {b["id"].lower(): set(b["grounded"]) for b in network["buses"]}

    def nodes(bus, terminals):
        return [0 if node in grounded[bus.lower()] else int(node) for node in terminals]

    for transformer in network["transformers"]:
        metadata = transformer.get("extras", {}).get("dss_open_transformer_ports")
        if metadata is None:
            continue
        original = "transformer." + transformer["name"].lower()
        ports = []
        for index, (port, winding) in enumerate(zip(metadata, transformer["windings"], strict=True), 1):
            external = _terminal(native, original, index)
            width = native.CktElement.NumConductors()
            if (native.CktElement.NumTerminals() != len(metadata)
                    or external != dict(bus=port["bus"].lower(), nodes=nodes(port["bus"], port["terminal_map"]))
                    or len(port["closed"]) != width):
                raise ValueError("original transformer boundary changed")
            if [not native.CktElement.IsOpen(index, c + 1) for c in range(width)] != port["closed"]:
                raise ValueError("original transformer conductor states changed")
            represented = len(winding["terminal_map"])
            if (len(port["contacts"]) != represented
                    or port["moved"] != any(not value for value in port["closed"][:represented])):
                raise ValueError("inconsistent moved transformer port")
            if not port["moved"] and (winding["bus"].lower() != external["bus"]
                    or nodes(winding["bus"], winding["terminal_map"]) != external["nodes"][:represented]):
                raise ValueError("unmoved transformer winding changed boundary")
            internal = nodes(winding["bus"], winding["terminal_map"])
            if len(internal) < width:
                internal += [0] * (width - len(internal))
            contacts = []
            for conductor, name in enumerate(port["contacts"]):
                required = port["moved"] and port["closed"][conductor] and external["nodes"][conductor] != 0
                if (name is not None) != required:
                    raise ValueError("missing or unexpected closed transformer contact")
                if name is None:
                    contacts.append(None)
                    continue
                switch = switches[name.lower()]
                if (switch["open"] or switch.get("extras", {}) or not port["closed"][conductor]
                        or switch["bus_from"].lower() != external["bus"]
                        or nodes(switch["bus_from"], switch["terminal_map_from"]) != [external["nodes"][conductor]]
                        or switch["bus_to"].lower() != winding["bus"].lower()
                        or nodes(switch["bus_to"], switch["terminal_map_to"]) != [internal[conductor]]):
                    raise ValueError("invalid typed transformer contact")
                contacts.append(dict(element="line." + name.lower(), terminal=1,
                                     bus=external["bus"], nodes=[external["nodes"][conductor]]))
            stabilization = shunts[port["stabilization"].lower()]
            expected_g = [[1e-12 if a == b else 0 for b in range(width)] for a in range(width)]
            if (stabilization["bus"].lower() != external["bus"]
                    or nodes(stabilization["bus"], stabilization["terminal_map"]) != external["nodes"]
                    or stabilization["g"] != expected_g
                    or any(v != 0 for row in stabilization["b"] for v in row)):
                raise ValueError("invalid typed transformer stabilization")
            ports.append(dict(external=external, internal=dict(bus=winding["bus"].lower(), nodes=internal),
                              closed=port["closed"], moved=port["moved"], contacts=contacts,
                              stabilization="reactor." + stabilization["name"].lower()))
        result[original] = dict(kind="open_transformer", element=original, ports=ports)
    return result


def quantities(engine, element, terminal, expected):
    if _terminal(engine, element, terminal) != expected:
        raise ValueError(f"canonical transformer boundary changed: {element}")
    count = engine.CktElement.NumConductors()
    start = (terminal - 1) * count
    return pairs(engine.CktElement.Currents())[start:start + count], [v * 1000 for v in pairs(engine.CktElement.Powers())[start:start + count]]


def observe(engine, interface):
    result = []
    loss = 0j
    for terminal, port in enumerate(interface["ports"], 1):
        current, power = quantities(engine, port["stabilization"], 1, port["external"])
        tc, tp = quantities(engine, interface["element"], terminal, port["internal"])
        for conductor, closed in enumerate(port["closed"]):
            contact = port["contacts"][conductor] if conductor < len(port["contacts"]) else None
            if contact is not None:
                cc, cp = quantities(engine, contact["element"], 1, dict(bus=contact["bus"], nodes=contact["nodes"]))
                current[conductor] += cc[0]
                power[conductor] += cp[0]
            elif closed or not port["moved"]:
                # A closed grounded return stays directly on earth. Delta's
                # unused slot contributes zero even if its switch flag opens.
                current[conductor] += tc[conductor]
                power[conductor] += tp[conductor]
        result.append(dict(nodes=port["external"]["nodes"], currents=current, powers=power))
        loss += sum(power)
    return result, loss
