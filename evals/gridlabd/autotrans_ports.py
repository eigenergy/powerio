"""Observe DSS AutoTrans API quantities on the qualified coil-bank equivalent.

Native GetCurrents adds the series return into common hot currents, while
retaining the series-return slots. Powers uses those adjusted currents, but
Losses uses the unadjusted primitive. Consequently sum(Powers) is not Losses.
Keep both conventions; never hide native slots or substitute boundary loss.
"""


def pairs(values):
    return [complex(a, b) for a, b in zip(values[::2], values[1::2], strict=True)]


def interfaces(native, network):
    result = {}
    banks = {t["name"].lower(): t for t in network["transformers"]}
    switches = {s["name"].lower(): s for s in network["switches"]}
    grounded = {b["id"].lower(): set(b["grounded"]) for b in network["buses"]}
    originals = {n.lower() for n in native.Circuit.AllElementNames()}

    def nodes(bus, terminals):
        return [0 if t in grounded[bus.lower()] else int(t) for t in terminals]

    for transformer in banks.values():
        meta = transformer.get("extras", {})
        if "dss_autotrans_original" not in meta:
            continue
        original = meta["dss_autotrans_original"].lower()
        if original not in originals or original in result:
            raise ValueError("absent or ambiguous original AutoTrans")
        native.Circuit.SetActiveElement(original)
        count = native.CktElement.NumTerminals()
        order = native.CktElement.NodeOrder()
        buses = [b.split(".", 1)[0].lower() for b in native.CktElement.BusNames()]
        if (count not in (2, 3) or native.CktElement.NumConductors() != 6
                or native.CktElement.NumPhases() != 3 or not native.CktElement.Enabled()
                or order[3:6] != order[6:9] or order[9:12] != [0, 0, 0]
                or (count == 3 and order[15:18] != [0, 0, 0])):
            raise ValueError("original AutoTrans outside qualified terminal profile")
        names = meta["dss_autotrans_banks"]
        contacts = meta["dss_autotrans_contacts"]
        if len(names) != 3 or len(set(names)) != 3 or len(contacts) != 2:
            raise ValueError("invalid AutoTrans equivalent inventory")
        series_bus = banks[names[0].lower()]["windings"][0]["bus"]
        virtual = []
        for side, name in enumerate(contacts):
            contact = switches[name.lower()]
            if (contact["open"] or contact.get("extras", {})
                    or contact["bus_from"].lower() != buses[side]
                    or contact["bus_to"].lower() != series_bus.lower()
                    or nodes(contact["bus_from"], contact["terminal_map_from"])
                    != order[6 * side:6 * side + 3]
                    or len(contact["terminal_map_to"]) != 3):
                raise ValueError("invalid AutoTrans series/common contact")
            virtual.append(nodes(series_bus, contact["terminal_map_to"]))
        if len(set(virtual[0] + virtual[1])) != 6:
            raise ValueError("aliased AutoTrans internal nodes")
        ports = []
        for phase, name in enumerate(names):
            bank = banks[name.lower()]
            if bank["phases"] != 1 or len(bank["windings"]) != count:
                raise ValueError("invalid AutoTrans coil bank")
            expected = [(series_bus.lower(), [virtual[0][phase], virtual[1][phase]]),
                        (buses[1], [order[6 + phase], 0])]
            if count == 3:
                expected.append((buses[2], [order[12 + phase], order[12 + (phase + 1) % 3]]))
            for winding, (bus, node) in zip(bank["windings"], expected, strict=True):
                if (winding["conn"] != "delta" or winding["bus"].lower() != bus
                        or nodes(bus, winding["terminal_map"]) != node):
                    raise ValueError("invalid AutoTrans coil connection")
            ports.append(dict(element="transformer." + name.lower(),
                              buses=[e[0] for e in expected], nodes=sum([e[1] for e in expected], [])))
        result[original] = dict(kind="autotrans", banks=ports, nodes=order, windings=count)
    return result


def select_bank(engine, port):
    if port["element"] not in {n.lower() for n in engine.Circuit.AllElementNames()}:
        raise ValueError("missing canonical AutoTrans bank")
    engine.Circuit.SetActiveElement(port["element"])
    if (not engine.CktElement.Enabled() or engine.CktElement.NumConductors() != 2
            or engine.CktElement.NodeOrder() != port["nodes"]
            or [b.split(".", 1)[0].lower() for b in engine.CktElement.BusNames()] != port["buses"]):
        raise ValueError("canonical AutoTrans coil inventory changed")


def mapping(phase, windings):
    return [phase, phase + 3, phase + 6, phase + 9,
            phase + 12, (phase + 1) % 3 + 12][:2 * windings]


def observe(engine, interface):
    count = interface["windings"]
    currents, voltages = [0j] * (6 * count), [0j] * (6 * count)
    loss = 0j
    for phase, bank in enumerate(interface["banks"]):
        select_bank(engine, bank)
        ci, cv = pairs(engine.CktElement.Currents()), pairs(engine.CktElement.Voltages())
        loss += complex(*engine.CktElement.Losses())
        for index, target in enumerate(mapping(phase, count)):
            currents[target] += ci[index]
            voltages[target] = cv[index]
    for phase in range(3):
        currents[phase + 6] += currents[phase + 3]
    powers = [v * i.conjugate() for v, i in zip(voltages, currents, strict=True)]
    return [dict(nodes=interface["nodes"][6*t:6*t+6],
                 currents=currents[6*t:6*t+6], powers=powers[6*t:6*t+6])
            for t in range(count)], loss
