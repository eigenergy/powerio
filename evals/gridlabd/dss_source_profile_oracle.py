"""Check source reference binding, enum interpretation and snapshot profiles.

Native source EMFs are recovered from measured ports and YPrim. Non-snapshot
modes are observed but are explicitly outside this staged reader qualification.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from dss_source_state_oracle import complex_array
from oracle import LOCK


def cases():
    for property in ("daily", "yearly", "duty", "spectrum"):
        cls = "spectrum" if property == "spectrum" else "loadshape"
        spec = (
            "numharm=1 harmonic=[1] %mag=[73] angle=[19]"
            if cls == "spectrum"
            else "npts=1 interval=1 mult=[0.4]"
        )
        commands = {
            "default": (f"Edit Vsource.source {property}=DeFaUlT", None),
            "custom": (
                f"New {cls}.custom {spec}\nEdit Vsource.source {property}=custom",
                None,
            ),
            "forward": (
                f"Edit Vsource.source {property}=custom\nNew {cls}.custom {spec}",
                401,
            ),
            "overwritten_missing": (
                f"Edit Vsource.source {property}=missing {property}=default",
                401,
            ),
            "disabled_missing": (
                f"Edit Vsource.source enabled=no {property}=missing",
                401,
            ),
            "none_is_name": (f"Edit Vsource.source {property}=none", 401),
            "declared_none": (
                f"New {cls}.none {spec}\nEdit Vsource.source {property}=none",
                401 if cls == "loadshape" else None,
            ),
            "class_prefix": (f"Edit Vsource.source {property}={cls}.default", 401),
            "empty": (
                f'Edit Vsource.source {property}=default\nEdit Vsource.source {property}=""',
                None,
            ),
            "copied": (
                f"New {cls}.custom {spec}\nNew Vsource.template enabled=no {property}=custom\nEdit Vsource.source like=template enabled=yes bus1=feed",
                None,
            ),
        }
        for label, (body, error) in commands.items():
            yield f"reference/{property}/{label}", body, error
    for name in (
        "defaultload",
        "defaultgen",
        "defaultvsource",
        "linear",
        "pwm6",
        "dc6",
    ):
        yield f"spectrum/{name}", f"Edit Vsource.source spectrum={name}", None
    for property, values in {
        "sequence": (
            "positive",
            "posi",
            "negative",
            "negat",
            "zero",
            "ze",
            "nonsense",
            "1",
            "missing",
        ),
        "scantype": ("positive", "posi", "zero", "none", "n", "neg", "nonsense", "1"),
        "model": ("ideal", "ide", "thevenin", "the", "invalid", "1"),
    }.items():
        for value in values:
            invalid = value in ("1", "missing", "invalid") or (
                property == "scantype" and value == "nonsense"
            )
            yield (
                f"enum/{property}/{value}",
                f"Edit Vsource.source {property}={value}",
                303 if invalid else None,
            )
    for value in (
        "snap",
        "snapshot",
        "s",
        "daily",
        "yearly",
        "dutycycle",
        "direct",
        "time",
        "dynamic",
        "harmonic",
        "harmonict",
        "harmonics",
        "h",
        "t",
        "f",
        "y",
        "d",
        "di",
        "du",
        "har",
        "dailyx",
        "7",
        "nonsense",
    ):
        # Dynamics/harmonics require an already solved circuit. No transient
        # or harmonic calculation is executed after selecting these modes.
        yield f"mode/{value}", f"Solve controlmode=off\nSet mode={value}", None
    yield "mode/order_set", "Set mode=daily\nSet mode=snapshot", None
    yield (
        "mode/order_solve",
        "Set mode=daily\nSolve mode=snapshot controlmode=off",
        None,
    )
    yield "mode/positional", "Set stepsize=1 snapshot", None
    yield (
        "mode/variable",
        "Var @chosen=snapshot\nSet mode=daily\nSet mode=@chosen",
        None,
    )
    yield (
        "mode/clear",
        "Set mode=daily\nClear\nNew Circuit.c phases=3 basekv=0.24 pu=0.9 z1=[1 2]",
        None,
    )


def observe(script, directory):
    engine = dss.NewContext()
    directory.mkdir(parents=True, exist_ok=False)
    engine.Basic.AllowChangeDir(False)
    engine.Basic.DataPath(str(directory.resolve()))
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned engine")
    result = {
        "engine": version,
        "script": script,
        "sha256": hashlib.sha256(script.encode()).hexdigest(),
    }
    try:
        for command in script.splitlines():
            engine.Text.Command(command)
        mode = engine.Solution.ModeID().lower()
        result["mode"] = "snapshot" if mode == "snap" else mode
        result["native_accepted"] = True
        result["profile_accepted"] = result["mode"] == "snapshot"
        if result["profile_accepted"]:
            engine.Text.Command("Solve controlmode=off")
            if not engine.Solution.Converged():
                raise ValueError("snapshot did not converge")
            engine.Circuit.SetActiveElement("Vsource.source")
            n = engine.CktElement.NumPhases()
            y = complex_array(engine.CktElement.YPrim()).reshape(
                2 * n, 2 * n, order="F"
            )
            voltage = complex_array(engine.CktElement.Voltages())
            current = complex_array(engine.CktElement.Currents())
            emf = voltage[:n] - voltage[n:] - np.linalg.solve(y[:n, :n], current[:n])
            result["emf"] = np.stack((emf.real, emf.imag), axis=-1).tolist()
    except Exception as error:
        result.update(error=str(error), native_accepted=False, profile_accepted=False)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}, "failures": []}
    transitions = [
        (
            f"transition/{mode}",
            f"Solve controlmode=off\nSet mode={mode}\nSet frequency=70\nSet mode=snapshot",
            None,
        )
        for mode in ("harmonic", "dynamic")
    ]
    for name, body, error in [*cases(), *transitions]:
        script = (
            "Clear\nSet defaultbasefrequency=60\nNew Circuit.c phases=3 bus1=feed basekv=0.24 pu=0.9 z1=[1 2]\n"
            + body
        )
        result = observe(script, args.output / "native" / name)
        if name.startswith("transition/"):
            result["profile_accepted"] = False
            result["profile_gap"] = (
                "intermediate transition requires native solved state, even after returning to snapshot"
            )
        result["expected_error"] = error
        if error is None and not result["native_accepted"]:
            report["failures"].append(name)
        elif error is not None and f"#{error}" not in result.get("error", ""):
            report["failures"].append(name)
        report["cases"][name] = result
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(report["cases"]), "failures": report["failures"]}))
    if report["failures"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
