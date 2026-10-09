"""Validate bounded static load laws through public IR and canonical DSS output.

Run the ignored Rust typed-current test against the resulting report as well:
POWERIO_LOAD_VOLTAGE_SAMPLES=/path/report.json cargo test -p powerio \
    --test load_voltage_ir typed_laws_match_native_load_currents -- --ignored
"""

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import opendssdirect as dss


def snapshot(path):
    ctx = dss.NewContext()
    ctx.Text.Command(f'Redirect "{path}"')
    if not ctx.Solution.Converged():
        raise ValueError(f"native solve did not converge: {path}")
    ctx.Circuit.SetActiveElement("Load.l")
    values = ctx.CktElement.Voltages()
    voltage = complex(*values[:2]) - complex(*values[2:4])
    current = complex(*ctx.CktElement.Currents()[:2])
    power = voltage * current.conjugate()
    return {
        key: [value.real, value.imag]
        for key, value in (("voltage", voltage), ("current", current), ("power", power))
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--powerio", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    cli = args.powerio.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    lock = json.loads((Path(__file__).parent / "engines.json").read_text())
    if lock["dss"]["version"] not in dss.Basic.Version():
        raise ValueError("unpinned OpenDSS engine")
    models = {
        "power": "model=1",
        "current": "model=5",
        "impedance": "model=2",
        "zip": "model=8 zipv=[.2 .3 .5 .1 .4 .5 0]",
        "dropout": "model=8 zipv=[0 0 1 0 0 1 .8]",
    }
    cases = []
    for label, model in models.items():
        for pu in (0.25, 0.5, 0.7, 0.798, 0.8, 0.802, 0.949, 0.95, 1.0, 1.05, 1.2):
            name = f"{label}-{pu}"
            case = output / name
            case.mkdir()
            source = case / "input.dss"
            source.write_text(
                f"Clear\nNew Circuit.c phases=1 bus1=b.1 bus2=b.0 basekv=.24 pu={pu} "
                "r1=.000001 x1=.000001 r0=.000001 x0=.000001\n"
                f"New Load.l phases=1 bus1=b.1.0 kv=.24 kw=1 kvar=.5 {model} "
                "vminpu=.95 vmaxpu=1.05 vlowpu=.5\n"
                "Set controlmode=off tolerance=1e-12 maxiterations=100\nSolve\n"
            )
            ir = case / "input.pio.json"
            canonical = case / "canonical.dss"
            for command in (
                ["serialize", str(source), "-o", str(ir)],
                ["convert", str(ir), "--to", "opendss", "-o", str(canonical)],
            ):
                run = subprocess.run(
                    [str(cli), *command], text=True, capture_output=True, check=False
                )
                (case / f"{command[0]}.log").write_text(run.stdout + run.stderr)
                run.check_returncode()
            original = snapshot(source)
            actual = snapshot(canonical)
            # Same voltage/current/power thresholds as the established engine
            # microcase suite. No angle alignment or magnitude-only comparison.
            absolute = dict(voltage=1e-6, current=1e-7, power=1e-4)
            errors = {
                key: abs(complex(*original[key]) - complex(*actual[key]))
                for key in original
            }
            passed = all(
                errors[key] <= absolute[key] + 1e-9 * abs(complex(*original[key]))
                for key in original
            )
            cases.append(
                dict(
                    id=name,
                    ir=str(ir),
                    input_sha256=hashlib.sha256(source.read_bytes()).hexdigest(),
                    original=original,
                    canonical=actual,
                    errors=errors,
                    passed=passed,
                )
            )
    report = dict(
        scope="Models 1, 2, 5, 8; explicit static state; public IR and canonical DSS",
        version=dss.Basic.Version(),
        cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
        cases=cases,
    )
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(passed=sum(c["passed"] for c in cases), total=len(cases))))
    if not all(c["passed"] for c in cases):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
