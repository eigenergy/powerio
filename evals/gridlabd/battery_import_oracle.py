"""Verify the IEEE9500 internal-battery idle state through actual IR/DSS import."""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

from oracle import LOCK, add, compare, recorded, run_glm
from reactor_source_oracle import deck as base_deck
from reactor_source_oracle import native
from solar_import_oracle import dss_snapshot


def deck(soc, pf, scheduled, solver):
    text = base_deck("unbalanced", solver)
    text += f"""
module generators;
object meter {{ name inv_meter; parent sink; phases ABCN; nominal_voltage 240; }}
object inverter {{ name inv; parent inv_meter; phases ABCN;
generator_status ONLINE; generator_mode CONSTANT_PQ; inverter_type FOUR_QUADRANT;
charge_lockout_time 1; discharge_lockout_time 1; sense_object inv_meter;
charge_on_threshold -5000; charge_off_threshold 0;
discharge_off_threshold 100000; discharge_on_threshold 150000;
inverter_efficiency .975; use_multipoint_efficiency FALSE; V_base 480;
rated_power 83333.333; max_charge_rate 250000; max_discharge_rate 250000;
P_Out {scheduled}; four_quadrant_control_mode CONSTANT_PF; power_factor {pf};
object battery {{ name bat; nominal_voltage 48; battery_capacity 500000;
state_of_charge {soc}; use_internal_battery_model true; battery_type LI_ION;
round_trip_efficiency .86; rated_power 250000; }}; }}
object recorder {{ parent inv; property VA_Out,power_in,battery_soc; interval 1; file inv.csv; }}
"""
    return re.sub(r"//[^\n]*", "", text).replace(";", ";\n")


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
    for case, (soc, pf, scheduled) in {
        "ieee9500-idle": (1, -1, -2500),
        "half-full-positive-schedule": (0.4, 0.8, 3000),
    }.items():
        for solver in ("NR", "FBS"):
            for profile in ("pnnl", "arras"):
                d = args.output / case / solver / profile
                d.mkdir(parents=True)
                source = d / "model.glm"
                source.write_text(deck(soc, pf, scheduled, solver))
                run_glm(
                    profile,
                    d,
                    args.pnnl_root,
                    outputs=("source.csv", "sink.csv", "reactor.csv", "inv.csv"),
                )
                actual = native(d, "ABC")
                state = recorded(d / "inv.csv")
                add(actual, "inverter.power", "VA", state["VA_Out"])
                proc = subprocess.run(
                    [
                        str(args.bridge),
                        str(source),
                        str(d / "network.json"),
                        profile,
                        "--dss-output",
                        str(d / "dss"),
                    ],
                    capture_output=True,
                    text=True,
                    timeout=120,
                    check=False,
                )
                (d / "bridge.json").write_text(proc.stdout)
                (d / "bridge.stderr").write_text(proc.stderr)
                proc.check_returncode()
                if not json.loads(proc.stdout)["ir_network_equal"]:
                    raise ValueError("IR mismatch")
                imported = dss_snapshot(
                    d / "dss/case.dss", "ordinary-pf-positive", "ABC"
                )
                wrong = dss_snapshot(
                    d / "dss/case.dss", "ordinary-pf-positive", "ABC", True
                )
                expected = {}
                add(expected, "inverter.power", "VA", 0j)
                checks = {
                    "native_idle": state["VA_Out"] == 0
                    and state["power_in"] == 0
                    and abs(state["battery_soc"] - soc) < 1e-12,
                    "native_vs_import": compare(actual, imported)["passed"],
                    "wrong_power_negative": not compare(
                        expected, {"inverter.power": wrong["inverter.power"]}
                    )["passed"],
                }
                report["cases"][case + "/" + solver + "/" + profile] = {
                    "passed": all(checks.values()),
                    "checks": checks,
                    "comparison": compare(actual, imported),
                }
                report["passed"] = all(v["passed"] for v in report["cases"].values())
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(case, solver, profile, checks, flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
