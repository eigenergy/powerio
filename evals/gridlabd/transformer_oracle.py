"""Independent single-phase and center-tapped transformer calibration.

The reference solves an ideal winding/core circuit with explicit series
impedances. It does not copy the GridLAB-D transformer matrix formula.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, literal, recorded, run_glm

RATIO = 2.0
PRIMARY_Z = (0.01 + 0.02j) * 240**2 / 10000
SECONDARY_Z = np.array([0.015 + 0.025j, 0.025 + 0.035j]) * 120**2 / 10000
LOAD_Z = np.array([10 + 3j, 20 + 6j])
PAIR_Z = 30 + 5j


def snapshot(vsource, voltage, iprimary, currents, loss, split):
    result = {}
    add(result, "source.A.voltage", "V", vsource)
    add(result, "transformer.from.A.current", "A", iprimary)
    add(result, "transformer.from.A.power", "VA", vsource * iprimary.conjugate())
    for label, v, i in zip(
        ("L1", "L2", "N") if split else ("A",), voltage, currents, strict=True
    ):
        add(result, f"sink.{label}.voltage", "V", v)
        add(result, f"transformer.to.{label}.current", "A", -i)
        add(result, f"transformer.to.{label}.power", "VA", -v * i.conjugate())
    if split:
        add(result, "sink.L1L2.voltage", "V", voltage[0] - voltage[1])
    add(result, "transformer.loss", "VA", loss)
    return result


def analytic(source: complex, split: bool, core_y: complex = 0j) -> dict:
    if not split:
        z = (0.02 + 0.04j) * 120**2 / 10000
        current = source / RATIO / (LOAD_Z[0] + z)
        voltage = current * LOAD_Z[0]
        loss = abs(current) ** 2 * z
        return snapshot(source, [voltage], current / RATIO, [current], loss, False)
    # Unknowns are the primary-referred core potential U and the two physical
    # hot-conductor voltages. The second ideal winding has opposite polarity.
    # Stamp each secondary leakage through its coil incidence, then add the
    # primary leakage, core shunt, leg loads and line-line load independently.
    y = np.zeros((3, 3), dtype=complex)
    for k, sign in enumerate((1, -1)):
        incidence = np.zeros(3)
        incidence[0], incidence[k + 1] = sign / RATIO, -1
        y += np.outer(incidence, incidence) / SECONDARY_Z[k]
        y[k + 1, k + 1] += 1 / LOAD_Z[k]
    pair = np.array([0, 1, -1])
    y += np.outer(pair, pair) / PAIR_Z
    y[0, 0] += 1 / PRIMARY_Z + core_y
    u, v1, v2 = np.linalg.solve(y, np.array([source / PRIMARY_Z, 0, 0]))
    i1, i2 = (u / RATIO - v1) / SECONDARY_Z[0], (-u / RATIO - v2) / SECONDARY_Z[1]
    ip = (source - u) / PRIMARY_Z
    loss = (
        abs(ip) ** 2 * PRIMARY_Z
        + abs(i1) ** 2 * SECONDARY_Z[0]
        + abs(i2) ** 2 * SECONDARY_Z[1]
    )
    loss += abs(u) ** 2 * core_y.conjugate()
    return snapshot(source, [v1, v2, 0j], ip, [i1, i2, -i1 - i2], loss, True)


def primitive(split: bool) -> np.ndarray:
    """Coil-incidence reference with internal ideal-core potential eliminated."""
    if split:
        # Variables [core U, primary terminal voltage, leg1, leg2].
        y = np.zeros((4, 4), dtype=complex)
        for incidence, z in (
            (np.array([-1, 1, 0, 0]), PRIMARY_Z),
            (np.array([1 / RATIO, 0, -1, 0]), SECONDARY_Z[0]),
            (np.array([-1 / RATIO, 0, 0, -1]), SECONDARY_Z[1]),
        ):
            y += np.outer(incidence, incidence) / z
        reduced = y[1:, 1:] - np.outer(y[1:, 0], y[0, 1:]) / y[0, 0]
        terminal_map = np.array(
            [[1, -1, 0, 0, 0, 0], [0, 0, 1, -1, 0, 0], [0, 0, 0, 0, -1, 1]]
        )
    else:
        turns = np.array([1 / RATIO, -1])
        reduced = np.outer(turns, turns) / ((0.02 + 0.04j) * 120**2 / 10000)
        terminal_map = np.array([[1, -1, 0, 0], [0, 0, 1, -1]])
    return terminal_map.T @ reduced @ terminal_map


def add_primitive(result: dict, matrix: np.ndarray) -> None:
    for i in range(len(matrix)):
        for j in range(len(matrix)):
            add(result, f"transformer.y.{i}.{j}", "S", matrix[i, j])


def glm(split: bool) -> str:
    lines = [
        "// Original synthetic PowerIO transformer oracle; fixed electrical state.",
        "#set complex_format=%.15g%+.15gj",
        "#set complex_output_format=RECT",
        "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; stoptime '2000-01-01 00:00:02'; }",
        "module powerflow { solver_method NR; }",
        "module tape;",
        "object node { name source; phases AN; bustype SWING; nominal_voltage 240; voltage_A 240+0j; }",
    ]
    if split:
        lines += [
            "object triplex_load { name sink; phases AS; nominal_voltage 120; maximum_voltage_error 1e-10;",
            f"constant_impedance_1 {literal(LOAD_Z[0])}; constant_impedance_2 {literal(LOAD_Z[1])};",
            f"constant_impedance_12 {literal(PAIR_Z)}; }}",
            "object transformer_configuration { name cfg; connect_type SINGLE_PHASE_CENTER_TAPPED;",
            "install_type PADMOUNT; primary_voltage 240; secondary_voltage 120; power_rating 10; powerA_rating 10;",
            "resistance 0.01; reactance 0.02; resistance1 0.015; reactance1 0.025; resistance2 0.025; reactance2 0.035;",
            # Include finite core admittance in the analytic GLD reference.
            # DSS's zero-core-loss reference is independently evaluated.
            "shunt_resistance 1000000000; shunt_reactance 1000000000; }",
            "object transformer { name line; phases AS; from source; to sink; configuration cfg; }",
        ]
        sink_props = "voltage_1,voltage_2,voltage_N,voltage_12"
        ports = "ABC"
    else:
        lines += [
            "object load { name sink; phases AN; nominal_voltage 120; maximum_voltage_error 1e-10;",
            f"constant_impedance_A {literal(LOAD_Z[0])}; }}",
            "object transformer_configuration { name cfg; connect_type SINGLE_PHASE; install_type PADMOUNT;",
            "primary_voltage 240; secondary_voltage 120; power_rating 10; resistance 0.02; reactance 0.04; }",
            "object transformer { name line; phases AN; from source; to sink; configuration cfg; }",
        ]
        sink_props, ports = "voltage_A", "A"
    lines += [
        'object impedance_dump { filename "impedance.xml"; }',
        'object recorder { parent source; property voltage_A; interval 1; file "source.csv"; }',
        f'object recorder {{ parent sink; property {sink_props}; interval 1; file "sink.csv"; }}',
    ]
    props = ["current_in_A", "power_in_A", "power_losses"]
    props += [f"{q}_out_{p}" for p in ports for q in ("current", "power")]
    lines.append(
        f'object recorder {{ parent line; property {",".join(props)}; interval 1; file "line.csv"; }}'
    )
    return "\n".join(lines) + "\n"


def dss_deck(split: bool) -> str:
    lines = [
        "clear",
        "new circuit.oracle phases=1 bus1=source.1.0 basekv=.24 pu=1 "
        + "r1=0.000001 x1=0.000001 r0=0.000001 x0=0.000001",
    ]
    if split:
        lines += [
            "new transformer.x phases=1 windings=3 buses=[source.1.0 sink.1.0 sink.0.2] "
            "conns=[wye wye wye] kvs=[.24 .12 .12] kvas=[10 10 10] "
            "%rs=[1 1.5 2.5] xhl=4.5 xht=5.5 xlt=6 %noloadloss=0 %imag=0 ppm_antifloat=0"
        ]
        loads = [
            ("sink.1.0", 120, LOAD_Z[0]),
            ("sink.2.0", 120, LOAD_Z[1]),
            ("sink.1.2", 240, PAIR_Z),
        ]
    else:
        lines += [
            "new transformer.x phases=1 windings=2 buses=[source.1.0 sink.1.0] "
            "conns=[wye wye] kvs=[.24 .12] kvas=[10 10] %rs=[1 1] xhl=4 "
            "%noloadloss=0 %imag=0 ppm_antifloat=0"
        ]
        loads = [("sink.1.0", 120, LOAD_Z[0])]
    for k, (bus, voltage, impedance) in enumerate(loads):
        power = voltage**2 / impedance.conjugate() / 1000
        lines.append(
            f"new load.l{k} phases=1 bus1={bus} conn=wye kv={voltage / 1000} "
            f"kw={power.real:.15g} kvar={power.imag:.15g} model=2 vminpu=0 vmaxpu=2"
        )
    lines += [
        f"edit transformer.x taps=[{' '.join(['1'] * (3 if split else 2))}]",
        # Deliberately asks for a different voltage, but must not run.
        "new regcontrol.frozen transformer=x winding=2 ptratio=1 vreg=150 band=1",
        "set controlmode=off",
        "set tolerance=1e-12 maxiterations=100",
    ]
    return "\n".join(lines) + "\n"


def read_glm(directory: Path, split: bool) -> dict:
    source = recorded(directory / "source.csv")["voltage_A"]
    sink, link = recorded(directory / "sink.csv"), recorded(directory / "line.csv")
    if split:
        volts = [sink["voltage_1"], -sink["voltage_2"], sink["voltage_N"]]
        currents = [link[f"current_out_{p}"] for p in "ABC"]
    else:
        volts, currents = [sink["voltage_A"]], [link["current_out_A"]]
    result = snapshot(
        source, volts, link["current_in_A"], currents, link["power_losses"], split
    )
    # Compare actual reported powers and line-line voltage, not recomputed
    # products that could conceal an adapter polarity mistake.
    result["transformer.from.A.power"]["value"] = [
        link["power_in_A"].real,
        link["power_in_A"].imag,
    ]
    for label, phase in zip(
        ("L1", "L2", "N") if split else ("A",), "ABC" if split else "A", strict=True
    ):
        power = -link[f"power_out_{phase}"]
        result[f"transformer.to.{label}.power"]["value"] = [power.real, power.imag]
    if split:
        result["sink.L1L2.voltage"]["value"] = [
            sink["voltage_12"].real,
            sink["voltage_12"].imag,
        ]
    return result


def read_dss(path: Path, split: bool) -> tuple[dict, dict]:
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    engine.Text.Command(f'redirect "{path}"')

    def taps():
        engine.Transformers.Name("x")
        result = []
        for winding in range(1, (3 if split else 2) + 1):
            engine.Transformers.Wdg(winding)
            result.append(engine.Transformers.Tap())
        return result

    before = taps()
    if before != [1.0] * (3 if split else 2) or engine.Solution.ControlMode() != -1:
        raise ValueError(
            "fixture did not declare the expected frozen taps/control mode"
        )
    engine.Solution.Solve()
    after = taps()
    if before != after:
        raise ValueError("disabled controller changed the declared physical state")
    if not engine.Solution.Converged():
        raise ValueError("OpenDSS transformer case did not converge")
    engine.Circuit.SetActiveElement("transformer.x")

    def unpack(values):
        return np.array(values[::2]) + 1j * np.array(values[1::2])

    voltage, currents = (
        unpack(engine.CktElement.Voltages()),
        unpack(engine.CktElement.Currents()),
    )
    powers = 1000 * unpack(engine.CktElement.Powers())
    nodes = engine.CktElement.NodeOrder()
    if nodes[:4] != [1, 0, 1, 0] or (split and sorted(nodes[4:]) != [0, 2]):
        raise ValueError("unexpected winding conductor order")
    hot2, neutral2 = (
        (4 + nodes[4:].index(2), 4 + nodes[4:].index(0)) if split else (0, 0)
    )
    if split:
        volts = voltage[[2, hot2, 3]]
        output = -np.array(
            [currents[2], currents[hot2], currents[3] + currents[neutral2]]
        )
    else:
        volts, output = voltage[[2]], -currents[[2]]
    result = snapshot(
        voltage[0],
        volts,
        currents[0],
        output,
        complex(*engine.CktElement.Losses()),
        split,
    )
    result["transformer.from.A.power"]["value"] = [powers[0].real, powers[0].imag]
    observed_powers = (
        [powers[2], powers[hot2], powers[3] + powers[neutral2]]
        if split
        else [powers[2]]
    )
    for label, power in zip(
        ("L1", "L2", "N") if split else ("A",), observed_powers, strict=True
    ):
        result[f"transformer.to.{label}.power"]["value"] = [power.real, power.imag]
    order = [0, 1, 2, 3, neutral2, hot2] if split else [0, 1, 2, 3]
    yp = unpack(engine.CktElement.YPrim()).reshape((len(order), len(order)), order="F")
    add_primitive(result, yp[np.ix_(order, order)])
    return result, {
        "version": engine.Basic.Version(),
        "control_mode": engine.Solution.ControlMode(),
        "state_origin": "source-declared; no control settling",
        "taps_before": before,
        "taps_after": after,
        "primitive_order": [
            "primary.A",
            "primary.N",
            "secondary1.L1" if split else "secondary1.A",
            "secondary1.N",
        ]
        + (["secondary2.N", "secondary2.L2"] if split else []),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    output, root = args.output.resolve(), args.pnnl_root.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": "transformer/coil calibration; no PowerIO conversion",
        "cases": {},
    }
    for split in (False, True):
        name = "center_tapped" if split else "single_phase"
        results = {}
        for profile in ("pnnl", "arras", "dss"):
            directory = output / name / profile
            directory.mkdir(parents=True)
            if profile == "dss":
                path = directory / "model.dss"
                path.write_text(dss_deck(split))
                actual, metadata = read_dss(path, split)
                if LOCK["dss"]["version"] not in metadata["version"]:
                    raise ValueError("DSS engine version mismatch")
                source = complex(*actual["source.A.voltage"]["value"])
                core_y = 0j
            else:
                path = directory / "model.glm"
                path.write_text(glm(split))
                metadata = {"version": run_glm(profile, directory, root)}
                actual = read_glm(directory, split)
                source = 240 + 0j
                core_y = (1 - 1j) / (1e9 * 240**2 / 10000) if split else 0j
            expected = analytic(source, split, core_y)
            if profile == "dss":
                add_primitive(expected, primitive(split))
            result = compare(expected, actual)
            results[profile] = dict(
                result,
                engine=metadata,
                core_admittance_S=[core_y.real, core_y.imag],
                input_sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
            )
            (directory / "snapshot.json").write_text(
                json.dumps(
                    {"expected": expected, "actual": actual}, indent=2, allow_nan=False
                )
                + "\n"
            )
            print(name, profile, result)
            if profile == "dss" and split:
                defective = output / "reversed-secondary.dss"
                defective.write_text(dss_deck(True).replace("sink.0.2", "sink.2.0"))
                changed, _ = read_dss(defective, True)
                defect = compare(expected, changed)
                witness = "sink.L2.voltage"
                report["reversed_secondary_detected"] = not compare(
                    {witness: expected[witness]}, {witness: changed[witness]}
                )["passed"]
                (output / "reversed-secondary.json").write_text(
                    json.dumps(defect, indent=2) + "\n"
                )
        report["cases"][name] = results
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return (
        0
        if report["reversed_secondary_detected"]
        and all(r["passed"] for c in report["cases"].values() for r in c.values())
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
