"""Original IEEE37/123 FBS cases through IR and physical DSS port measurements."""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

from fbs_feeder_import_oracle import pinned
from ieee8500_import_oracle import grouped
from ieee9500_import_oracle import (
    imported_snapshot,
    instrument,
    inventory,
    native_snapshot,
)
from oracle import LOCK, compare, run_glm


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--case", choices=("ieee37", "ieee123"), required=True)
    for name in ("source", "bridge", "output", "pnnl-root"):
        p.add_argument("--" + name, type=Path, required=True)
    args = p.parse_args()
    data, pin = pinned(args.source, args.case)
    text = data.decode()
    dependencies = []
    for include in re.findall(r'#include "([^"]+)";', text):
        path = (args.source.parent / include).resolve()
        content, dependency = pinned(path, args.case)
        dependencies.append(dependency)
        text = text.replace(f'#include "{include}";', content.decode())
    objects = inventory(text)
    args.output.mkdir(parents=True, exist_ok=False)
    report = {
        "engines": LOCK,
        "source": pin,
        "dependencies": dependencies,
        "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        "cases": {},
    }
    for profile in ("pnnl", "arras"):
        d = args.output / profile
        d.mkdir()
        proc = subprocess.run(
            [
                str(args.bridge),
                str(args.source),
                str(d / "network.json"),
                profile,
                "--acquisition-root",
                str(args.source.resolve().parent.parent),
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
        b = json.loads(proc.stdout)
        if not b["ir_network_equal"]:
            raise ValueError("IR mismatch")
        # Keep native fixed states and assertions. Tighten the FBS voltage
        # stopping fraction and add one recorder step to the original clock.
        model = text.replace(
            "solver_method FBS;",
            "solver_method FBS; default_maximum_voltage_error 1e-12;",
        )
        model = re.sub(
            r"(?m)^module powerflow;",
            "module powerflow { solver_method FBS; default_maximum_voltage_error 1e-12; }",
            model,
        )
        clock = re.search(r"clock\s*\{([^{}]*)\}", model)
        stamp = re.search(r"(?:timestamp|starttime)\s+'([^']+)'", clock[1])[1]
        if "stoptime" not in clock[1]:
            stop = stamp.rsplit(":", 1)[0] + ":01"
            model = (
                model[: clock.end(1)]
                + f"\n stoptime '{stop}';\n"
                + model[clock.end(1) :]
            )
        if not re.search(r"module\s+tape\s*;", model):
            model += "\nmodule tape;\n"
        model, spec = instrument(model, objects)
        if profile == "arras":
            # The fork renamed dump formatting keywords; this only changes
            # telemetry syntax, never electrical inputs or fixed states.
            model = re.sub(
                r"(object voltdump\s*\{[^{}]*?mode\s+)RECT",
                r"\1rect",
                model,
            )
            model = re.sub(r"object group_recorder\s*\{[^{}]*\}", "", model)
            fields = [
                f"{name}:{prop}"
                for name, obj in objects.items()
                for prop in spec[obj["class"]]
            ]
            chunks = [fields[i : i + 12] for i in range(0, len(fields), 12)]
            (d / "records").mkdir()
            for i, fields in enumerate(chunks):
                model += f'object multi_recorder {{ property {",".join(fields)}; file "records/chunk{i}.csv"; interval 1; }}\n'
            spec = {"chunks": chunks}
        (d / "model.glm").write_text(model)
        outputs = (
            ("records/.",)
            if profile == "arras"
            else tuple(
                f"{cls}-{prop}.csv" for cls, props in spec.items() for prop in props
            )
        )
        run_glm(
            profile,
            d,
            args.pnnl_root,
            outputs=outputs,
            open_files=65536 if profile == "arras" else None,
        )
        expected, reported = native_snapshot(grouped(d, spec), objects)
        actual, solve = imported_snapshot(d / "dss/case.dss", b["network"], objects)
        changed, _ = imported_snapshot(
            d / "dss/case.dss", b["network"], objects, wrong_tap=True
        )
        check = compare(expected, actual)
        bad = compare(expected, changed)
        item = {
            "ir_equal": True,
            "objects": len(objects),
            "measurements": len(expected),
            "solve": solve,
            "comparison": check,
            "wrong_device_negative": {"passed": not bad["passed"], "mismatch": bad},
            "passed": check["passed"] and not bad["passed"],
        }
        for name, value in (
            ("native.json", expected),
            ("imported.json", actual),
            ("reported-loss.json", reported),
        ):
            (d / name).write_text(json.dumps(value))
        report["cases"][profile] = item
        report["passed"] = all(c["passed"] for c in report["cases"].values())
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(
            profile,
            item["passed"],
            check["max_absolute_error"],
            check["failures"][:5],
            flush=True,
        )
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
