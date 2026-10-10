"""Full IEEE8500 static import through IR and OpenDSS, measured at native ports.

Uses the externally acquired, hash-pinned GLM. Both native forks use a tighter
stopping criterion and grouped full-precision recordings. No feeder bytes are
vendored. Canonical GLM service writing and edited-feeder qualification remain
separate requirements.
"""

import argparse
import csv
import hashlib
import json
import re
import subprocess
from pathlib import Path

import numpy as np
from ieee8500_reference import SOURCE_SHA256, prepare
from oracle import LOCK, add, compare, parse_complex, run_glm

NODE_CLASSES = ("node", "capacitor", "triplex_node", "triplex_load")
LINK_CLASSES = ("overhead_line", "triplex_line", "transformer", "regulator", "switch")
PORT_PROPERTIES = [
    f"{q}_{end}_{p}"
    for end in ("in", "out")
    for p in "ABC"
    for q in ("current", "power")
] + ["power_losses"]


def instrument(text, profile, objects):
    text = text.replace(
        "default_maximum_voltage_error 1e-4", "default_maximum_voltage_error 1e-12"
    )
    text += "\n#set complex_format=%.15g%+.15gj\n#set complex_output_format=RECT\n"
    spec = {
        cls: [
            "voltage_" + p
            for p in (("1", "2", "N") if cls.startswith("triplex") else "ABC")
        ]
        for cls in NODE_CLASSES
    }
    spec.update({cls: PORT_PROPERTIES for cls in LINK_CLASSES})
    if profile == "arras":
        # The pinned Arras group_recorder uses sizeof(pointer) as its output
        # buffer size and truncates complex rows. Bounded multi_recorder lists
        # retain the same full-precision observables without that native bug.
        chunks = []
        chunk = []
        for name, obj in objects.items():
            for prop in spec[obj["class"]]:
                field = f"{name}:{prop}"
                if len(",".join(chunk + [field])) > 900:
                    chunks.append(chunk)
                    chunk = []
                chunk.append(field)
        if chunk:
            chunks.append(chunk)
        for index, fields in enumerate(chunks):
            text += f'object multi_recorder {{ property {",".join(fields)}; file "records/chunk{index}.csv"; interval 1; }}\n'
        return text, {"chunks": chunks}
    for cls, props in spec.items():
        for prop in props:
            text += f'object group_recorder {{ group "class={cls}"; property {prop}; file "{cls}-{prop}.csv"; interval 1; strict true; print_units false; }}\n'
    return text, spec


def inventory(text):
    # This pinned feeder has flat objects; macro braces are not object braces.
    clean = re.sub(r"\$\{[^}]+\}", "MACRO", text)
    result = {}
    for cls, body in re.findall(r"object (\w+)(?::\d+)?\s*\{([^{}]*)\}", clean):
        fields = {
            k: (a or b)
            for k, a, b in re.findall(
                r'\b(name|parent|phases|from|to)\s+(?:"([^"]+)"|([^;\s]+))\s*;', body
            )
        }
        if cls in NODE_CLASSES + LINK_CLASSES:
            name = fields.pop("name")
            if name in result:
                raise ValueError("duplicate source identity")
            result[name] = {"class": cls, **fields}
    if (
        sum(v["class"] in NODE_CLASSES for v in result.values()) != 4879
        or sum(v["class"] in LINK_CLASSES for v in result.values()) != 4876
    ):
        raise ValueError("pinned feeder inventory changed")
    return result


def grouped(directory, spec):
    result = {}
    if "chunks" in spec:
        for index, fields in enumerate(spec["chunks"]):
            lines = (directory / f"records/chunk{index}.csv").read_text().splitlines()
            header = next(
                line.removeprefix("# ")
                for line in lines
                if line.startswith("# timestamp,")
            )
            rows = list(
                csv.DictReader(
                    [header]
                    + [line for line in lines if line and not line.startswith("#")]
                )
            )
            row = rows[-1]
            row.pop("timestamp")
            if set(row) != set(fields) or any(value is None for value in row.values()):
                raise ValueError("incomplete multi-recorder row")
            for field, value in row.items():
                name, prop = field.rsplit(":", 1)
                result.setdefault(name, {})[prop] = parse_complex(value)
        return result
    for cls, props in spec.items():
        for prop in props:
            lines = (directory / f"{cls}-{prop}.csv").read_text().splitlines()
            header = next(
                line.removeprefix("# ")
                for line in lines
                if line.startswith("# timestamp,")
            )
            rows = list(
                csv.DictReader(
                    [header]
                    + [line for line in lines if line and not line.startswith("#")]
                )
            )
            if not rows:
                raise ValueError("empty native recorder")
            for name, value in rows[-1].items():
                if name != "timestamp":
                    result.setdefault(name, {})[prop] = parse_complex(value)
    return result


def native_snapshot(rows, objects):
    if rows.keys() != objects.keys():
        raise ValueError("native recorder/object identity mismatch")
    result = {}
    for name, obj in objects.items():
        row = rows[name]
        if obj["class"] in NODE_CLASSES:
            props = ("1", "2", "N") if obj["class"].startswith("triplex") else "ABC"
            for p, prop in zip("ABC", props, strict=True):
                add(result, f"bus.{name}.{p}", "V", row[f"voltage_{prop}"])
        else:
            for end, sign in (("in", 1), ("out", -1)):
                for p in "ABC":
                    for q, unit in (("current", "A"), ("power", "VA")):
                        add(
                            result,
                            f"link.{name}.{end}.{p}.{q}",
                            unit,
                            sign * row[f"{q}_{end}_{p}"],
                        )
            # Native link.cpp makes each phase's real loss nonnegative before
            # summation. Mutual coupling can transfer real power between phases;
            # physical dissipation is instead the signed external port sum.
            physical = sum(row[f"power_in_{p}"] - row[f"power_out_{p}"] for p in "ABC")
            add(result, f"link.{name}.loss", "VA", physical)
            add(result, f"link.{name}.reported_loss", "VA", row["power_losses"])
    return result


def imported_snapshot(path, network, objects, wrong_tap=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if wrong_tap:
        t = next(t for t in network["transformers"] if t["name"] == "FEEDER_REG__A")
        e.Text.Command(
            f"edit transformer.{t['name']} wdg=1 tap={t['windings'][0]['tap'] * 1.01:.17g}"
        )
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS did not converge with controls disabled")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    def element(name):
        if e.Circuit.SetActiveElement(name) < 0:
            raise ValueError(f"missing element {name}")
        return (
            e.CktElement.NodeOrder(),
            unpack(e.CktElement.Currents()),
            1000 * unpack(e.CktElement.Powers()),
            complex(*e.CktElement.Losses()),
        )

    def bus_root(name):
        seen = set()
        while "parent" in objects[name]:
            if name in seen:
                raise ValueError("parent cycle")
            seen.add(name)
            name = objects[name]["parent"]
        return name

    transformers = {t["name"]: t for t in network["transformers"]}
    switches = {s["name"]: s for s in network["switches"]}
    result = {}
    for name, obj in objects.items():
        cls = obj["class"]
        if cls in NODE_CLASSES:
            if e.Circuit.SetActiveBus(bus_root(name)) < 0:
                raise ValueError(f"missing bus {name}")
            volts = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
            for p in "ABC":
                if cls.startswith("triplex"):
                    value = volts.get({"A": 1, "B": 2, "C": 0}[p], 0j) * (
                        -1 if p == "B" else 1
                    )
                else:
                    value = volts.get("ABC".index(p) + 1, 0j)
                add(result, f"bus.{name}.{p}", "V", value)
            continue
        currents = np.zeros((2, 3), complex)
        powers = currents.copy()
        loss = 0j

        def scatter(nodes, i, s, end, triplex=False, currents=currents, powers=powers):
            for node, iv, sv in zip(nodes, i, s, strict=True):
                idx = (
                    {1: 0, 2: 1, 0: 2}.get(node)
                    if triplex
                    else node - 1
                    if node in (1, 2, 3)
                    else None
                )
                if idx is not None:
                    currents[end, idx] += iv
                    powers[end, idx] += sv

        if cls in ("overhead_line", "triplex_line"):
            nodes, i, s, loss = element("line." + name)
            n = len(nodes) // 2
            for end in (0, 1):
                scatter(
                    nodes[end * n : (end + 1) * n],
                    i[end * n : (end + 1) * n],
                    s[end * n : (end + 1) * n],
                    end,
                    cls == "triplex_line",
                )
        elif cls == "transformer" and name not in transformers:
            # The pinned feeder has one DELTA_GWYE substation bank.
            for phase in "ABC":
                nodes, i, s, part = element(f"transformer.{name}__glm_{phase}")
                loss += part
                for end in (0, 1):
                    scatter(
                        nodes[end * 2 : (end + 1) * 2],
                        i[end * 2 : (end + 1) * 2],
                        s[end * 2 : (end + 1) * 2],
                        end,
                    )
        elif cls == "transformer":
            nodes, i, s, _ = element("transformer." + name)
            if len(nodes) != 8:
                raise ValueError("expected four-winding center-tapped equivalent")
            scatter(nodes[:2], i[:2], s[:2], 0)
            scatter(nodes[2:6], i[2:6], s[2:6], 1, True)
            # Include the core load behind primary leakage via external ports.
            loss = sum(s[:6])
        elif cls == "regulator":
            for phase in "ABC":
                coil = f"{name}__{phase}"
                nodes, i, s, part = element("transformer." + coil)
                loss += part
                scatter(nodes[:2], i[:2], s[:2], 0)
                nodes, i, s, part = element("line._pio_xfmr_" + coil)
                loss += part
                scatter(nodes[1:], i[1:], s[1:], 1)
        elif cls == "switch" and switches[name]["open"]:
            if e.Circuit.SetActiveElement("line." + name) < 0 or e.CktElement.Enabled():
                raise ValueError("ideal open contact was not disabled")
            # Disabled elements are outside OpenDSS's solved admittance matrix;
            # they have zero physical currents and no initialized NodeOrder.
        else:
            nodes, i, s, loss = element("line." + name)
            n = len(nodes) // 2
            scatter(nodes[:n], i[:n], s[:n], 0)
            if switches[name]["open"]:
                scatter(nodes[n:], i[n:], s[n:], 1)
            else:
                nodes, i, s, part = element("line._pio_switch_" + name)
                loss += part
                scatter(nodes[n:], i[n:], s[n:], 1)
        for end, label in enumerate(("in", "out")):
            for k, p in enumerate("ABC"):
                add(result, f"link.{name}.{label}.{p}.current", "A", currents[end, k])
                add(result, f"link.{name}.{label}.{p}.power", "VA", powers[end, k])
        add(result, f"link.{name}.loss", "VA", loss)
        if cls == "transformer":
            reported = (
                complex(abs(loss.real), loss.imag) if name in transformers else loss
            )
        else:
            phases = powers[0] + powers[1]
            reported = sum(complex(abs(value.real), value.imag) for value in phases)
        add(result, f"link.{name}.reported_loss", "VA", reported)
    return result, {
        "converged": True,
        "iterations": e.Solution.Iterations(),
        "control_mode": e.Solution.ControlMode(),
    }


def kcl(snapshot, network, objects):
    """External bus current balance; internal service core loads stay internal."""
    roots = {}
    for name in objects:
        root = name
        while "parent" in objects[root]:
            root = objects[root]["parent"]
        roots[name] = root
    bus_map = {b["id"]: b for b in network["buses"] if b["id"] in objects}
    residual = {
        (b["id"], t): 0j
        for b in bus_map.values()
        for t in b["terminals"]
        if t not in b.get("grounded", [])
    }

    def voltage(bus, terminal):
        if terminal in bus_map[bus].get("grounded", []):
            return 0j
        p = "ABC"[int(terminal) - 1]
        value = complex(*snapshot[f"bus.{bus}.{p}"]["value"])
        return (
            -value
            if objects[bus]["class"].startswith("triplex") and terminal == "2"
            else value
        )

    for name, obj in objects.items():
        if obj["class"] not in LINK_CLASSES:
            continue
        for end, field in (("in", "from"), ("out", "to")):
            bus = roots[obj[field]]
            for p in "ABC":
                terminal = (
                    {"A": "1", "B": "2", "C": "n"}[p]
                    if objects[bus]["class"].startswith("triplex")
                    else str("ABC".index(p) + 1)
                )
                current = complex(*snapshot[f"link.{name}.{end}.{p}.current"]["value"])
                if (bus, terminal) in residual:
                    residual[bus, terminal] += current
                elif current != 0j and terminal != "n":
                    raise ValueError("current on absent phase")
    for load in network["loads"]:
        bus = load["bus"]
        if bus not in bus_map:
            continue  # internal core already included at external transformer ports
        if (
            load["voltage_model"]["model"] != "constant_power"
            or len(load["terminal_map"]) != 2
        ):
            raise ValueError("pinned external load model changed")
        a, b = load["terminal_map"]
        power = complex(load["p_nom"][0], load["q_nom"][0])
        current = (power / (voltage(bus, a) - voltage(bus, b))).conjugate()
        for terminal, sign in ((a, 1), (b, -1)):
            if (bus, terminal) in residual:
                residual[bus, terminal] += sign * current
    for shunt in network["shunts"]:
        bus = shunt["bus"]
        terms = shunt["terminal_map"]
        current = (np.array(shunt["g"]) + 1j * np.array(shunt["b"])) @ np.array(
            [voltage(bus, t) for t in terms]
        )
        for terminal, value in zip(terms, current, strict=True):
            if (bus, terminal) in residual:
                residual[bus, terminal] += value
    source_buses = {s.get("value", s)["bus"] for s in network["sources"]}
    actual = {}
    zero = {}
    for (bus, terminal), value in residual.items():
        if bus in source_buses:
            continue
        key = f"kcl.{bus}.{terminal}"
        add(actual, key, "A", value)
        add(zero, key, "A", 0j)
    return compare(zero, actual)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for arg in ("source", "bridge", "output", "pnnl-root"):
        p.add_argument("--" + arg, type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    data = args.source.read_bytes()
    if hashlib.sha256(data).hexdigest() != SOURCE_SHA256:
        raise ValueError("IEEE8500 source pin mismatch")
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "source_sha256": SOURCE_SHA256,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "native_stopping_fraction": 1e-12,
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        directory = args.output / profile
        directory.mkdir()
        text = prepare(data, profile, True)
        objects = inventory(text)
        # The parser receives the portable static feeder. Group recording is
        # added only for the native run and is not part of its electrical data.
        source = directory / "input.glm"
        source.write_text(text)
        cmd = [
            str(args.bridge),
            str(source),
            str(directory / "network.pio.json"),
            profile,
            "--dss-output",
            str(directory / "dss"),
        ]
        proc = subprocess.run(
            cmd, capture_output=True, text=True, timeout=120, check=False
        )
        (directory / "bridge.stdout").write_text(proc.stdout)
        (directory / "bridge.stderr").write_text(proc.stderr)
        proc.check_returncode()
        bridge = json.loads(proc.stdout)
        if not bridge["ir_network_equal"]:
            raise ValueError("IR changed network")
        native_text, spec = instrument(text, profile, objects)
        (directory / "model.glm").write_text(native_text)
        outputs = (
            tuple(f"{cls}-{prop}.csv" for cls, props in spec.items() for prop in props)
            if profile == "pnnl"
            else ()
        )
        if profile == "arras":
            (directory / "records").mkdir()
            outputs = ("records/.",)
        run_glm(
            profile,
            directory,
            args.pnnl_root,
            outputs=outputs,
            open_files=65536 if profile == "arras" else None,
        )
        expected = native_snapshot(grouped(directory, spec), objects)
        actual, solve = imported_snapshot(
            directory / "dss/case.dss", bridge["network"], objects
        )
        check = compare(expected, actual)
        balance = {
            "native": kcl(expected, bridge["network"], objects),
            "imported": kcl(actual, bridge["network"], objects),
        }
        altered, _ = imported_snapshot(
            directory / "dss/case.dss", bridge["network"], objects, wrong_tap=True
        )
        negative = compare(expected, altered)
        if negative["passed"]:
            raise ValueError("wrong-tap negative control unexpectedly matched")
        original_equal = None
        if profile == "pnnl":
            original = subprocess.run(
                [
                    str(args.bridge),
                    str(args.source),
                    str(directory / "original.pio.json"),
                    profile,
                ],
                capture_output=True,
                text=True,
                timeout=120,
                check=True,
            )
            original_equal = json.loads(original.stdout)["network"] == bridge["network"]
            if not original_equal:
                raise ValueError(
                    "portable static input changed original imported circuit"
                )
        report["cases"][profile] = {
            "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
            "native_input_sha256": hashlib.sha256(native_text.encode()).hexdigest(),
            "comparison": check,
            "bus_current_balance": balance,
            "wrong_tap_negative": {"passed": True, "mismatch": negative},
            "original_import_equal": original_equal,
            "solve": solve,
            "ir_network_equal": True,
        }
        (directory / "measurements.json").write_text(
            json.dumps({"native": expected, "imported": actual}, indent=2) + "\n"
        )
        print(
            profile,
            check["passed"],
            check["observables"],
            check["max_absolute_error"],
            check["failures"][:3],
            flush=True,
        )
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    report["passed"] = all(
        c["comparison"]["passed"]
        and all(balance["passed"] for balance in c["bus_current_balance"].values())
        and c["wrong_tap_negative"]["passed"]
        and c["ir_network_equal"]
        for c in report["cases"].values()
    )
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
