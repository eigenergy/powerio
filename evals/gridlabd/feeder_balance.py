"""Independent external-terminal current balance for qualified P/I/Z feeders."""

import numpy as np
from oracle import FEEDER_TOLERANCES


def current_balance(snapshot, network, objects, link_classes):
    network = network.get("value", network)
    network = network.get("network", network)
    buses = {b["id"]: b for b in network["buses"] if b["id"] in objects}
    sources = {s.get("value", s)["bus"] for s in network["sources"]}
    residual = {
        (b["id"], t): 0j
        for b in buses.values()
        for t in b["terminals"]
        if t not in b.get("grounded", []) and b["id"] not in sources
    }
    throughput = dict.fromkeys(residual, 0.0)

    def root(name):
        seen = set()
        while "parent" in objects[name]:
            if name in seen:
                raise ValueError("electrical parent cycle")
            seen.add(name)
            name = objects[name]["parent"]
        return name

    def voltage(bus, terminal):
        if terminal in buses[bus].get("grounded", []):
            return 0j
        triplex = objects[bus]["class"].startswith("triplex")
        phase = "C" if triplex and terminal == "n" else "ABC"[int(terminal) - 1]
        value = complex(*snapshot[f"bus.{bus}.{phase}"]["value"])
        return -value if triplex and terminal == "2" else value

    def accumulate(bus, terminal, value):
        key = bus, terminal
        if key in residual:
            residual[key] += value
            throughput[key] += abs(value)

    for name, obj in objects.items():
        if obj["class"] not in link_classes:
            continue
        phases = "AB" if obj["class"] == "triplex_line" else "ABC"
        for end, endpoint in [("in", "from"), ("out", "to")]:
            bus = root(obj[endpoint])
            for phase in phases:
                terminal = (
                    {"A": "1", "B": "2", "C": "n"}[phase]
                    if objects[bus]["class"].startswith("triplex")
                    else str("ABC".index(phase) + 1)
                )
                value = complex(
                    *snapshot[f"link.{name}.{end}.{phase}.current"]["value"]
                )
                if terminal not in buses[bus]["terminals"] and value != 0:
                    raise ValueError("current on an absent terminal")
                accumulate(bus, terminal, value)

    def pair(bus, terminals, current):
        if len(terminals) != 2:
            raise ValueError("balance profile needs an explicit two-terminal coil")
        for terminal, sign in zip(terminals, [1, -1], strict=True):
            accumulate(bus, terminal, sign * current)

    for load in network["loads"]:
        bus = load["bus"]
        if bus not in buses:
            continue  # Private service core already appears at external ports.
        if len(load["p_nom"]) != 1 or len(load["terminal_map"]) != 2:
            raise ValueError("balance profile needs scalar load coils")
        a, b = load["terminal_map"]
        v = voltage(bus, a) - voltage(bus, b)
        s = complex(load["p_nom"][0], load["q_nom"][0])
        model = load["voltage_model"]
        if model["model"] not in (
            "constant_power",
            "constant_current",
            "constant_impedance",
        ):
            raise ValueError("unqualified external load law")
        if s == 0:
            current = 0j
        elif model["model"] == "constant_power":
            current = (s / v).conjugate()
        elif model["model"] == "constant_current":
            current = (s / model["v_nom"][0]).conjugate() * v / abs(v)
        elif model["model"] == "constant_impedance":
            current = s.conjugate() * v / model["v_nom"][0] ** 2
        else:
            raise ValueError("unqualified external load law")
        pair(bus, load["terminal_map"], current)
    for generator in network["generators"]:
        if generator["extras"]:
            raise ValueError("unqualified generator electrical extras")
        bus = generator["bus"]
        terms = generator["terminal_map"]
        if len(generator["p_nom"]) != 1 or len(terms) != 2:
            raise ValueError("balance profile needs scalar fixed generator coils")
        v = voltage(bus, terms[0]) - voltage(bus, terms[1])
        s = complex(generator["p_nom"][0], generator["q_nom"][0])
        pair(bus, terms, -(s / v).conjugate())
    for ibr in network.get("ibrs", []):
        if (
            ibr["extras"]
            or ibr["control_profile"] is not None
            or ibr["voltage_aggregation"] is not None
        ):
            raise ValueError("unqualified inverter electrical law")
        if ibr["p_min"] != ibr["p_max"] or ibr["q_min"] != ibr["q_max"]:
            raise ValueError("balance profile requires fixed inverter dispatch")
        terms = ibr["terminal_map"]
        if len(ibr["p_min"]) != 1 or len(terms) != 2:
            raise ValueError("balance profile needs scalar fixed inverter coils")
        bus = ibr["bus"]
        v = voltage(bus, terms[0]) - voltage(bus, terms[1])
        s = complex(ibr["p_min"][0], ibr["q_min"][0])
        pair(bus, terms, -(s / v).conjugate())
    for shunt in network["shunts"]:
        bus = shunt["bus"]
        terms = shunt["terminal_map"]
        y = np.array(shunt["g"]) + 1j * np.array(shunt["b"])
        current = y @ np.array([voltage(bus, t) for t in terms])
        for terminal, value in zip(terms, current, strict=True):
            accumulate(bus, terminal, value)
    atol, rtol = FEEDER_TOLERANCES["A"]
    failures = []
    maximum = 0.0
    maximum_fraction = 0.0
    for key, value in residual.items():
        error = abs(value)
        if not np.isfinite(error):
            raise ValueError("nonfinite current balance")
        budget = atol + rtol * throughput[key]
        maximum = max(maximum, error)
        maximum_fraction = max(maximum_fraction, error / budget)
        if error > budget:
            failures.append(f"{key}: residual {error:.8g} A exceeds {budget:.8g} A")
    return {
        "passed": bool(residual) and not failures,
        "observables": len(residual),
        "max_absolute_residual_A": maximum,
        "max_budget_fraction": maximum_fraction,
        "failures": failures,
        "budget": {"absolute_A": atol, "relative_to_incident_current": rtol},
    }
