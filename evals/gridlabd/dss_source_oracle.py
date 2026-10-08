"""Qualify exact DSS source circuits and separately audit typed reimport.

A finite source R followed by -R is algebraically ideal at the external ports:
internal-node KCL is (Vx - E - Vn)/R - (Vx - Vp)/R = 0, hence Vp - Vn = E.
The native circuit uses no small impedance. This harness is not an exporter.
Typed reimport failures remain failures even when the native equivalent passes.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK, add, command, compare


PATTERNS = {
    "unbalanced": np.array(
        [230 * np.exp(0.1j), 241 * np.exp(-2j), 225 * np.exp(2.15j)]
    ),
    "phase_pair": 240 * np.exp(1j * np.array([-2 * np.pi / 3, 2 * np.pi / 3])),
    "split": np.array([120, -120], dtype=complex),
    "zero": np.array([0, 240j], dtype=complex),
}
LAWS = {"P": (1,), "I": (5,), "Z": (2,), "PIZ": (1, 5, 2)}


def parameters(pattern):
    emf = PATTERNS[pattern].copy()
    nominal = np.where(abs(emf) > 0, abs(emf), 240)
    power = np.array([300 + 80j, 400 - 20j, 500 + 100j])[: len(emf)]
    impedance = np.array([0.3 + 0.6j, 0.4 + 0.7j, 0.5 + 0.8j])[: len(emf)]
    return emf, nominal, power, impedance


def load_current(voltage, nominal, power, law):
    current = np.zeros(len(voltage), dtype=complex)
    for model in LAWS[law]:
        if model == 2:
            current += power.conjugate() * voltage / nominal**2
        else:
            if np.any(abs(voltage) < 1):
                raise ValueError("nonlinear reference outside qualified voltage domain")
            current += (power / voltage).conjugate() * (
                abs(voltage) / nominal if model == 5 else 1
            )
    return current


def reference(pattern, law, floating):
    emf, nominal, power, z = parameters(pattern)
    rg = 3.0 if floating else 0.0
    # Solve the physical circuit without any compensating source resistance.
    sink = emf.copy()
    for iteration in range(1000):
        current = load_current(sink, nominal, power, law)
        vn = -rg * sum(current)
        nxt = emf + vn - z * current
        if max(abs(nxt - sink)) < 1e-11:
            sink = nxt
            break
        sink = nxt
    else:
        raise ValueError("independent physical reference did not converge")
    current = load_current(sink, nominal, power, law)
    vn = -rg * sum(current)
    source = emf + vn
    residual = float(max(abs((source - sink) / z - current)))
    if residual > 1e-9:
        raise ValueError(f"independent physical KCL residual {residual}")
    return physical_snapshot(source, sink, vn, current), {
        "iterations": iteration + 1,
        "kcl_residual_A": residual,
    }


def physical_snapshot(source, sink, vn, current):
    result = {}
    add(result, "reference.voltage", "V", vn)
    for i in range(len(source)):
        add(result, f"source.{i}.voltage", "V", source[i])
        add(result, f"sink.{i}.voltage", "V", sink[i])
        add(result, f"line.{i}.current", "A", current[i])
        add(result, f"line.{i}.from_power", "VA", source[i] * current[i].conjugate())
        add(result, f"line.{i}.to_power", "VA", -sink[i] * current[i].conjugate())
        add(result, f"source.{i}.current", "A", -current[i])
        add(result, f"source.{i}.return_current", "A", current[i])
    return result


def deck(pattern, law, floating, frequency, resistance, compensation=1.0):
    emf, nominal, power, z = parameters(pattern)
    lines = ["clear", f"set defaultbasefrequency={frequency}"]
    for i, e in enumerate(emf):
        head = "Circuit.test" if i == 0 else f"Vsource.p{i}"
        kv = max(abs(e) / 1000, 0.001)
        pu = 0 if e == 0 else 1
        lines.append(
            f"New {head} phases=1 bus1=internal{i}.1 "
            f"bus2=source.{4 if floating else 0} basekv={kv:.17g} pu={pu} "
            f"angle={np.angle(e, deg=True):.17g} frequency={frequency} "
            f"r1={resistance} x1=0 r0={resistance} x0=0"
        )
        for name, start, end, value in [
            (
                f"comp{i}",
                f"internal{i}.1",
                f"source.{i + 1}",
                -resistance * compensation,
            ),
            (f"line{i}", f"source.{i + 1}", f"sink.{i + 1}", z[i]),
        ]:
            lines.append(
                f"New Line.{name} phases=1 bus1={start} bus2={end} "
                f"r1={value.real:.17g} x1={value.imag:.17g} "
                f"r0={value.real:.17g} x0={value.imag:.17g} "
                "c1=0 c0=0 length=1 units=none"
            )
        for model in LAWS[law]:
            lines.append(
                f"New Load.l{i}_{model} phases=1 bus1=sink.{i + 1}.0 "
                f"conn=wye model={model} kv={nominal[i] / 1000:.17g} "
                f"kw={power[i].real / 1000:.17g} kvar={power[i].imag / 1000:.17g} "
                "vminpu=0 vmaxpu=2"
            )
    if floating:
        lines.append(
            "New Line.ground phases=1 bus1=source.4 bus2=source.0 "
            "r1=3 x1=0 r0=3 x0=0 c1=0 c0=0 length=1 units=none"
        )
    lines += ["set controlmode=off tolerance=1e-12 maxiterations=100", "solve"]
    return "\n".join(lines) + "\n"


def complex_array(raw):
    return np.array(raw[::2]) + 1j * np.array(raw[1::2])


def native_snapshot(path, count, floating):
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned OpenDSS engine")
    engine.Text.Command(f'redirect "{path.resolve()}"')
    if not engine.Solution.Converged():
        raise ValueError("native circuit did not converge")
    voltage = {}
    for bus in ("source", "sink"):
        engine.Circuit.SetActiveBus(bus)
        voltage[bus] = dict(
            zip(engine.Bus.Nodes(), complex_array(engine.Bus.Voltages()), strict=True)
        )
    result = {}
    add(result, "reference.voltage", "V", voltage["source"][4] if floating else 0)
    for i in range(count):
        for bus in ("source", "sink"):
            add(result, f"{bus}.{i}.voltage", "V", voltage[bus][i + 1])
        engine.Circuit.SetActiveElement(f"line.line{i}")
        currents = complex_array(engine.CktElement.Currents())
        powers = complex_array(engine.CktElement.Powers()) * 1000
        add(result, f"line.{i}.current", "A", currents[0])
        add(result, f"line.{i}.from_power", "VA", powers[0])
        add(result, f"line.{i}.to_power", "VA", powers[1])
        engine.Circuit.SetActiveElement("vsource.source" if i == 0 else f"vsource.p{i}")
        currents = complex_array(engine.CktElement.Currents())
        add(result, f"source.{i}.current", "A", currents[0])
        add(result, f"source.{i}.return_current", "A", currents[1])
    return result, version


def typed_linear_solution(document):
    """Independent MNA of the actual typed Z-load microcase, excluding extras.

    This is a bounded oracle for this generator, not a general PowerIO solver.
    VoltageSource is ideal by its public contract. Electrical data in extras
    cannot supply a missing typed impedance or a missing reference constraint.
    """
    value = document["value"]
    assert value["type"] == "powerio.MulticonductorNetwork"
    net = value["data"]
    assert not any(net[k] for k in ("switches", "transformers", "generators", "shunts"))
    # Only explicitly inactive source records may remain outside the typed
    # circuit in these microcases. Their properties never supply physics.
    assert all(
        obj["class"] == "vsource" and obj["props"][-1] == ["enabled", "no"]
        for obj in net["untyped"]
    )
    nodes = [
        (b["id"], t)
        for b in net["buses"]
        for t in b["terminals"]
        if t not in b["grounded"]
    ]
    indexes = {key: i for i, key in enumerate(nodes)}
    declared = {(b["id"], t) for b in net["buses"] for t in b["terminals"]}
    size = len(nodes)
    y = np.zeros((size, size), dtype=complex)

    def incidence(start, end=None):
        a = np.zeros(size)
        for key, sign in [(start, 1), (end, -1)]:
            if key is not None and key not in declared:
                raise ValueError(f"undeclared typed terminal {key}")
            if key in indexes:
                a[indexes[key]] += sign
        return a

    codes = {c["name"]: c for c in net["linecodes"]}
    for line in net["lines"]:
        code = codes[line["linecode"]]
        assert all(
            np.count_nonzero(code[k]) == 0 for k in ("g_from", "g_to", "b_from", "b_to")
        )
        z = (np.array(code["r_series"]) + 1j * np.array(code["x_series"])) * line[
            "length"
        ]
        a = np.array(
            [
                incidence((line["bus_from"], start), (line["bus_to"], end))
                for start, end in zip(
                    line["terminal_map_from"], line["terminal_map_to"], strict=True
                )
            ]
        )
        assert a.shape[0] == code["n_conductors"]
        y += a.T @ np.linalg.solve(z, a)
    for load in net["loads"]:
        assert load["voltage_model"]["model"] == "constant_impedance"
        assert len(load["terminal_map"]) == 2 and len(load["p_nom"]) == 1
        a = incidence(*[(load["bus"], t) for t in load["terminal_map"]])
        s = complex(load["p_nom"][0], load["q_nom"][0])
        y += np.outer(a, a) * s.conjugate() / load["voltage_model"]["v_nom"][0] ** 2
    constraints, emfs = [], []
    for wire in net["sources"]:
        source = wire
        if "type" in wire:
            assert wire["type"] == "powerio.ReferencedVoltageSource"
            source = wire["value"]
        reference = source.get("reference_terminal")
        for t, mag, angle in zip(
            source["terminal_map"],
            source["v_magnitude"],
            source["v_angle"],
            strict=True,
        ):
            a = incidence(
                (source["bus"], t),
                (source["bus"], reference) if reference is not None else None,
            )
            if not np.any(a):
                assert mag == 0
                continue
            constraints.append(a)
            emfs.append(mag * np.exp(1j * angle))
    c = np.array(constraints, dtype=complex).reshape((len(constraints), size))
    matrix = np.block([[y, c.T], [c, np.zeros((len(c), len(c)))]])
    rhs = np.r_[np.zeros(size), emfs]
    solved = np.linalg.solve(matrix, rhs)
    residual = float(np.max(abs(matrix @ solved - rhs), initial=0))
    if residual > 1e-8:
        raise ValueError(f"typed MNA residual {residual}")
    volts = {key: solved[indexes[key]] if key in indexes else 0j for key in declared}
    currents = {}
    for line in net["lines"]:
        code = codes[line["linecode"]]
        z = (np.array(code["r_series"]) + 1j * np.array(code["x_series"])) * line[
            "length"
        ]
        drop = np.array(
            [
                volts[(line["bus_from"], start)] - volts[(line["bus_to"], end)]
                for start, end in zip(
                    line["terminal_map_from"], line["terminal_map_to"], strict=True
                )
            ]
        )
        currents[line["name"]] = np.linalg.solve(z, drop)
    return volts, currents, residual


def typed_linear_snapshot(document, count, floating):
    volts, _, residual = typed_linear_solution(document)
    result = {}
    add(
        result,
        "reference.voltage",
        "V",
        volts.get(("source", "4"), 0) if floating else 0,
    )
    for i in range(count):
        for bus in ("source", "sink"):
            add(result, f"{bus}.{i}.voltage", "V", volts[(bus, str(i + 1))])
    return result, residual


def materialize_probe_sources(document):
    """Hand-lower this generator's R-only sources into existing typed records.

    This is an attribution probe, not the production DSS reader: it accepts
    only the exact generated scalar source settings and bus2 spellings.
    Original IR and its failing physical comparisons remain untouched.
    """
    document = copy.deepcopy(document)
    net = document["value"]["data"]
    buses = {b["id"]: b for b in net["buses"]}
    for i, source in enumerate(net["sources"]):
        extra = source["extras"]
        r = float(extra["r1"])
        assert r > 0 and float(extra["r0"]) == r
        assert float(extra["x1"]) == float(extra["x0"]) == 0
        assert extra["bus2"] in ("source.0", "source.4")
        assert source["terminal_map"] == ["1", "4"]
        assert source["v_magnitude"][1] == 0
        # Put the ideal emf on a new terminal of the reference bus, then
        # connect its finite source impedance to the actual source terminal.
        terminal = f"oracle_emf_{i}"
        assert terminal not in buses["source"]["terminals"]
        buses["source"]["terminals"].append(terminal)
        code = copy.deepcopy(net["linecodes"][0])
        name = f"oracle_source_impedance_{i}"
        assert name not in {c["name"] for c in net["linecodes"]}
        code.update(name=name, r_series=[[r]], x_series=[[0]], extras={})
        net["linecodes"].append(code)
        net["lines"].append(
            {
                "name": name,
                "bus_from": "source",
                "bus_to": source["bus"],
                "terminal_map_from": [terminal],
                "terminal_map_to": ["1"],
                "linecode": name,
                "length": 1.0,
                "extras": {},
            }
        )
        source.update(
            bus="source",
            terminal_map=[terminal],
            v_magnitude=source["v_magnitude"][:1],
            v_angle=source["v_angle"][:1],
            extras={},
        )
        if extra["bus2"] == "source.4":
            source["reference_terminal"] = "4"
            net["sources"][i] = {
                "type": "powerio.ReferencedVoltageSource",
                "value": source,
            }
    return document


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "native": {},
        "typed_reimport": {},
        "materialization_probe": {},
    }
    for pattern in PATTERNS:
        for law in ("Z",) if pattern == "zero" else LAWS:
            for floating in (False, True):
                expected, convergence = reference(pattern, law, floating)
                for frequency in (50, 60):
                    for resistance in (1.0, 10.0, 100.0):
                        key = f"{pattern}/{law}/floating_{floating}/{frequency}/{resistance}"
                        directory = args.output / key
                        directory.mkdir(parents=True)
                        result = {"passed": False, "reference_convergence": convergence}
                        source = directory / "case.dss"
                        source.write_text(
                            deck(pattern, law, floating, frequency, resistance)
                        )
                        result["input_sha256"] = hashlib.sha256(
                            source.read_bytes()
                        ).hexdigest()
                        try:
                            actual, version = native_snapshot(
                                source, len(PATTERNS[pattern]), floating
                            )
                            check = compare(expected, actual)
                            result.update(
                                comparison=check, version=version, observed=actual
                            )
                            negative = directory / "negative.dss"
                            negative.write_text(
                                # Leave 1% uncompensated. A 10% residue at
                                # 100 ohms can exceed the split-phase P-load
                                # transfer limit; nonconvergence alone is not
                                # a measured negative-control comparison.
                                deck(
                                    pattern, law, floating, frequency, resistance, 0.99
                                )
                            )
                            wrong, _ = native_snapshot(
                                negative, len(PATTERNS[pattern]), floating
                            )
                            control = compare(expected, wrong)
                            result.update(
                                comparison=check,
                                version=version,
                                observed=actual,
                                negative_control=control,
                                negative_sha256=hashlib.sha256(
                                    negative.read_bytes()
                                ).hexdigest(),
                                passed=check["passed"] and not control["passed"],
                            )
                        except Exception as error:
                            result["error"] = str(error)
                        report["native"][key] = result
                        if law == "Z":
                            typed = {"passed": False}
                            report["materialization_probe"][key] = {"passed": False}
                            try:
                                target = directory / "network.pio.json"
                                typed["serialization"] = command(
                                    [
                                        str(args.powerio.resolve()),
                                        "serialize",
                                        str(source.resolve()),
                                        "-o",
                                        str(target.resolve()),
                                    ],
                                    directory,
                                )
                                actual, residual = typed_linear_snapshot(
                                    json.loads(target.read_text()),
                                    len(PATTERNS[pattern]),
                                    floating,
                                )
                                voltage = {
                                    k: v
                                    for k, v in expected.items()
                                    if k.endswith(".voltage")
                                }
                                check = compare(voltage, actual)
                                typed.update(
                                    comparison=check,
                                    observed=actual,
                                    kcl_residual=residual,
                                    passed=check["passed"],
                                    ir_sha256=hashlib.sha256(
                                        target.read_bytes()
                                    ).hexdigest(),
                                )
                                lowered = materialize_probe_sources(
                                    json.loads(target.read_text())
                                )
                                repaired, residual = typed_linear_snapshot(
                                    lowered, len(PATTERNS[pattern]), floating
                                )
                                repaired_check = compare(voltage, repaired)
                                materialized_path = (
                                    directory / "hand_materialized.pio.json"
                                )
                                materialized_path.write_text(
                                    json.dumps(lowered, indent=2) + "\n"
                                )
                                # summary deserializes IR through the facade;
                                # serialize intentionally refuses IR input.
                                decoded = json.loads(
                                    command(
                                        [
                                            str(args.powerio.resolve()),
                                            "summary",
                                            str(materialized_path.resolve()),
                                        ],
                                        directory,
                                    )
                                )
                                assert decoded["model"] == "multiconductor"
                                report["materialization_probe"][key] = {
                                    "passed": repaired_check["passed"],
                                    "comparison": repaired_check,
                                    "kcl_residual": residual,
                                    "facade_deserialization": decoded,
                                    "ir_sha256": hashlib.sha256(
                                        materialized_path.read_bytes()
                                    ).hexdigest(),
                                }
                            except Exception as error:
                                typed["error"] = str(error)
                            report["typed_reimport"][key] = typed
                        (args.output / "report.json").write_text(
                            json.dumps(report, indent=2) + "\n"
                        )
    for group in ("native", "typed_reimport", "materialization_probe"):
        print(
            group,
            sum(v["passed"] for v in report[group].values()),
            "passed of",
            len(report[group]),
        )
    return (
        0
        if all(
            v["passed"]
            for group in ("native", "typed_reimport", "materialization_probe")
            for v in report[group].values()
        )
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
