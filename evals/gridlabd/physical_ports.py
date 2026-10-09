"""Validated external interfaces of canonical multi-element equivalents.

No internal resistor or transformer loss is substituted for boundary power.
Names select a declared interface; bus and conductor inventories verify it.
"""


def _bus(name):
    return name.split(".", 1)[0].lower()


def _terminal(engine, element, terminal):
    names = {name.lower() for name in engine.Circuit.AllElementNames()}
    if element.lower() not in names:
        raise ValueError(f"missing physical interface element {element}")
    engine.Circuit.SetActiveElement(element)
    if not engine.CktElement.Enabled():
        raise ValueError(f"disabled physical interface element {element}")
    count = engine.CktElement.NumConductors()
    if not 1 <= terminal <= engine.CktElement.NumTerminals():
        raise ValueError(f"missing terminal {terminal} on {element}")
    start = (terminal - 1) * count
    return dict(
        bus=_bus(engine.CktElement.BusNames()[terminal - 1]),
        nodes=engine.CktElement.NodeOrder()[start : start + count],
    )


def interfaces(native, network, aliases):
    """Original ordered terminals mapped to explicitly identified equivalents."""
    result = {}
    originals = {name.lower() for name in native.Circuit.AllElementNames()}
    for original, physical in aliases.items():
        if original not in originals:
            raise ValueError(f"provenance names absent original element {original}")
        native.Circuit.SetActiveElement(original)
        if not native.CktElement.Enabled():
            continue
        count = native.CktElement.NumTerminals()
        result[original] = [
            dict(element=physical, terminal=t, **_terminal(native, original, t))
            for t in range(1, count + 1)
        ]
    switches = {s["name"].lower(): s for s in network["switches"]}
    grounded = {b["id"].lower(): set(b["grounded"]) for b in network["buses"]}
    for shunt in network["shunts"]:
        metadata = shunt.get("extras", {})
        if "dss_external_contacts" not in metadata:
            continue
        original = metadata["dss_original_element"].lower()
        contacts = metadata["dss_external_contacts"]
        if original in result or len(contacts) != 2:
            raise ValueError(f"ambiguous external interface for {original}")
        if original not in originals:
            raise ValueError(f"absent original open Line {original}")
        native.Circuit.SetActiveElement(original)
        if native.CktElement.NumTerminals() != 2:
            raise ValueError(f"open Line terminal inventory changed: {original}")
        count = native.CktElement.NumConductors()
        if len(shunt["terminal_map"]) != 2 * count:
            raise ValueError(f"open Line admittance interface changed: {original}")
        ports = []
        for side, contact in enumerate(contacts):
            switch = switches[contact.lower()]
            port = _terminal(native, original, side + 1)
            source_nodes = [
                0 if node in grounded[switch["bus_from"].lower()] else int(node)
                for node in switch["terminal_map_from"]
            ]
            if (
                switch["open"]
                or switch["bus_from"].lower() != port["bus"]
                or source_nodes != port["nodes"]
                or switch["bus_to"].lower() != shunt["bus"].lower()
                or switch["terminal_map_to"]
                != shunt["terminal_map"][side * count : (side + 1) * count]
                or switch.get("extras", {})
            ):
                raise ValueError(f"invalid typed open Line boundary: {original}")
            ports.append(dict(element="line." + contact.lower(), terminal=1, **port))
        result[original] = ports
    return result


def observe(engine, ports):
    """Return ordered boundary currents/powers and their total complex loss."""
    result = []
    total = 0j
    for port in ports:
        actual = _terminal(engine, port["element"], port["terminal"])
        if actual["bus"] != port["bus"] or sorted(actual["nodes"]) != sorted(
            port["nodes"]
        ):
            raise ValueError(f"canonical boundary inventory changed: {port}")
        if actual["nodes"] == port["nodes"]:
            order = list(range(len(port["nodes"])))
        elif len(set(actual["nodes"])) == len(actual["nodes"]):
            order = [actual["nodes"].index(node) for node in port["nodes"]]
        else:
            raise ValueError("ambiguous repeated-node conductor permutation")
        # _terminal selected this exact element; include only its external port.
        count = engine.CktElement.NumConductors()
        start = (port["terminal"] - 1) * count
        values = []
        for data, scale in (
            (engine.CktElement.Currents(), 1),
            (engine.CktElement.Powers(), 1000),
        ):
            pairs = [
                complex(a, b) * scale
                for a, b in zip(data[::2], data[1::2], strict=True)
            ]
            values.append([pairs[start + index] for index in order])
        currents, powers = values
        total += sum(powers)
        result.append(dict(nodes=port["nodes"], currents=currents, powers=powers))
    return result, total
