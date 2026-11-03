"""Native ordered Line matrices and loaded terminal observations.

The profile is ordinary multiconductor pi sections. It exercises inline/code
assignments, copy/edit/solve boundaries, historical earth-return coefficients,
unit resets and fixed switch parameters. Geometry, Kron, long-line correction,
positive-sequence conversion and conductor open/close remain separate work.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from dss_linecode_oracle import complex_values, primitive
from oracle import LOCK


def layouts():
    a = "New Line.a bus1=sourcebus bus2=end_a length=0.5 "
    b = "New Line.b bus1=sourcebus bus2=end_b "
    seq = "r1=0.2 x1=0.4 r0=0.8 x0=1.2 c1=12 c0=6"
    mat = "phases=2 rmatrix=[1|0.1 2] xmatrix=[3|0.2 4] cmatrix=[12|-1 15]"
    code = "New Linecode.c nphases=2 rmatrix=[1|0.1 2] xmatrix=[3|0.2 4] cmatrix=[12|-1 15] units=km"
    cases = {
        "default": a,
        "sequence": a + seq,
        "matrix": a + mat,
        "frequency_before": a + "basefreq=45 " + mat,
        "frequency_after": a + mat + " basefreq=45",
        "frequency_repeated": a + "basefreq=45 " + seq + " basefreq=55",
        "kxg_before_frequency": a + "xg=0.3 rho=500 basefreq=45 " + seq,
        "kxg_after_frequency": a + "basefreq=45 xg=0.3 rho=500 " + seq,
        "rg_only": a + "rg=0.07 " + seq,
        "no_earth_correction": a + "rg=0 xg=0 " + seq,
        "matrix_then_scalar": a + mat + " r1=0.7",
        "scalar_then_matrix": a + "r1=0.7 " + mat,
        "scalar_solve_then_matrix": a + "r1=0.7\nSolve\nEdit Line.a " + mat,
        "cap_alias_then_matrix": a + "b1=7 " + mat,
        "matrix_then_c0": a + mat + " c0=9",
        "matrix_then_b0": a + mat + " b0=9",
        "resize_sequence": a + seq + " phases=1 phases=4",
        "matrix_same_phases": a + mat + " phases=2",
        "switch": a + "switch=yes",
        "switch_then_sequence": a + "switch=yes " + seq + " length=0.5",
        "sequence_then_switch": a + seq + " switch=yes",
        "matrix_then_switch": a + mat + " switch=yes",
        "switch_then_matrix": a + "switch=yes " + mat,
        "switch_false": a + "switch=yes switch=no",
        "switch_numeric_false": a + "switch=1",
        "matrix_code": code + "\n" + a + "linecode=c",
        "code_late_edit": code
        + "\n"
        + a
        + "linecode=c\nEdit Linecode.c rmatrix=[5|0.5 6] basefreq=45",
        "code_rebind": code
        + "\n"
        + a
        + "linecode=c\nEdit Linecode.c rmatrix=[5|0.5 6] basefreq=45\nEdit Line.a linecode=c",
        "code_after_scalar": code + "\n" + a + "r1=0.7 linecode=c",
        "scalar_after_code": code + "\n" + a + "linecode=c r1=0.7",
        "code_after_switch": code + "\n" + a + "switch=yes linecode=c",
        "copy_pending_sequence": a + seq + "\n" + b + "like=a",
        "copy_solved_sequence": a + seq + "\nSolve\n" + b + "like=a",
        "copy_matrix": a + mat + "\n" + b + "like=a",
        "copy_earth_units": a
        + seq
        + " rg=0.3 xg=0.5 rho=500 units=km\n"
        + b
        + "rg=0.04 xg=0.06 rho=80 units=ft like=a",
        "copy_pending_target": a + mat + "\n" + b + "r1=0.7 like=a",
        "copy_same_phase": a + seq + "\n" + b + "r1=0.7\nSolve\nEdit Line.b like=a",
        "copy_self": a + seq + "\nEdit Line.a like=a",
        "copy_self_pending": a + "\nSolve\nEdit Line.a r1=0.7 like=a",
        "copy_nested": a
        + seq
        + "\n"
        + b
        + "like=a\nSolve\nNew Line.c bus1=sourcebus bus2=end_c like=b",
        "copy_switch_flag": a + "switch=yes\n" + b + "like=a",
        "copy_enable": a + seq + " enabled=no\n" + b + "enabled=no like=a",
        "copy_ratings": a
        + seq
        + " normamps=120 emergamps=150\n"
        + b
        + "normamps=40 emergamps=50 like=a",
        "basefreq_cached": a + seq + "\nSolve\nEdit Line.a basefreq=45\nSolve",
        "basefreq_buildy_cached": a
        + seq
        + "\nSolve\nEdit Line.a basefreq=45\nBuildY\nSolve",
        "rho_cached": a + seq + "\nSolve\nEdit Line.a rho=500\nSolve",
        "rho_buildy_cached": a + seq + "\nSolve\nEdit Line.a rho=500\nBuildY\nSolve",
        "b_alias_cached": a + seq + "\nSolve\nEdit Line.a b1=7\nSolve",
        "b_alias_buildy_cached": a + seq + "\nSolve\nEdit Line.a b1=7\nBuildY\nSolve",
        "calcvoltagebases": a + seq + "\nCalcVoltageBases\n" + b + "like=a",
    }
    cases.update(
        {
            "disable_line": a + seq + "\nDisable Line.a",
            "enable_line": a + seq + " enabled=no\nEnable Line.a",
            "disable_wildcard": a + seq + "\n" + b + seq + "\nDisable Line.*",
            "enable_wildcard": a
            + seq
            + " enabled=no\n"
            + b
            + seq
            + " enabled=no\nEnable Line.*",
            "wildcard_active_selection": a
            + seq
            + "\n"
            + b
            + seq
            + "\nDisable Line.*\n~ enabled=yes",
            "explicit_active_selection": a
            + seq
            + "\n"
            + b
            + seq
            + "\nDisable Line.a\n~ enabled=yes",
            "wildcard_then_new": a + seq + "\nDisable Line.*\n" + b + seq,
            "disable_copy_enable": a + seq + "\nDisable Line.a\n" + b + "like=a",
        }
    )
    for key, edit in (
        ("basefreq", "basefreq=45"),
        ("rho", "rho=500"),
        ("b_alias", "b1=7"),
    ):
        cases[f"{key}_length_rebuild"] = (
            a
            + seq
            + f"\nSolve\nEdit Line.a {edit}\nBuildY\nEdit Line.a length=0.5\nSolve"
        )
    for n in (1, 2, 3, 4):
        cases[f"sequence_{n}"] = a + f"phases={n} " + seq
        cases[f"inline_default_{n}"] = a + f"phases={n}"
        cases[f"code_sequence_{n}"] = (
            f"New Linecode.c nphases={n} " + seq + "\n" + a + "linecode=c"
        )
    for unit in ("none", "mi", "kft", "km", "m", "ft", "in", "cm", "mm"):
        cases[f"units_before_{unit}"] = a + f"units={unit} " + seq
        cases[f"units_after_{unit}"] = a + seq + f" units={unit}"
        cases[f"default_cap_{unit}"] = a + f"r1=0.2 units={unit}"
        cases[f"code_units_{unit}"] = (
            code + "\n" + a + f"linecode=c units={unit} length=500"
        )
    cases["units_repeated"] = a + seq + " units=km units=ft units=m"
    cases["matrix_units_reset"] = a + "units=km " + mat + " units=ft"
    cases.update({
        "ieee4_short_cmatrix": a + "phases=4 cmatrix=(2.849 | -.922454 3.00287 | -.353318 -.587272 2.70828)\nNew Reactor.matrix_reference phases=1 bus1=sourcebus.4 r=.1 x=0",
        "short_rmatrix": a + "rmatrix=(1|.2 2)",
        "short_xmatrix": a + "xmatrix=(1|.2 2)",
        "upper_entries_retained": a + "rmatrix=(1 .7 .8|.2 2)",
        "asymmetric_rows_overwrite": a + "rmatrix=(1 .7 .8|.2 2 .9|.3 .4 3)",
        "empty_middle_row": a + "cmatrix=(1||.3 .4 3)",
        "empty_first_row": a + "cmatrix=(|.2 2|.3 .4 3)",
        "extra_matrix_row": a + "cmatrix=(1|.2 2|.3 .4 3|99)",
        "empty_matrix_token": a + mat + " rmatrix=() cmatrix=()",
        "blank_matrix_token": a + "cmatrix=( )",
    })
    return cases


def observe(script, output):
    output.mkdir(parents=True, exist_ok=False)
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    engine.Basic.DataPath(str(output.resolve()))
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned engine")
    commands = script.splitlines()
    result = {"engine": version}
    try:
        for command in commands:
            engine.Text.Command(command)
        for name in engine.Lines.AllNames():
            engine.Lines.Name(name)
            if engine.CktElement.Enabled():
                bus = engine.CktElement.BusNames()[1]
                command = f"New Load.probe_{name} bus1={bus}.1 phases=1 kv=7.2 kw=0.001 model=2"
                engine.Text.Command(command)
                commands.append(command)
        engine.Text.Command("Solve")
        commands.append("Solve")
        if not engine.Solution.Converged():
            raise ValueError("native circuit did not converge")
        lines = {}
        for name in engine.Lines.AllNames():
            engine.Lines.Name(name)
            engine.Text.Command(f"? Line.{name}.basefreq")
            line = {
                "frequency": float(engine.Text.Result()),
                "n": engine.Lines.Phases(),
                "units": engine.Lines.Units(),
                "bus": engine.CktElement.BusNames(),
                "enabled": engine.CktElement.Enabled(),
                "is_switch": engine.Lines.IsSwitch(),
                "length": engine.Lines.Length(),
                "normamps": engine.Lines.NormAmps(),
                "emergamps": engine.Lines.EmergAmps(),
            }
            if line["enabled"]:
                line.update(
                    yprim=engine.CktElement.YPrim(),
                    voltages=engine.CktElement.Voltages(),
                    currents=engine.CktElement.Currents(),
                )
            lines[name] = line
        result["lines"] = lines
    except Exception as error:
        result["error"] = str(error)
    result["script"] = "\n".join(commands) + "\n"
    result["sha256"] = hashlib.sha256(result["script"].encode()).hexdigest()
    return result


def compare(native_path, typed_path, output):
    report = json.loads(native_path.read_text())
    typed = json.loads(typed_path.read_text())
    if report["failures"] or report["cases"].keys() != typed.keys():
        raise ValueError("incomplete native or typed case set")
    findings = {
        "cases": len(typed),
        "lines": 0,
        "complex_observables": 0,
        "failures": [],
        "roundoff_limited": [],
        "resistance_negative_controls": 0,
    }
    for case, native in report["cases"].items():
        enabled = {name for name, line in native["lines"].items() if line["enabled"]}
        if enabled != typed[case].keys():
            raise ValueError(f"{case}: incomplete enabled line set")
        for name, actual in typed[case].items():
            line = native["lines"][name]
            y = primitive(actual["r"], actual["x"], actual["b"])
            expected = complex_values(line["yprim"]).reshape(y.shape, order="F")
            currents = y @ complex_values(line["voltages"])
            native_currents = complex_values(line["currents"])
            if not all(
                np.isfinite(values).all()
                for values in (
                    y,
                    expected,
                    currents,
                    native_currents,
                    complex_values(line["voltages"]),
                )
            ):
                raise ValueError(f"{case}/{name}: nonfinite electrical observation")
            findings["lines"] += 1
            findings["complex_observables"] += y.size + currents.size
            if not np.allclose(y, expected, rtol=1e-10, atol=1e-11):
                findings["failures"].append(
                    f"{case}/{name}: YPrim (max error {np.max(abs(y - expected))})"
                )
            voltage = complex_values(line["voltages"])
            # A complex multiply-add takes up to eight real operations. Bound
            # both dot products using gamma_k, rather than relaxing the matrix
            # tolerance or assigning every current a larger absolute tolerance.
            k = 8 * y.shape[1] + 4
            unit_roundoff = np.finfo(float).eps / 2
            gamma = k * unit_roundoff / (1 - k * unit_roundoff)
            bound = (
                1e-9
                + 1e-10 * abs(native_currents)
                + gamma * ((abs(y) + abs(expected)) @ abs(voltage))
            )
            error = abs(currents - native_currents)
            self_error = abs(expected @ voltage - native_currents)
            if np.any(error > bound) or np.any(self_error > bound):
                findings["failures"].append(
                    f"{case}/{name}: terminal current exceeds floating-point bound"
                )
            if not np.allclose(currents, native_currents, rtol=1e-10, atol=1e-9):
                findings["roundoff_limited"].append(
                    {
                        "line": f"{case}/{name}",
                        "max_error": float(max(error)),
                        "native_self_error": float(max(self_error)),
                        "max_bound": float(max(bound)),
                    }
                )
            perturbed = primitive(
                1.0001 * np.asarray(actual["r"]), actual["x"], actual["b"]
            )
            if np.allclose(perturbed, expected, rtol=1e-10, atol=1e-11):
                findings["failures"].append(
                    f"{case}/{name}: resistance negative control failed"
                )
            findings["resistance_negative_controls"] += 1
    output.mkdir(parents=True, exist_ok=False)
    (output / "comparison.json").write_text(json.dumps(findings, indent=2) + "\n")
    print(
        json.dumps({**findings, "roundoff_limited": len(findings["roundoff_limited"])})
    )
    if findings["failures"]:
        raise SystemExit(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--typed", type=Path)
    args = parser.parse_args()
    if args.compare or args.typed:
        if not args.compare or not args.typed:
            parser.error("--compare and --typed must be supplied together")
        compare(args.compare, args.typed, args.output)
        return
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}, "failures": []}
    for label, body in layouts().items():
        for constructor in (50, 60):
            for operating in (50, 60, 70):
                name = f"{label}/{constructor}/{operating}"
                script = f"Clear\nSet defaultbasefrequency={constructor}\nNew Circuit.c\nEdit Vsource.source frequency={operating}\nSet frequency={operating} controlmode=off longlinecorrection=no\n{body}\n"
                result = observe(script, args.output / "native" / name)
                report["cases"][name] = result
                if "error" in result:
                    report["failures"].append(name)
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(report["cases"]), "failures": report["failures"]}))
    if report["failures"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
