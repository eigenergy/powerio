"""PNNL IEEE9500 balanced/unbalanced imports, IR, and physical DSS ports.

Supply the externally cached CIMHub ieee9500 directory. No feeder is vendored.
Native controls retain their original fixed states and future outage schedules.
"""

import argparse
import copy
import hashlib
import json
import math
import re
import subprocess
from collections import defaultdict
from pathlib import Path

import numpy as np
from feeder_balance import current_balance
from ieee8500_import_oracle import PORT_PROPERTIES, grouped
from oracle import FEEDER_TOLERANCES, LOCK, TOLERANCES, add, command, compare, run_glm

NODES = (
    "node",
    "meter",
    "substation",
    "load",
    "capacitor",
    "triplex_node",
    "triplex_meter",
    "triplex_load",
)
LINKS = (
    "overhead_line",
    "underground_line",
    "triplex_line",
    "transformer",
    "regulator",
    "switch",
    "series_reactor",
)
SOURCE_HASHES = {
    "bal": {
        "base": "afc1f0c09054897a70e58f357489dc0a578e5cde9dfd4c32e83d9e08cdd940b4",
        "run": "9850d3cb21be3a99e918e64ccbfadd7efd3d2bf67e990e729cea9e5a972ff72b",
        "edits2": "6e9c0104671bbad45a849b41001ef7a0027c93a14052eb18d6ff9b05a8ca2cd1",
    },
    "unbal": {
        "base": "239eaefd3fef02eb7eebffdbda6c8452aba518803e983eb038f0531dcdf2a6e6",
        "run": "9f54d733958348119f0e0175b0b50fed92ec57632f609dad5e8fc0ae7d73c7c8",
        "edits2": "6e9c0104671bbad45a849b41001ef7a0027c93a14052eb18d6ff9b05a8ca2cd1",
    },
}


def inventory(text):
    # Keep quoted strings intact while discarding comments. Braces in macros
    # are not object boundaries; nested assertions must not hide their parent.
    clean = re.sub(r"\$\{[^}]+\}", "MACRO", text)
    tokens = list(
        re.finditer(
            r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|//[^\n]*|/\*.*?\*/|[{}]|[^{}"\'/]+|/',
            clean,
            re.S,
        )
    )
    clean = "".join(
        " " * len(t[0]) if t[0].startswith(("//", "/*")) else t[0] for t in tokens
    )
    objects, aliases = {}, {}
    for ordinal, header in enumerate(
        re.finditer(r"\bobject\s+(\w+)(?::(\d+))?\s*\{", clean)
    ):
        cls, identity = header.groups()
        if cls not in NODES + LINKS:
            continue
        depth, pieces = 1, []
        for token in re.finditer(
            r'"(?:\\.|[^"\\])*"|\'(?:\\.|[^\'\\])*\'|[{}]|[^{}"\']+',
            clean[header.end() :],
        ):
            value = token[0]
            if value == "{":
                depth += 1
            elif value == "}":
                depth -= 1
                if depth == 0:
                    break
                if depth == 1:
                    pieces.append(";")
            elif depth == 1:
                pieces.append(value)
        else:
            raise ValueError("unterminated source object")
        fields = {
            k: (a or b)
            for k, a, b in re.findall(
                r'\b(name|parent|phases|from|to)\s+(?:"([^"]+)"|([^;\s]+))\s*;',
                "".join(pieces),
            )
        }
        name = fields.pop("name", None)
        if name is None:
            name = f"__glm_object_{ordinal}"
            if identity is None:
                raise ValueError(
                    "unnamed object without source identity needs explicit instrumentation"
                )
            fields["source_identity"] = f"{cls}:{identity}"
        if identity:
            aliases[f"{cls}:{identity}"] = name
        if name is None or name in objects:
            raise ValueError("missing or duplicate source identity")
        objects[name] = {"class": cls, **fields}
    for obj in objects.values():
        for field in ("from", "to", "parent"):
            if field in obj:
                obj[field] = aliases.get(obj[field], obj[field])
    return objects


def instrument(text, objects, stopping_fraction=1e-8, iteration_limit=100):
    for name, obj in objects.items():
        if "source_identity" in obj:
            pattern = r"(\bobject\s+" + re.escape(obj["source_identity"]) + r"\s*\{)"
            text, count = re.subn(
                pattern, lambda m, name=name: m[0] + f' name "{name}";', text
            )
            if count != 1:
                raise ValueError("ambiguous unnamed recorder target")
    text = text.replace(
        "stoptime '2000-01-01 0:00:00'", "stoptime '2000-01-01 0:00:01'"
    )
    text = text.replace(
        "solver_method NR;",
        f"solver_method NR; default_maximum_voltage_error {stopping_fraction:.17g};",
    )
    text += f"\n#set iteration_limit={iteration_limit};\n"
    text += "\n#set complex_format=%.17g%+.17gj\n#set double_format=%.17g\n#set complex_output_format=RECT\n"
    classes = {obj["class"] for obj in objects.values()}
    spec = {
        cls: [
            "voltage_" + p
            for p in (("1", "2", "N") if cls.startswith("triplex") else "ABC")
        ]
        for cls in NODES
        if cls in classes
    }
    spec.update({cls: PORT_PROPERTIES for cls in LINKS if cls in classes})
    for cls, props in spec.items():
        for prop in props:
            text += f'object group_recorder {{ group "class={cls}"; property {prop}; file "{cls}-{prop}.csv"; interval 1; strict true; print_units false; }}\n'
    return text, spec


def native_snapshot(rows, objects):
    if rows.keys() != objects.keys():
        raise ValueError(f"recorder identities differ: {rows.keys() ^ objects.keys()}")
    result = {}
    reported = {}
    for name, obj in objects.items():
        row = rows[name]
        if obj["class"] in NODES:
            props = ("1", "2", "N") if obj["class"].startswith("triplex") else "ABC"
            for phase, prop in zip("ABC", props, strict=True):
                add(result, f"bus.{name}.{phase}", "V", row[f"voltage_{prop}"])
        else:
            # Direct triplex matrices specify no physical neutral current.
            phases = "AB" if obj["class"] == "triplex_line" else "ABC"
            for end, sign in (("in", 1), ("out", -1)):
                for phase in phases:
                    for q, unit in (("current", "A"), ("power", "VA")):
                        add(
                            result,
                            f"link.{name}.{end}.{phase}.{q}",
                            unit,
                            sign * row[f"{q}_{end}_{phase}"],
                        )
            loss = sum(row[f"power_in_{p}"] - row[f"power_out_{p}"] for p in phases)
            add(result, f"link.{name}.loss", "VA", loss)
            reported[name] = [row["power_losses"].real, row["power_losses"].imag]
    return result, reported


def imported_snapshot(path, network, objects, wrong_tap=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("OpenDSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if wrong_tap:
        transformers = network["transformers"]
        if transformers:
            t = next(
                (t for t in transformers if any(w["tap"] != 1 for w in t["windings"])),
                transformers[0],
            )
            e.Text.Command(
                f"edit transformer.{t['name']} wdg=1 tap={t['windings'][0]['tap'] * 1.01:.17g}"
            )
        else:
            # Perturb a feeder line when all transformers use admittance
            # equivalents; an unloaded transformer would make a weak negative.
            if network.get("lines"):
                line = network["lines"][0]
                e.Text.Command(
                    f"edit line.{line['name']} length={line['length'] * 1.01:.17g}"
                )
            else:
                contact = next(
                    s for s in network["switches"] if s["name"].endswith("__glm_to")
                )
                e.Text.Command(f"edit line.{contact['name']} r1=1.1 r0=1.1")
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged() or e.Solution.ControlMode() != -1:
        raise ValueError("DSS did not converge with controls disabled")
    element_names = {n.lower() for n in e.Circuit.AllElementNames()}
    raw_equivalent_ports = {}

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    def element(name):
        if e.Circuit.SetActiveElement(name) < 0:
            raise ValueError(f"missing element {name}")
        values = (
            e.CktElement.NodeOrder(),
            unpack(e.CktElement.Currents()),
            1000 * unpack(e.CktElement.Powers()),
        )
        if "_pio_coupled_" in name.lower():
            raw_equivalent_ports[name] = {
                "buses": e.CktElement.BusNames(),
                "nodes": values[0],
                "current_A": [[v.real, v.imag] for v in values[1]],
                "power_VA": [[v.real, v.imag] for v in values[2]],
            }
        return values

    def root(name):
        seen = set()
        while "parent" in objects[name]:
            if name in seen:
                raise ValueError("parent cycle")
            seen.add(name)
            name = objects[name]["parent"]
        return name

    current_forms = []
    original_buses = {
        name.lower() for name, obj in objects.items() if obj["class"] in NODES
    }

    def current_form_ends(name, obj):
        if obj["class"] not in ("overhead_line", "underground_line"):
            return None
        if e.Circuit.SetActiveElement("line." + name) < 0:
            raise ValueError(f"missing original line {name}")
        buses = [b.split(".")[0].lower() for b in e.CktElement.BusNames()]
        if len(buses) != 2:
            raise ValueError("line terminal count changed")
        aux = buses[1]
        if aux in original_buses:
            return None
        if aux.startswith("_pio_current_"):
            separator = "_"
        elif "__glm_current" in aux:
            separator = "__"
        else:
            return None
        # Generated names identify a candidate, but actual terminal topology
        # must confirm it before any physical quantities are aggregated.
        parts = []
        for side, branch in [
            ("from", "line." + name),
            ("to", "line." + aux + separator + "to"),
        ]:
            if e.Circuit.SetActiveElement(branch) < 0:
                raise ValueError(f"missing current-form branch {branch}")
            actual_buses = [b.split(".")[0].lower() for b in e.CktElement.BusNames()]
            if actual_buses != [root(obj[side]).lower(), aux]:
                raise ValueError("current-form physical endpoint changed")
            n = e.CktElement.NumConductors()
            nodes, amps, va = element(branch)
            parts.append((nodes[:n], amps[:n], va[:n]))
            shunt = "load." + aux + separator + side + "_shunt"
            if e.Circuit.SetActiveElement(shunt) < 0:
                raise ValueError(f"missing current-form endpoint shunt {shunt}")
            if (
                e.CktElement.BusNames()[0].split(".")[0].lower()
                != root(obj[side]).lower()
            ):
                raise ValueError("current-form shunt endpoint changed")
            n = e.CktElement.NumConductors()
            nodes, amps, va = element(shunt)
            parts.append((nodes[:n], amps[:n], va[:n]))
        current_forms.append(name)
        return parts

    transformers = {t["name"]: t for t in network.get("transformers", [])}
    switches = {s["name"]: s for s in network.get("switches", [])}
    inactive_lines = set()
    for record in network.get("untyped", []):
        if record["class"].lower() != "line":
            continue
        values = {k.lower(): v for k, v in record["props"] if k is not None}
        if values.get("enabled", "").lower() in ("no", "false", "0"):
            inactive_lines.add(record["name"].lower())
    omitted_inactive = []
    line_end_loads = defaultdict(list)
    for load in network["loads"]:
        if "__glm_shunt_" not in load["name"]:
            continue
        line, side = load["name"].split("__glm_shunt_", 1)
        line_end_loads[line, side.split("_")[0]].append(load)

    result = {}
    missing_buses = []
    for name, obj in objects.items():
        cls = obj["class"]
        if cls in NODES:
            if e.Circuit.SetActiveBus(root(name)) < 0:
                # Keep expected native measurements. compare() must report these
                # as missing, rather than copying native initialized voltages.
                missing_buses.append(name)
                continue
            volts = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
            for phase in "ABC":
                if cls.startswith("triplex"):
                    value = volts.get({"A": 1, "B": 2, "C": 0}[phase], 0j) * (
                        -1 if phase == "B" else 1
                    )
                else:
                    value = volts.get("ABC".index(phase) + 1, 0j)
                add(result, f"bus.{name}.{phase}", "V", value)
            continue
        currents = np.zeros((2, 3), complex)
        powers = currents.copy()

        def scatter(
            nodes, amps, va, end, triplex=False, currents=currents, powers=powers
        ):
            for node, iv, sv in zip(nodes, amps, va, strict=True):
                index = (
                    {1: 0, 2: 1, 0: 2}.get(node)
                    if triplex
                    else node - 1
                    if node in (1, 2, 3)
                    else None
                )
                if index is not None:
                    currents[end, index] += iv
                    powers[end, index] += sv

        current_ends = (
            None if name.lower() in inactive_lines else current_form_ends(name, obj)
        )
        if current_ends is not None:
            for part, (nodes, amps, va) in enumerate(current_ends):
                scatter(nodes, amps, va, part // 2)
        elif cls in ("transformer", "overhead_line", "underground_line") and any(
            s["name"] == name for s in network.get("shunts", [])
        ):
            for end, side in enumerate(("from", "to")):
                nodes, amps, va = element(f"line.{name}__glm_{side}")
                count = len(nodes) // 2
                scatter(nodes[:count], amps[:count], va[:count], end)
        elif cls in ("transformer", "regulator"):
            names = (
                [name]
                if name in transformers
                else [
                    n
                    for n in transformers
                    if n
                    in [f"{name}__{p}" for p in "ABC"]
                    + [f"{name}__glm_{p}" for p in "ABC"]
                ]
            )
            if not names:
                raise ValueError(f"no transformer equivalent for {name}")
            for coil in names:
                nodes, amps, va = element("transformer." + coil)
                t = transformers[coil]
                count = len(nodes) // len(t["windings"])
                for w, winding in enumerate(t["windings"]):
                    for end, endpoint in enumerate(("from", "to")):
                        if winding["bus"] == root(obj[endpoint]):
                            sl = slice(w * count, (w + 1) * count)
                            companion = "line._pio_xfmr_" + coil
                            if w == 1 and companion.lower() in element_names:
                                cn, ci, cs = element(companion)
                                half = len(cn) // 2
                                scatter(cn[half:], ci[half:], cs[half:], end)
                                continue
                            scatter(
                                nodes[sl],
                                amps[sl],
                                va[sl],
                                end,
                                objects[root(obj[endpoint])]["class"].startswith(
                                    "triplex"
                                ),
                            )
        elif (
            name.lower() in inactive_lines
            and "line." + name.lower() not in element_names
        ):
            # Reimport retains disabled lines as source-only records. Canonical
            # output diagnoses their omission. Absence is a zero electrical
            # branch, but only with this explicit inactive-state evidence.
            omitted_inactive.append(name)
        elif name in switches and switches[name]["open"]:
            if e.Circuit.SetActiveElement("line." + name) < 0 or e.CktElement.Enabled():
                raise ValueError("open contact not disabled")
        else:
            nodes, amps, va = element("line." + name)
            count = len(nodes) // 2
            companion = "line._pio_switch_" + name
            if companion.lower() in element_names:
                cn, ci, cs = element(companion)
                half = len(cn) // 2
                scatter(cn[half:], ci[half:], cs[half:], 1)
            else:
                scatter(
                    nodes[count:], amps[count:], va[count:], 1, cls == "triplex_line"
                )
            coupled = "vsource._pio_coupled_" + name
            if coupled.lower() in element_names:
                nodes, amps, va = element(coupled)
                count = len(nodes) // 2
                if cls == "triplex_line" and abs(amps[count + 2]) > 1e-8:
                    raise ValueError("auxiliary third port is not open")
            scatter(nodes[:count], amps[:count], va[:count], 0, cls == "triplex_line")
        for end, side in enumerate(("from", "to")):
            for load in line_end_loads[name, side]:
                if load["bus"] != root(obj[side]):
                    raise ValueError(
                        "line end-shunt load has the wrong physical endpoint"
                    )
                nodes, amps, va = element("load." + load["name"])
                count = e.CktElement.NumConductors()
                scatter(nodes[:count], amps[:count], va[:count], end)
        phases = "AB" if cls == "triplex_line" else "ABC"
        # Boundary currents can be observed across a series equivalent, but its
        # internal voltage and power may differ after source reimport. Physical
        # port powers use actual endpoint voltages, never internal-source power.
        for end, endpoint in enumerate(("from", "to")):
            bus = root(obj[endpoint])
            if e.Circuit.SetActiveBus(bus) < 0:
                raise ValueError(f"missing physical endpoint {bus}")
            volts = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
            triplex = objects[bus]["class"].startswith("triplex")
            for index, phase in enumerate(phases):
                node = {"A": 1, "B": 2, "C": 0}[phase] if triplex else index + 1
                if node != 0 and node not in volts and currents[end, index] != 0:
                    raise ValueError("nonzero boundary current has no endpoint voltage")
                powers[end, index] = (
                    volts.get(node, 0j) * currents[end, index].conjugate()
                )
        for end, label in enumerate(("in", "out")):
            for k, phase in enumerate(phases):
                add(
                    result,
                    f"link.{name}.{label}.{phase}.current",
                    "A",
                    currents[end, k],
                )
                add(result, f"link.{name}.{label}.{phase}.power", "VA", powers[end, k])
        add(result, f"link.{name}.loss", "VA", sum(powers[:, : len(phases)].flatten()))
    return result, {
        "converged": True,
        "iterations": e.Solution.Iterations(),
        "control_mode": e.Solution.ControlMode(),
        "missing_buses": missing_buses,
        "current_form_lines": current_forms,
        "omitted_inactive_lines": omitted_inactive,
        "raw_equivalent_ports": raw_equivalent_ports,
        "power_definition": "physical endpoint voltage times conjugate boundary current",
    }


def telemetry_audit(network, snapshot, objects):
    """Check the pinned native NR reporting equation independently of DSS.

    line.cpp uses d=a=I+ZY/2; physical pi ports require d=I+YZ/2.
    link.cpp reports Iin=c*Vto+d*Iout, so the difference is
    (ZY-YZ)*Iout/2. This audit explains telemetry; it never replaces raw data
    or makes a failed whole-feeder comparison pass.
    """
    codes = {c["name"]: c for c in network["linecodes"]}
    result = []
    for line in network["lines"]:
        name = line["name"]
        if objects.get(name, {}).get("class") not in (
            "overhead_line",
            "underground_line",
        ):
            continue
        code = codes[line["linecode"]]
        length = line["length"]
        z = (np.array(code["r_series"]) + 1j * np.array(code["x_series"])) * length
        y = 2 * (np.array(code["g_from"]) + 1j * np.array(code["b_from"])) * length
        if not np.any(z @ y - y @ z):
            continue
        phases = ["ABC"[int(t) - 1] for t in line["terminal_map_from"]]
        volts = [
            np.array(
                [complex(*snapshot[f"bus.{line[end]}.{p}"]["value"]) for p in phases]
            )
            for end in ("bus_from", "bus_to")
        ]
        vf, vt = volts
        series = np.linalg.solve(z, vf - vt)
        physical = series + y @ vf / 2
        outward = series - y @ vt / 2
        delta = (z @ y - y @ z) @ outward / 2
        recorded = np.array(
            [complex(*snapshot[f"link.{name}.in.{p}.current"]["value"]) for p in phases]
        )
        result.append(
            {
                "name": name,
                "physical_difference_A": float(max(abs(recorded - physical))),
                "predicted_reporting_difference_A": float(max(abs(delta))),
                "reporting_equation_residual_A": float(
                    max(abs(recorded - physical - delta))
                ),
            }
        )
    return sorted(result, key=lambda item: item["physical_difference_A"], reverse=True)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("source", "bridge", "output", "pnnl-root"):
        p.add_argument("--" + name, type=Path, required=True)
    p.add_argument("--native-stopping-fraction", type=float, default=1e-8)
    p.add_argument("--native-iteration-limit", type=int, default=100)
    p.add_argument("--execution-timeout", type=float, default=120)
    p.add_argument("--roundtrip-bridge", type=Path)
    args = p.parse_args()
    if (
        not math.isfinite(args.native_stopping_fraction)
        or args.native_stopping_fraction <= 0
        or not 1 <= args.native_iteration_limit <= 10000
    ):
        p.error(
            "native stopping fraction must be finite/positive and iteration limit in 1..10000"
        )
    if not math.isfinite(args.execution_timeout) or args.execution_timeout <= 0:
        p.error("execution timeout must be finite and positive")
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": "GLM import/IR to static DSS; optional repeated DSS import/IR/output. Canonical GLM is not qualified by this report.",
        "validation_profile": "feeder-1ppm-v1",
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "native_stopping_fraction": args.native_stopping_fraction,
        "native_iteration_limit": args.native_iteration_limit,
        "execution_timeout_seconds": args.execution_timeout,
        "roundtrip_bridge_sha256": hashlib.sha256(
            args.roundtrip_bridge.read_bytes()
        ).hexdigest()
        if args.roundtrip_bridge
        else None,
        "validation_profiles": {"calibration": TOLERANCES, "feeder": FEEDER_TOLERANCES},
        "cases": {},
    }
    for case in ("bal", "unbal"):
        name = "ieee9500" + case
        src = args.source / (name + "_glm") / "glm"
        directory = args.output / case
        directory.mkdir()
        hashes = {}
        for suffix in ("base", "run", "edits2"):
            path = src / f"{name}_{suffix}.glm"
            data = path.read_bytes()
            hashes[suffix] = hashlib.sha256(data).hexdigest()
            if hashes[suffix] != SOURCE_HASHES[case][suffix]:
                raise ValueError("IEEE9500 base pin mismatch")
            (directory / path.name).write_bytes(data)
        source = directory / f"{name}_run.glm"
        proc = subprocess.run(
            [
                str(args.bridge),
                str(source),
                str(directory / "network.pio.json"),
                "pnnl",
                "--dss-output",
                str(directory / "dss"),
            ],
            capture_output=True,
            text=True,
            timeout=120,
            check=False,
        )
        (directory / "bridge.json").write_text(proc.stdout)
        (directory / "bridge.stderr").write_text(proc.stderr)
        proc.check_returncode()
        bridge = json.loads(proc.stdout)
        if not bridge["ir_network_equal"]:
            raise ValueError("IR changed circuit")
        objects = inventory(
            source.read_text() + (directory / f"{name}_base.glm").read_text()
        )
        native_text, spec = instrument(
            source.read_text(),
            objects,
            args.native_stopping_fraction,
            args.native_iteration_limit,
        )
        (directory / "model.glm").write_text(native_text)
        run_glm(
            "pnnl",
            directory,
            args.pnnl_root,
            execution_timeout=args.execution_timeout,
            outputs=tuple(
                f"{cls}-{prop}.csv" for cls, props in spec.items() for prop in props
            ),
        )
        expected, reported = native_snapshot(grouped(directory, spec), objects)
        actual, solve = imported_snapshot(
            directory / "dss/case.dss", bridge["network"], objects
        )
        calibration = compare(expected, actual)
        check = compare(expected, actual, tolerances=FEEDER_TOLERANCES)
        balance = current_balance(actual, bridge["network"], objects, LINKS)
        changed = copy.deepcopy(bridge["network"])
        changed = changed.get("value", changed)
        changed = changed.get("network", changed)
        for load in changed["loads"]:
            load["p_nom"] = [v * 1.1 for v in load["p_nom"]]
            load["q_nom"] = [v * 1.1 for v in load["q_nom"]]
        changed_load = current_balance(actual, changed, objects, LINKS)

        wrong, _ = imported_snapshot(
            directory / "dss/case.dss", bridge["network"], objects, wrong_tap=True
        )
        negative = compare(expected, wrong, tolerances=FEEDER_TOLERANCES)
        repeated = None
        if args.roundtrip_bridge:
            destination = directory / "reimported"
            repeat = json.loads(
                command(
                    [
                        str(args.roundtrip_bridge.resolve()),
                        str((directory / "dss/case.dss").resolve()),
                        str(destination.resolve()),
                    ],
                    directory,
                )
            )
            if not repeat["ir_network_equal"] or not repeat["targets"]["dss"]["ok"]:
                raise ValueError("DSS reimport/IR/write failed")
            repeat_net = repeat["network"]
            repeat_net = repeat_net.get("value", repeat_net)
            repeat_net = repeat_net.get("network", repeat_net)
            reimported, repeat_solve = imported_snapshot(
                destination / "dss/case.dss", repeat_net, objects
            )
            repeated = {
                "ir_equal": True,
                "comparison": compare(
                    expected, reimported, tolerances=FEEDER_TOLERANCES
                ),
                "calibration_comparison": compare(expected, reimported),
                "current_balance": current_balance(
                    reimported, bridge["network"], objects, LINKS
                ),
                "solve": repeat_solve,
            }
            (directory / "reimported.json").write_text(json.dumps(reimported))
        item = {
            "source_sha256": hashes,
            "inventory": {
                cls: sum(o["class"] == cls for o in objects.values())
                for cls in NODES + LINKS
            },
            "ir_equal": True,
            "solve": solve,
            "comparison": check,
            "calibration_comparison": calibration,
            "current_balance": balance,
            "changed_load_negative": {
                "passed": not changed_load["passed"],
                "mismatch": changed_load,
            },
            "reimported": repeated,
            "native_reporting_audit": telemetry_audit(
                bridge["network"], expected, objects
            ),
            "voltage_comparison": compare(
                {k: v for k, v in expected.items() if v["unit"] == "V"},
                {k: v for k, v in actual.items() if v["unit"] == "V"},
            ),
            "wrong_tap_negative": {
                "passed": not negative["passed"],
                "mismatch": negative,
            },
            "passed": check["passed"]
            and balance["passed"]
            and not negative["passed"]
            and not changed_load["passed"]
            and (
                repeated is None
                or (
                    repeated["comparison"]["passed"]
                    and repeated["current_balance"]["passed"]
                )
            ),
        }
        for filename, value in (
            ("native.json", expected),
            ("imported.json", actual),
            ("reported-loss.json", reported),
        ):
            (directory / filename).write_text(json.dumps(value))
        report["cases"][case] = item
        report["passed"] = all(v["passed"] for v in report["cases"].values())
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(
            case,
            check["passed"],
            check["max_absolute_error"],
            check["failures"][:8],
            flush=True,
        )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
