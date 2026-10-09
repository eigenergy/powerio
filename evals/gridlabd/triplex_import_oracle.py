"""Connected triplex geometry import: physical neutral and both-end port checks.

Uses actual PowerIO import, IR restoration and OpenDSS emission. Includes NR,
FBS, A/B/C primaries, unequal wires, 50 Hz, mixed loads, IEEE8500 service values
and the native zero-mutual-impedance rule for an omitted overhead spacing.
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, add, compare, recorded, run_glm
from split_phase_import_oracle import cases as service_cases
from split_phase_import_oracle import read_native
from transformer_oracle import LOAD_Z, PAIR_Z, PRIMARY_Z, SECONDARY_Z, snapshot

OUTPUTS = (
    "source.csv",
    "sink.csv",
    "line.csv",
    "impedance.xml",
    "load.csv",
    "service.csv",
)


def cases():
    services = service_cases()
    chosen = [services[i] for i in (0, 1, 2, 5)]
    chosen += [
        (
            "frequency-50Hz",
            "A",
            services[0][2].replace(
                "solver_method NR;", "solver_method NR; nominal_frequency 50;"
            ),
            True,
        ),
        ("missing-overhead-spacing", "A", services[0][2], False),
    ]
    for label, phase, original, _ in chosen:
        text = original.replace(
            "to sink; configuration cfg;", "to head; configuration cfg;"
        ).replace(
            "parent sink; property voltage_1,", "parent head; property voltage_1,"
        )
        text += f"\nobject triplex_node {{ name head; phases {phase}S; nominal_voltage 120; maximum_voltage_error 1e-10; }}\n"
        same_wire = label == "ieee8500-service-parameters"
        for i, (r, gmr) in enumerate(
            zip(
                (1.535, 1.535 if same_wire else 1.8, 1.535 if same_wire else 2.0),
                (
                    0.0111,
                    0.0111 if same_wire else 0.008,
                    0.0111 if same_wire else 0.010,
                ),
                strict=True,
            )
        ):
            text += f"object triplex_line_conductor {{ name wire{i}; resistance {r}; geometric_mean_radius {gmr}; rating.summer.continuous 315 A; rating.summer.emergency 315 A; rating.winter.continuous 315 A; rating.winter.emergency 315 A; }}\n"
        text += "object triplex_line_configuration { name cable; conductor_1 wire0; conductor_2 wire1; conductor_N wire2; diameter .368; insulation_thickness .08; }\n"
        text += f"object triplex_line {{ name service; from head; to sink; phases {phase}S; length 100; configuration cable; }}\n"
        text += 'object recorder { parent sink; property voltage_1,voltage_2,voltage_N; interval 1; file "load.csv"; }\n'
        props = [
            f"{q}_{end}_{p}"
            for end in ("in", "out")
            for p in "ABC"
            for q in ("current", "power")
        ] + ["power_losses"]
        text += f'object recorder {{ parent service; property {",".join(props)}; interval 1; file "service.csv"; }}\n'
        if label == "missing-overhead-spacing":
            text = text.replace(
                "from source; to head;", "from primary; to head;"
            ).replace(
                "parent source; property voltage_A;",
                "parent primary; property voltage_A;",
            )
            text += "object node { name primary; phases AN; nominal_voltage 240; maximum_voltage_error 1e-10; }\nobject overhead_line_conductor { name oh; resistance .306; geometric_mean_radius .0244; }\nobject line_spacing { name missing; }\nobject line_configuration { name upstream_cfg; conductor_A oh; conductor_N oh; spacing missing; }\nobject overhead_line { name upstream; from source; to primary; phases AN; length 100; configuration upstream_cfg; }\n"
        yield label, phase, text


def line_observations(result, volts, currents, powers, loss):
    # The shared transformer observer calls its output bus sink. In this
    # connected circuit that bus is the service's head, before the cable.
    for key in list(result):
        if key.startswith("sink."):
            result["secondary." + key[5:]] = result.pop(key)
    for k, label in enumerate(("L1", "L2", "N")):
        add(result, f"load.{label}.voltage", "V", volts[3 + k])
        for end, offset in (("from", 0), ("to", 3)):
            add(result, f"service.{end}.{label}.current", "A", currents[offset + k])
            add(result, f"service.{end}.{label}.power", "VA", powers[offset + k])
    add(result, "service.loss", "VA", loss)


def native_snapshot(directory, phase):
    result = read_native(directory, phase)
    head = recorded(directory / "sink.csv")
    load = recorded(directory / "load.csv")
    service = recorded(directory / "service.csv")
    volts = [
        row[field] * sign
        for row in (head, load)
        for field, sign in (("voltage_1", 1), ("voltage_2", -1), ("voltage_N", 1))
    ]
    currents = [
        direction * service[f"current_{end}_{p}"]
        for end, direction in (("in", 1), ("out", -1))
        for p in "ABC"
    ]
    powers = [
        direction * service[f"power_{end}_{p}"]
        for end, direction in (("in", 1), ("out", -1))
        for p in "ABC"
    ]
    line_observations(result, volts, currents, powers, service["power_losses"])
    return result


def imported_snapshot(path, phase, floating=False, direct=False):
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("OpenDSS version pin mismatch")
    engine.Text.Command(f'redirect "{path}"')
    if floating:
        engine.Text.Command("edit line.service bus2=sink.1.2.4")
    engine.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    engine.Solution.Solve()
    if not engine.Solution.Converged() or engine.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS did not converge with controls off")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    engine.Circuit.SetActiveElement("transformer.line")
    tv, ti, tp = (
        unpack(engine.CktElement.Voltages()),
        unpack(engine.CktElement.Currents()),
        1000 * unpack(engine.CktElement.Powers()),
    )
    if engine.CktElement.NodeOrder() != ["ABC".index(phase) + 1, 0, 1, 0, 0, 2, 1, 0]:
        raise ValueError("unexpected transformer conductor order")
    result = snapshot(
        tv[0],
        [tv[2], tv[5], 0j],
        ti[0],
        [-ti[2], -ti[5], -ti[3] - ti[4]],
        sum(tp[:6]),
        True,
    )
    result["transformer.from.A.power"]["value"] = [tp[0].real, tp[0].imag]
    for label, value in zip(
        ("L1", "L2", "N"), (tp[2], tp[5], tp[3] + tp[4]), strict=True
    ):
        result[f"transformer.to.{label}.power"]["value"] = [value.real, value.imag]
    engine.Circuit.SetActiveElement("line.service")
    if direct:
        if engine.CktElement.NodeOrder() != [1, 2, 1, 2]:
            raise ValueError("unexpected two-conductor correction order")
        lv, li, lp = (
            unpack(engine.CktElement.Voltages()),
            unpack(engine.CktElement.Currents()),
            1000 * unpack(engine.CktElement.Powers()),
        )
        fv, fi, fp = lv[:2], li[:2], lp[:2]
        if "Vsource._pio_coupled_service".lower() in [
            n.lower() for n in engine.Circuit.AllElementNames()
        ]:
            engine.Circuit.SetActiveElement("vsource._pio_coupled_service")
            sv, si, sp = (
                unpack(engine.CktElement.Voltages()),
                unpack(engine.CktElement.Currents()),
                1000 * unpack(engine.CktElement.Powers()),
            )
            if abs(si[2]) > 1e-8 or abs(si[5]) > 1e-8:
                raise ValueError("auxiliary third port is not open")
            fv, fi, fp = sv[:2], si[:2], sp[:2]

        def ports(first, second):
            return np.array([first[0], first[1], 0j, second[0], second[1], 0j])

        line_observations(
            result,
            ports(fv, lv[2:]),
            ports(fi, li[2:]),
            ports(fp, lp[2:]),
            sum(fp) + sum(lp[2:]),
        )
        return result
    if engine.CktElement.NodeOrder() != [1, 2, 0, 1, 2, 4 if floating else 0]:
        raise ValueError("unexpected service conductor order")

    v, i, p = (
        unpack(engine.CktElement.Voltages()),
        unpack(engine.CktElement.Currents()),
        1000 * unpack(engine.CktElement.Powers()),
    )
    line_observations(result, v, i, p, complex(*engine.CktElement.Losses()))
    return result


def analytic(frequency):
    """Independent grounded three-wire line plus ideal-core circuit solve."""
    distances = np.array(
        [
            [0.0111, (0.368 + 2 * 0.08) / 12, (0.368 + 0.08) / 12],
            [(0.368 + 2 * 0.08) / 12, 0.008, (0.368 + 0.08) / 12],
            [(0.368 + 0.08) / 12, (0.368 + 0.08) / 12, 0.010],
        ]
    )
    z = np.full((3, 3), 0.00158836 * frequency, dtype=complex)
    z += (
        1j
        * 0.00202237
        * frequency
        * (np.log(1 / distances) + np.log(100 / frequency) / 2 + 7.6786)
    )
    z += np.diag([1.535, 1.8, 2.0])
    line_y = np.linalg.inv(z * 100 / 5280)
    y = np.zeros((5, 5), dtype=complex)
    for k, sign in enumerate((1, -1)):
        incidence = np.zeros(5)
        incidence[0], incidence[1 + k] = sign / 2, -1
        y += np.outer(incidence, incidence) / SECONDARY_Z[k]
        y[3 + k, 3 + k] += 1 / LOAD_Z[k]
    pair = np.array([0, 0, 0, 1, -1])
    y += np.outer(pair, pair) / PAIR_Z
    incidence = np.array([[0, 1, 0, -1, 0], [0, 0, 1, 0, -1]])
    y += incidence.T @ line_y[:2, :2] @ incidence
    core_y = (1 / 50 - 1j / 100) / (240**2 / 10000)
    y[0, 0] += 1 / PRIMARY_Z + core_y
    u, h1, h2, l1, l2 = np.linalg.solve(y, np.array([240 / PRIMARY_Z, 0, 0, 0, 0]))
    ip = (240 - u) / PRIMARY_Z
    i1, i2 = (u / 2 - h1) / SECONDARY_Z[0], (-u / 2 - h2) / SECONDARY_Z[1]
    loss = (
        abs(ip) ** 2 * PRIMARY_Z
        + abs(i1) ** 2 * SECONDARY_Z[0]
        + abs(i2) ** 2 * SECONDARY_Z[1]
        + abs(u) ** 2 * core_y.conjugate()
    )
    result = snapshot(240, [h1, h2, 0j], ip, [i1, i2, -i1 - i2], loss, True)
    current = line_y @ np.array([h1 - l1, h2 - l2, 0j])
    v = np.array([h1, h2, 0, l1, l2, 0])
    i = np.concatenate((current, -current))
    p = v * i.conjugate()
    line_observations(result, v, i, p, sum(p))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "scope": __doc__,
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for label, phase, text in cases():
        for profile in ("pnnl", "arras"):
            directory = args.output / label / profile
            directory.mkdir(parents=True)
            source = directory / "model.glm"
            source.write_text(text)
            command = [
                str(args.bridge),
                str(source),
                str(directory / "network.pio.json"),
                profile,
                "--dss-output",
                str(directory / "dss"),
            ]
            proc = subprocess.run(
                command, capture_output=True, text=True, timeout=60, check=False
            )
            (directory / "bridge.stdout").write_text(proc.stdout)
            (directory / "bridge.stderr").write_text(proc.stderr)
            proc.check_returncode()
            if not json.loads(proc.stdout)["ir_network_equal"]:
                raise ValueError("IR changed the network")
            run_glm(profile, directory, args.pnnl_root, outputs=OUTPUTS)
            native = native_snapshot(directory, phase)
            actual = imported_snapshot(directory / "dss/case.dss", phase)
            checks = {"native_vs_import": compare(native, actual)}
            if label in ("A-NR-Z", "frequency-50Hz"):
                expected = analytic(50 if label == "frequency-50Hz" else 60)
                checks.update(
                    analytic_vs_native=compare(expected, native),
                    analytic_vs_import=compare(expected, actual),
                )
                changed = imported_snapshot(
                    directory / "dss/case.dss", phase, floating=True
                )
                defect = compare(native, changed)
                checks["floating_neutral_negative"] = {
                    "passed": not defect["passed"],
                    "mismatch": defect,
                }
            item = {
                "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
                "checks": checks,
                "passed": all(c["passed"] for c in checks.values()),
            }
            report["cases"][f"{label}/{profile}"] = item
            (directory / "measurements.json").write_text(
                json.dumps({"native": native, "imported": actual}, indent=2) + "\n"
            )
            print(
                label,
                profile,
                item["passed"],
                checks["native_vs_import"]["failures"][:2],
                flush=True,
            )
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
    report["passed"] = all(c["passed"] for c in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
