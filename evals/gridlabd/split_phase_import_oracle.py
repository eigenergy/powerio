"""Center-tapped GLM import through IR and actual OpenDSS emission.

Exercises the service circuit needed by IEEE8500. This does not qualify the
whole feeder, triplex lines, or canonical GLM emission. Native models are small
synthetic circuits; no public feeder data is vendored.
"""

import argparse
import cmath
import hashlib
import json
import subprocess
from pathlib import Path

import numpy as np
from oracle import LOCK, compare, literal, recorded, run_glm
from transformer_oracle import analytic, glm, snapshot


def cases():
    result = []
    for phase, solver, law in [("A", "NR", "Z"), ("B", "FBS", "Z"), ("C", "NR", "ZIP")]:
        voltage = cmath.rect(
            240, {"A": 0, "B": -2 * np.pi / 3, "C": 2 * np.pi / 3}[phase]
        )
        text = glm(True).replace("solver_method NR", f"solver_method {solver}")
        text = text.replace("phases AN", f"phases {phase}N").replace(
            "phases AS", f"phases {phase}S"
        )
        text = text.replace("voltage_A 240+0j", f"voltage_{phase} {literal(voltage)}")
        text = text.replace("property voltage_A;", f"property voltage_{phase};")
        text = text.replace(
            "current_in_A,power_in_A", f"current_in_{phase},power_in_{phase}"
        )
        text = text.replace("powerA_rating 10", f"power{phase}_rating 10")
        text = text.replace(
            "shunt_resistance 1000000000; shunt_reactance 1000000000;",
            "shunt_resistance 50; shunt_reactance 100;",
        )
        if law == "ZIP":
            text = text.replace(
                "constant_impedance_1 10+3j;",
                "constant_power_1 1100+200j; constant_current_1 2-.4j;",
            )
            text = text.replace(
                "constant_impedance_2 20+6j;",
                "constant_power_2 600+110j; constant_current_2 1-.3j;",
            )
            text = text.replace(
                "constant_impedance_12 30+5j;",
                "constant_power_12 800+100j; constant_current_12 2-.1j; constant_impedance_12 60+10j;",
            )
        result.append((f"{phase}-{solver}-{law}", phase, text, law == "Z"))
    baseline = result[0][2]
    default = baseline.replace("solver_method NR", "solver_method FBS").replace(
        " powerA_rating 10;", ""
    )
    default = default.replace(
        "resistance1 0.015; reactance1 0.025; resistance2 0.025; reactance2 0.035;", ""
    )
    result.append(("default-rating-and-leakage", "A", default, False))
    copied = baseline.replace("resistance1 0.015; reactance1 0.025;", "")
    result.append(("copied-secondary", "A", copied, False))
    # Parameters and load style of the 15 kVA A-phase IEEE8500 services.
    service = baseline.replace("solver_method NR", "solver_method FBS")
    service = service.replace(
        "nominal_voltage 240; voltage_A 240+0j",
        "nominal_voltage 7200; voltage_A 7200+0j",
    )
    service = service.replace("primary_voltage 240;", "primary_voltage 7200;").replace(
        "power_rating 10; powerA_rating 10;", "power_rating 15; powerA_rating 15;"
    )
    service = service.replace(
        "resistance 0.01; reactance 0.02; resistance1 0.015; reactance1 0.025; resistance2 0.025; reactance2 0.035;",
        "impedance .006+.0136j; impedance1 .012+.0068j; impedance2 .012+.0068j;",
    )
    service = service.replace(
        "shunt_resistance 50; shunt_reactance 100;", "shunt_impedance 1728000+691200j;"
    )
    service = service.replace(
        "constant_impedance_1 10+3j; constant_impedance_2 20+6j;",
        "constant_power_1 1945+487.5j; constant_power_2 1945+487.5j;",
    ).replace("constant_impedance_12 30+5j;", "")
    service = service.replace(
        "name line; phases AS;", "name line; nominal_voltage 7199.558; phases AS;"
    )
    result.append(("ieee8500-service-parameters", "A", service, False))
    return result


def read_native(directory, phase):
    source = recorded(directory / "source.csv")[f"voltage_{phase}"]
    sink, link = recorded(directory / "sink.csv"), recorded(directory / "line.csv")
    result = snapshot(
        source,
        [sink["voltage_1"], -sink["voltage_2"], sink["voltage_N"]],
        link[f"current_in_{phase}"],
        [link[f"current_out_{p}"] for p in "ABC"],
        link["power_losses"],
        True,
    )
    for key, value in [
        ("transformer.from.A.power", link[f"power_in_{phase}"]),
        ("sink.L1L2.voltage", sink["voltage_12"]),
    ]:
        result[key]["value"] = [value.real, value.imag]
    for label, p in zip(("L1", "L2", "N"), "ABC", strict=True):
        value = -link[f"power_out_{p}"]
        result[f"transformer.to.{label}.power"]["value"] = [value.real, value.imag]
    return result


def read_dss(path, phase, defect=None):
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("OpenDSS version pin mismatch")
    engine.Text.Command(f'redirect "{path}"')
    if defect == "reversed-leg":
        engine.Text.Command("edit transformer.line wdg=3 bus=sink.2.0")
    elif defect == "missing-core":
        engine.Text.Command("edit load.line__glm_core_loss enabled=no")
    elif defect == "wrong-rating":
        engine.Text.Command("edit transformer.line kvas=(30 30 30 30)")
    engine.Text.Command("set tolerance=1e-12 maxiterations=100 controlmode=off")
    engine.Solution.Solve()
    if not engine.Solution.Converged() or engine.Solution.ControlMode() != -1:
        raise ValueError("OpenDSS did not converge with controls disabled")
    engine.Circuit.SetActiveElement("transformer.line")

    def unpack(raw):
        return np.array(raw[::2]) + 1j * np.array(raw[1::2])

    v, i, s = (
        unpack(engine.CktElement.Voltages()),
        unpack(engine.CktElement.Currents()),
        1000 * unpack(engine.CktElement.Powers()),
    )
    if engine.CktElement.NodeOrder()[:4] != ["ABC".index(phase) + 1, 0, 1, 0]:
        raise ValueError("unexpected primary/secondary conductor order")
    last = engine.CktElement.NodeOrder()[4:6]
    hot2, n2 = 4 + last.index(2), 4 + last.index(0)
    # The external three ports include the core branch's demand. Summing all
    # four winding powers alone would omit the separately represented core loss.
    result = snapshot(
        v[0],
        [v[2], v[hot2], 0j],
        i[0],
        [-i[2], -i[hot2], -i[3] - i[n2]],
        sum(s[:6]),
        True,
    )
    result["transformer.from.A.power"]["value"] = [s[0].real, s[0].imag]
    for label, value in zip(
        ("L1", "L2", "N"), (s[2], s[hot2], s[3] + s[n2]), strict=True
    ):
        result[f"transformer.to.{label}.power"]["value"] = [value.real, value.imag]
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
    for label, phase, text, analytic_case in cases():
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
            bridge = json.loads(proc.stdout)
            if not bridge["ir_network_equal"]:
                raise ValueError("IR did not preserve the network")
            run_glm(profile, directory, args.pnnl_root)
            native = read_native(directory, phase)
            actual = read_dss(directory / "dss/case.dss", phase)
            checks = {"native_vs_import": compare(native, actual)}
            if analytic_case:
                source_voltage = complex(*native["source.A.voltage"]["value"])
                expected = analytic(
                    source_voltage, True, (1 / 50 - 1j / 100) / (240**2 / 10000)
                )
                checks["analytic_vs_native"] = compare(expected, native)
                checks["analytic_vs_import"] = compare(expected, actual)
            if label == "A-NR-Z":
                for defect in ("reversed-leg", "missing-core", "wrong-rating"):
                    changed = read_dss(directory / "dss/case.dss", phase, defect)
                    check = compare(native, changed)
                    if check["passed"]:
                        raise ValueError(
                            f"negative control {defect} unexpectedly passed"
                        )
                    checks[defect] = {"passed": True, "mismatch": check}
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
