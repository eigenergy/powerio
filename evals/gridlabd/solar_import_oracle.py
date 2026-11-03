"""Static solar-fed inverter import through IR and actual OpenDSS emission.

Both pinned forks exercise solar thermal defaults, power-factor signs, clipping,
weather temperature, phase allocation and center-tapped service connections.
Native inverter power is checked independently; all physical feeder-port
observations use the existing qualified reactor/service collectors.
"""

import argparse
import hashlib
import json
import math
import re
import subprocess
from pathlib import Path

from oracle import LOCK, add, compare, recorded, run_glm
from reactor_source_oracle import deck as ordinary_deck
from reactor_source_oracle import native as ordinary_native
from reactor_source_oracle import snapshot
from split_phase_import_oracle import cases as service_cases
from split_phase_import_oracle import read_dss as service_dss
from split_phase_import_oracle import read_native as service_native

CASES = {
    "ordinary-pf-positive": (False, 0, 13990, 3000, 1, 0.8, 59, 1, True),
    "ordinary-pf-negative": (False, 0, 1000, 5000, 0.95, -0.8, 104, 0.5, True),
    "ordinary-pure-q": (False, 0, 1000, 5000, 1, 0, 59, 1, True),
    "ordinary-offline": (False, 0, 1000, 5000, 1, 1, 59, 1, False),
    "service-a": (True, 0, 500, 1000, 1, 1, 59, 1, True),
    "service-b": (True, 1, 800, 1000, 0.95, -0.9, 77, 1, True),
    "service-c": (True, 2, 1000, 500, 0.98, 0.8, 59, 1, True),
    "ieee9500-pv-on-service": (True, 5, 13990, 17480, 1, 1, 59, 1, True),
}


def expected(case):
    triplex, _, rating, limit, eta, pf, temp, shade, online = CASES[case]
    insolation = 93.4458 * shade
    dc = (
        rating
        * insolation
        / 92.902
        * (1 + (-0.00437 / 33.8) * (temp + (118.4 - 68) / 74.32 * insolation - 77))
    )
    available = dc * eta
    power = (
        complex(
            available, -math.copysign(available / abs(pf) * math.sqrt(1 - pf * pf), pf)
        )
        if pf
        else complex(0, available)
    )
    maximum = limit * (1 if triplex else 3)
    if abs(power) > maximum:
        power = complex(
            abs(maximum * pf),
            math.copysign(maximum * math.sqrt(1 - pf * pf), power.imag),
        )
    return power if online else 0j


def deck(case):
    triplex, index, rating, limit, eta, pf, temp, shade, online = CASES[case]
    if triplex:
        _, phase, text, _ = service_cases()[index]
        flags = phase + "S"
    else:
        phase = "ABC"
        flags = "ABCN"
        text = ordinary_deck("unbalanced", "NR")
    meter_class = "triplex_meter" if triplex else "meter"
    nominal = 120 if triplex else 240
    text += f"object {meter_class} {{ name inv_meter; parent sink; phases {flags}; nominal_voltage {nominal}; }}\n"
    text += f"""\nmodule generators;
module climate;
object climate {{ name weather; temperature {temp}; solar_direct 93.4458; latitude 45; }}
object inverter {{ name inv; parent inv_meter; phases {flags}; generator_status {"ONLINE" if online else "OFFLINE"};
inverter_type FOUR_QUADRANT; inverter_efficiency {eta}; use_multipoint_efficiency FALSE; V_base 208;
rated_power {limit}; P_Out {rating}; four_quadrant_control_mode CONSTANT_PF; power_factor {pf};
object solar {{ name pv; panel_type SINGLE_CRYSTAL_SILICON; efficiency .2; derating 1; soiling 1; rated_power {rating}; shading_factor {shade}; }};
}}
object recorder {{ parent inv; property VA_Out; interval 1; file inv.csv; }}
"""
    return phase, re.sub(r"//[^\n]*", "", text).replace(";", ";\n")


def dss_snapshot(path, case, phase, negative=False):
    import opendssdirect as dss

    e = dss.NewContext()
    e.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in e.Basic.Version():
        raise ValueError("DSS pin mismatch")
    e.Text.Command(f'redirect "{path}"')
    if negative:
        e.Generators.First()
        e.Generators.kW(e.Generators.kW() + 1)
    e.Text.Command("set tolerance=1e-12 maxiterations=1000 controlmode=off")
    e.Solution.Solve()
    if not e.Solution.Converged():
        raise ValueError("DSS did not converge")

    def unpack(v):
        return [complex(*v[i : i + 2]) for i in range(0, len(v), 2)]

    injection = 0j
    for name in e.Generators.AllNames():
        e.Circuit.SetActiveElement("generator." + name)
        injection -= 1000 * sum(unpack(e.CktElement.Powers()))
    result = {}
    add(result, "inverter.power", "VA", injection)
    if CASES[case][0]:
        if not negative:
            result.update(service_dss(path, phase))
    else:

        def volt(bus):
            e.Circuit.SetActiveBus(bus)
            v = dict(zip(e.Bus.Nodes(), unpack(e.Bus.Voltages()), strict=True))
            return {p: v[i + 1] for i, p in enumerate("ABC")}

        source, sink = volt("source"), volt("sink")
        e.Circuit.SetActiveElement("line.reactor")
        currents = unpack(e.CktElement.Currents())
        result.update(
            snapshot(
                "ABC",
                source,
                sink,
                dict(zip("ABC", currents[:3], strict=True)),
                dict(zip("ABC", currents[3:], strict=True)),
            )
        )
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("bridge", "pnnl-root", "output"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for case in CASES:
        phase, text = deck(case)
        for profile in ("pnnl", "arras"):
            directory = args.output / case / profile
            directory.mkdir(parents=True)
            source = directory / "model.glm"
            source.write_text(text)
            outputs = (
                ("source.csv", "sink.csv", "line.csv", "inv.csv")
                if CASES[case][0]
                else ("source.csv", "sink.csv", "reactor.csv", "inv.csv")
            )
            run_glm(profile, directory, args.pnnl_root, outputs=outputs)
            actual = (
                service_native(directory, phase)
                if CASES[case][0]
                else ordinary_native(directory, "ABC")
            )
            native_power = recorded(directory / "inv.csv")["VA_Out"]
            add(actual, "inverter.power", "VA", native_power)
            ref = {}
            add(ref, "inverter.power", "VA", expected(case))
            proc = subprocess.run(
                [
                    str(args.bridge),
                    str(source),
                    str(directory / "network.json"),
                    profile,
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
            if not json.loads(proc.stdout)["ir_network_equal"]:
                raise ValueError("IR mismatch")
            imported = dss_snapshot(directory / "dss/case.dss", case, phase)
            wrong = dss_snapshot(directory / "dss/case.dss", case, phase, True)
            negative = compare(ref, {"inverter.power": wrong["inverter.power"]})
            checks = {
                "analytic_vs_native_injection": compare(
                    ref, {"inverter.power": actual["inverter.power"]}
                ),
                "native_vs_import": compare(actual, imported),
                "wrong_power_negative": {
                    "passed": not negative["passed"],
                    "mismatch": negative,
                },
            }
            item = {
                "checks": checks,
                "passed": all(v["passed"] for v in checks.values()),
            }
            report["cases"][case + "/" + profile] = item
            print(
                case, profile, {k: v["passed"] for k, v in checks.items()}, flush=True
            )
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
    report["passed"] = all(v["passed"] for v in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
