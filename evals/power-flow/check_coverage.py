"""Fail CI if the reference comparisons did not exercise their required checks."""

import json
from pathlib import Path
import sys

from benchmark import MEASURED, REFERENCE_CASES


def main(path):
    cases = json.loads(path.read_text())["cases"]
    failed = []

    def require_metric(result, name, check):
        if result.get("metrics", {}).get(check, {}).get("status") not in MEASURED:
            failed.append(f"{name} did not measure {check}")

    case14 = cases["matpower-case14"]
    for name in REFERENCE_CASES:
        if cases[name]["recover_original_files"]["status"] != "agree":
            failed.append(f"{name} did not recover its original files exactly")
    for target in ("egret-json", "powermodels-json", "xiidm", "jiidm", "psse", "psse35", "psse-rawx", "cgmes"):
        for stage in ("direct", "round_trip", "target_vs_return"):
            require_metric(case14["conversions"][target][stage], f"case14 via {target} ({stage})",
                           "admittance_relative_2_norm")
    if not case14["deliberate_error"]["detected"]:
        failed.append("case14 admittance comparison did not detect the changed branch reactance")
    require_metric(case14["conversions"]["pandapower-json"]["in_lightsim"],
                   "case14 in LightSim2Grid", "voltage_pu")
    ieee13 = cases["opendss-ieee13"]["conversions"]["dss"]["direct"]
    require_metric(ieee13, "IEEE 13-bus in OpenDSS", "voltage_V")
    require_metric(ieee13, "IEEE 13-bus in OpenDSS", "admittance_relative_2_norm")
    ravens = cases["mg-ravens-case3"]
    require_metric(ravens["upstream_companion_comparison"], "MG-RAVENS upstream OpenDSS example", "voltage_V")
    require_metric(ravens["conversions"]["bmopf-json"]["direct"], "MG-RAVENS to BMOPF in Tellegen", "voltage_V")
    if cases["gridlabd-ieee4"]["original_solution"]["status"] != "solved":
        failed.append("GridLAB-D did not solve the IEEE four-node feeder")
    for detail in failed:
        print(detail, file=sys.stderr)
    return bool(failed)


if __name__ == "__main__":
    sys.exit(main(Path(sys.argv[1])))
