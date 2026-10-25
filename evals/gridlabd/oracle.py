"""Independent static GridLAB-D/OpenDSS oracle checks, without importing PowerIO.

All snapshots use SI units, explicit terminal identities, rectangular complex
values, and currents/powers INTO each link end. No magnitude-only comparison,
implicit phase permutation, fitted angle rotation, or missing-key intersection.
"""

from __future__ import annotations

import argparse
import cmath
import csv
import hashlib
import importlib.metadata
import json
import math
import os
import re
import subprocess
import xml.etree.ElementTree as ET
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
LOCK = json.loads((HERE / "engines.json").read_text())
# Recorder output is set to 15 significant digits. These well-conditioned
# linear cases converge to <1e-7 V; tolerances also admit floating-point solver
# differences. Matrix tolerance is independent of the voltage/current limits.
TOLERANCES = {
    "V": (1e-6, 1e-9),
    "A": (1e-7, 1e-9),
    "VA": (1e-4, 1e-9),
    "ohm": (1e-10, 1e-10),
    "S": (1e-10, 1e-10),
}


# Whole-feeder agreement: one part per million plus SI floors for values near
# zero. Calibration keeps the original tighter budgets; matrix fidelity does
# not inherit the feeder voltage/current/power thresholds.
FEEDER_TOLERANCES = {
    **TOLERANCES,
    "V": (1e-4, 1e-6),
    "A": (1e-5, 1e-6),
    "VA": (1e-2, 1e-6),
}


def parse_complex(value: str) -> complex:
    """GridLAB-D complex literals: rectangular j/i, polar d(deg)/r(rad)."""
    value = value.strip()
    if value.endswith(("d", "r")):
        parts = re.fullmatch(
            r"([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)"
            r"([+-](?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)[dr]",
            value,
        )
        if not parts:
            raise ValueError(f"invalid polar value: {value}")
        magnitude, angle = map(float, parts.groups())
        result = cmath.rect(
            magnitude, math.radians(angle) if value[-1] == "d" else angle
        )
    else:
        result = complex(value.replace("i", "j"))
    if not math.isfinite(result.real) or not math.isfinite(result.imag):
        raise ValueError(f"nonfinite complex value: {value}")
    return result


def add(snapshot: dict, key: str, unit: str, value: complex) -> None:
    if key in snapshot:
        raise ValueError(f"duplicate observable: {key}")
    snapshot[key] = {"unit": unit, "value": [float(value.real), float(value.imag)]}


def compare(expected: dict, actual: dict, *, tolerances=None) -> dict:
    tolerances = TOLERANCES if tolerances is None else tolerances
    failures = []
    if not expected or not actual:
        failures.append("empty snapshot is not evidence")
    for key in sorted(expected.keys() - actual.keys()):
        failures.append(f"missing {key}")
    for key in sorted(actual.keys() - expected.keys()):
        failures.append(f"unexpected {key}")
    maximum = {}
    for key in sorted(expected.keys() & actual.keys()):
        ref, got = expected[key], actual[key]
        if ref["unit"] != got["unit"] or ref["unit"] not in tolerances:
            failures.append(f"{key}: incompatible units")
            continue
        a, b = complex(*ref["value"]), complex(*got["value"])
        if not all(math.isfinite(v) for v in (a.real, a.imag, b.real, b.imag)):
            failures.append(f"{key}: nonfinite value")
            continue
        error = abs(a - b)
        unit = ref["unit"]
        maximum[unit] = max(maximum.get(unit, 0.0), error)
        atol, rtol = tolerances[unit]
        if error > atol + rtol * abs(a):
            failures.append(f"{key}: {b} != {a}; absolute error {error:.8g} {unit}")
    return {
        "passed": not failures,
        "observables": len(expected),
        "max_absolute_error": maximum,
        "failures": failures,
    }


def cases() -> dict:
    return {
        "single_phase": (
            np.array([240 + 0j]),
            np.array([[1 + 0.5j]]),
            np.array([20 + 5j]),
        ),
        "coupled_unbalanced": (
            np.array(
                [
                    240,
                    cmath.rect(240, -2 * math.pi / 3),
                    cmath.rect(240, 2 * math.pi / 3),
                ]
            ),
            np.array(
                [
                    [1 + 0.5j, 0.1 + 0.2j, 0.07 + 0.15j],
                    [0.1 + 0.2j, 0.8 + 0.6j, 0.09 + 0.18j],
                    [0.07 + 0.15j, 0.09 + 0.18j, 1.2 + 0.4j],
                ]
            ),
            np.array([20 + 5j, 35 + 12j, 15 + 3j]),
        ),
    }


def analytic(source: np.ndarray, z: np.ndarray, loads: np.ndarray) -> dict:
    # Independent series-circuit solution, not either engine's nodal stamping.
    current = np.linalg.solve(z + np.diag(loads), source)
    voltage = loads * current
    result = {}
    for i in range(len(source)):
        phase = "ABC"[i]
        add(result, f"source.{phase}.voltage", "V", source[i])
        add(result, f"sink.{phase}.voltage", "V", voltage[i])
        add(result, f"line.from.{phase}.current", "A", current[i])
        add(result, f"line.to.{phase}.current", "A", -current[i])
        add(
            result, f"line.from.{phase}.power", "VA", source[i] * current[i].conjugate()
        )
        add(
            result, f"line.to.{phase}.power", "VA", -voltage[i] * current[i].conjugate()
        )
        for j in range(len(source)):
            add(result, f"line.z.{phase}{'ABC'[j]}", "ohm", z[i, j])
    add(result, "line.loss", "VA", sum((source - voltage) * current.conjugate()))
    return result


def literal(value: complex) -> str:
    return f"{value.real:.15g}{value.imag:+.15g}j"


def glm(source: np.ndarray, z: np.ndarray, loads: np.ndarray) -> str:
    phases = "ABC"[: len(source)]
    text = [
        "// Synthetic PowerIO oracle circuit; no active controls.",
        "#set complex_format=%.15g%+.15gj",
        "#set complex_output_format=RECT",
        "clock { timezone UTC0; starttime '2000-01-01 00:00:00'; "
        + "stoptime '2000-01-01 00:00:02'; }",
        "module powerflow { solver_method NR; }",
        "module tape;",
        f"object node {{ name source; phases {phases}N; bustype SWING; nominal_voltage 240;",
    ]
    text += [f"voltage_{p} {literal(source[i])};" for i, p in enumerate(phases)]
    text += [
        "}",
        f"object load {{ name sink; phases {phases}N; nominal_voltage 240;",
        "maximum_voltage_error 1e-10;",
    ]
    text += [
        f"constant_impedance_{p} {literal(loads[i])};" for i, p in enumerate(phases)
    ]
    text += ["}", "object line_configuration { name cfg;"]
    text += [
        f"z{i + 1}{j + 1} {literal(z[i, j])};"
        for i in range(len(source))
        for j in range(len(source))
    ]
    text += [
        "}",
        f"object overhead_line {{ name line; phases {phases}N; from source; "
        "to sink; length 5280 ft; configuration cfg; }",
        'object impedance_dump { filename "impedance.xml"; }',
    ]
    for bus in ("source", "sink"):
        props = ",".join(f"voltage_{p}" for p in phases)
        text.append(
            f"object recorder {{ parent {bus}; property {props}; "
            f'interval 1; file "{bus}.csv"; }}'
        )
    props = [
        f"{kind}_{p}"
        for p in phases
        for kind in ("current_in", "current_out", "power_in", "power_out")
    ]
    text.append(
        f"object recorder {{ parent line; property {','.join(props)},power_losses; "
        'interval 1; file "line.csv"; }'
    )
    return "\n".join(text) + "\n"


def dss_deck(source: np.ndarray, z: np.ndarray, loads: np.ndarray) -> str:
    n = len(source)
    # Separate finite source impedances are measured at sourcebus, not silently
    # treated as ideal. The analytic reference uses those measured phasors.
    text = [
        "! Synthetic PowerIO oracle circuit; no active controls.",
        "clear",
        "new circuit.oracle phases=1 bus1=source.1 basekv=.24 pu=1 "
        + "r1=0.000001 x1=0.000001 r0=0.000001 x0=0.000001",
    ]
    for i in range(1, n):
        text.append(
            f"new vsource.phase{i + 1} phases=1 bus1=source.{i + 1} basekv=.24 "
            f"angle={math.degrees(cmath.phase(source[i])):.15g} "
            "r1=0.000001 x1=0.000001 r0=0.000001 x0=0.000001"
        )

    def triangle(part):
        return " | ".join(
            " ".join(f"{part[i, j]:.15g}" for j in range(i + 1)) for i in range(n)
        )

    text.append(
        f"new line.line phases={n} bus1=source bus2=sink length=1 units=none "
        f"rmatrix=[{triangle(z.real)}] xmatrix=[{triangle(z.imag)}] "
        f"cmatrix=[{triangle(np.zeros_like(z.real))}]"
    )
    for i, impedance in enumerate(loads):
        power = 240**2 / impedance.conjugate() / 1000
        text.append(
            f"new load.load{i + 1} phases=1 bus1=sink.{i + 1}.0 conn=wye kv=.24 "
            f"kw={power.real:.15g} kvar={power.imag:.15g} model=2 vminpu=0 vmaxpu=2"
        )
    text += ["set controlmode=off", "set tolerance=1e-12 maxiterations=100", "solve"]
    return "\n".join(text) + "\n"


def recorded(path: Path) -> dict:
    lines = path.read_text().splitlines()
    header = next(line[2:] for line in lines if line.startswith("# timestamp,"))
    rows = list(
        csv.DictReader([header] + [s for s in lines if s and not s.startswith("#")])
    )
    if not rows:
        raise ValueError(f"no settled samples: {path}")
    return {k: parse_complex(v) for k, v in rows[-1].items() if k != "timestamp"}


def read_glm(directory: Path, n: int, line_class="overhead_line") -> dict:
    result = {}
    for bus in ("source", "sink"):
        values = recorded(directory / f"{bus}.csv")
        for phase in "ABC"[:n]:
            add(result, f"{bus}.{phase}.voltage", "V", values[f"voltage_{phase}"])
    values = recorded(directory / "line.csv")
    for phase in "ABC"[:n]:
        for side, suffix, sign in (("from", "in", 1), ("to", "out", -1)):
            for quantity, unit in (("current", "A"), ("power", "VA")):
                add(
                    result,
                    f"line.{side}.{phase}.{quantity}",
                    unit,
                    sign * values[f"{quantity}_{suffix}_{phase}"],
                )
    add(result, "line.loss", "VA", values["power_losses"])
    link = ET.parse(directory / "impedance.xml").find(line_class)
    if link is None or link.findtext("name") != "line":
        raise ValueError("missing line impedance export")
    # For this series-only line, b is the full series impedance. The general
    # dump's a/b/c/d matrices are NOT an assembled nodal admittance matrix.
    for i, phase in enumerate("ABC"[:n]):
        for j, other in enumerate("ABC"[:n]):
            add(
                result,
                f"line.z.{phase}{other}",
                "ohm",
                parse_complex(link.findtext(f"b_matrix/b{i + 1}{j + 1}")),
            )
    return result


def read_dss(path: Path, n: int) -> tuple[dict, dict]:
    import opendssdirect as dss

    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    engine.Text.Command(f'redirect "{path}"')
    if not engine.Solution.Converged():
        raise ValueError("OpenDSS did not converge")

    def complex_array(values):
        return np.array(values[0::2]) + 1j * np.array(values[1::2])

    result = {}
    for bus in ("source", "sink"):
        engine.Circuit.SetActiveBus(bus)
        for node, value in zip(
            engine.Bus.Nodes(), complex_array(engine.Bus.Voltages()), strict=True
        ):
            add(result, f"{bus}.{'ABCN'[node - 1]}.voltage", "V", value)
    engine.Circuit.SetActiveElement("line.line")
    if engine.CktElement.NumConductors() != n or engine.CktElement.NumTerminals() != 2:
        raise ValueError("unexpected OpenDSS line terminal layout")
    for quantity, unit, raw_values in (
        ("current", "A", engine.CktElement.Currents()),
        ("power", "VA", [v * 1000 for v in engine.CktElement.Powers()]),
    ):
        values = complex_array(raw_values)
        for k, side in enumerate(("from", "to")):
            for i, phase in enumerate("ABCN"[:n]):
                add(result, f"line.{side}.{phase}.{quantity}", unit, values[k * n + i])
    add(result, "line.loss", "VA", complex(*engine.CktElement.Losses()))
    # Derive Z from the engine's terminal primitive, rather than echoing R/X
    # properties that might not be the matrices used by the solver.
    yp = complex_array(engine.CktElement.YPrim()).reshape((2 * n, 2 * n), order="F")
    z = np.linalg.inv(-yp[:n, n:])
    for i, phase in enumerate("ABCN"[:n]):
        for j, other in enumerate("ABCN"[:n]):
            add(result, f"line.z.{phase}{other}", "ohm", z[i, j])
    return result, {
        "version": engine.Basic.Version(),
        "iterations": engine.Solution.Iterations(),
        "control_mode": engine.Solution.ControlMode(),
    }


def command(args: list[str], cwd: Path, env=None, *, timeout: float = 120) -> str:
    if not math.isfinite(timeout) or timeout <= 0:
        raise ValueError("execution timeout must be finite and positive")
    try:
        run = subprocess.run(
            args,
            cwd=cwd,
            env=env,
            text=True,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as error:

        def decoded(value):
            return (
                value.decode(errors="replace")
                if isinstance(value, bytes)
                else value or ""
            )

        with (cwd / "commands.jsonl").open("a") as log:
            log.write(
                json.dumps(
                    {
                        "argv": args,
                        "timed_out": True,
                        "timeout_seconds": timeout,
                        "stdout": decoded(error.stdout),
                        "stderr": decoded(error.stderr),
                    }
                )
                + "\n"
            )
        raise
    with (cwd / "commands.jsonl").open("a") as log:
        log.write(
            json.dumps(
                {
                    "argv": args,
                    "exit": run.returncode,
                    "timeout_seconds": timeout,
                    "stdout": run.stdout,
                    "stderr": run.stderr,
                }
            )
            + "\n"
        )
    if run.returncode:
        raise RuntimeError(
            f"{args[0]} exited {run.returncode}: {run.stdout}\n{run.stderr}"
        )
    return run.stdout


def run_glm(
    profile: str,
    directory: Path,
    pnnl_root: Path | None,
    outputs: tuple[str, ...] = ("source.csv", "sink.csv", "line.csv", "impedance.xml"),
    open_files: int | None = None,
    execution_timeout: float = 120,
) -> str:
    if profile == "pnnl":
        if pnnl_root is None:
            raise ValueError("--pnnl-root is required")
        for item in LOCK["pnnl"]["files"]:
            path = pnnl_root / item["path"]
            if hashlib.sha256(path.read_bytes()).hexdigest() != item["sha256"]:
                raise ValueError(f"engine pin mismatch: {path}")
        env = dict(os.environ, GLPATH=f"{pnnl_root / 'lib'}:{pnnl_root / 'share'}")
        exe = str(pnnl_root / "bin/gridlabd")
        version = command([exe, "--version"], directory, env)
        if LOCK["pnnl"]["version"] not in version:
            raise ValueError("PNNL engine version mismatch")
        command([exe, "model.glm"], directory, env, timeout=execution_timeout)
    else:
        image = LOCK["arras"]["image"]
        # The normal launcher checks GitHub after a successful command. Its
        # shell mode sets up the same runtime without that online check.
        (directory / "run.sh").write_text(
            '#!/bin/sh\nset -eu\n"$GLD_BIN/gridlabd.bin" --version\n'
            'exec "$GLD_BIN/gridlabd.bin" /case/model.glm\n'
        )
        # docker cp works with remote/Colima daemons whose /tmp is not the
        # host's /tmp. Avoid relying on silent empty bind mounts.
        container = command(
            [
                "docker",
                "create",
                "--platform",
                "linux/amd64",
                "--network",
                "none",
                "--workdir",
                "/case",
                *(
                    ["--ulimit", f"nofile={open_files}:{open_files}"]
                    if open_files
                    else []
                ),
                "--entrypoint",
                "gridlabd",
                image,
                "shell",
                "/case/run.sh",
            ],
            directory,
        ).strip()
        try:
            command(
                ["docker", "cp", str(directory) + "/.", container + ":/case"], directory
            )
            version = command(
                ["docker", "start", "--attach", container],
                directory,
                timeout=execution_timeout,
            )
            status = command(
                ["docker", "inspect", "--format", "{{.State.ExitCode}}", container],
                directory,
            )
            if status.strip() != "0":
                raise RuntimeError(f"Arras exited {status.strip()}")
            # Copy only engine outputs: copying the whole directory would
            # overwrite the host log with its stale pre-run container copy.
            for name in outputs:
                command(
                    [
                        "docker",
                        "cp",
                        container + ":/case/" + name,
                        str(directory / name),
                    ],
                    directory,
                )
        finally:
            command(["docker", "rm", "--force", container], directory)
        if LOCK["arras"]["version"] not in version:
            raise ValueError("Arras version mismatch")
    return version.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path)
    parser.add_argument(
        "--engines",
        nargs="+",
        choices=["pnnl", "arras", "dss"],
        default=["pnnl", "arras", "dss"],
    )
    args = parser.parse_args()
    output = args.output.resolve()
    # Never consume stale recorder output from a failed rerun.
    output.mkdir(parents=True, exist_ok=False)
    root = args.pnnl_root.resolve() if args.pnnl_root else None
    for package, version in LOCK["python"].items():
        if importlib.metadata.version(package) != version:
            raise ValueError(f"oracle dependency pin mismatch: {package} != {version}")
    report = {
        "scope": "independent linear circuit oracle calibration; no PowerIO conversion yet",
        "engines": args.engines,
        "cases": {},
    }
    for name, (source, z, loads) in cases().items():
        results = {}
        for profile in args.engines:
            directory = output / name / profile
            directory.mkdir(parents=True)
            if profile == "dss":
                deck = directory / "model.dss"
                deck.write_text(dss_deck(source, z, loads))
                actual, metadata = read_dss(deck, len(source))
                if LOCK["dss"]["version"] not in metadata["version"]:
                    raise ValueError("DSS engine version mismatch")
                measured_source = np.array(
                    [
                        complex(*actual[f"source.{p}.voltage"]["value"])
                        for p in "ABC"[: len(source)]
                    ]
                )
                expected = analytic(measured_source, z, loads)
            else:
                deck = directory / "model.glm"
                deck.write_text(glm(source, z, loads))
                metadata = {"version": run_glm(profile, directory, root)}
                actual = read_glm(directory, len(source))
                expected = analytic(source, z, loads)
            comparison = compare(expected, actual)
            results[profile] = dict(
                comparison,
                engine=metadata,
                input_sha256=hashlib.sha256(deck.read_bytes()).hexdigest(),
            )
            for label, snapshot in (("expected", expected), ("actual", actual)):
                (directory / f"{label}.json").write_text(
                    json.dumps(snapshot, indent=2, allow_nan=False) + "\n"
                )
            print(
                name,
                profile,
                "PASS" if comparison["passed"] else "FAIL",
                comparison["max_absolute_error"],
            )
            for failure in comparison["failures"]:
                print("  " + failure)
        report["cases"][name] = results
    (output / "report.json").write_text(
        json.dumps(report, indent=2, allow_nan=False) + "\n"
    )
    return (
        0
        if all(r["passed"] for c in report["cases"].values() for r in c.values())
        else 1
    )


if __name__ == "__main__":
    raise SystemExit(main())
