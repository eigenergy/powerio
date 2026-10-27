"""Qualify ordered linecode matrices, mode changes and assignment-time frequency.

This checks electrical parameter state directly, including one/four-conductor
codes and MakeLike destination semantics. It does not certify complete line
primitives after further line edits, Kron reduction, geometry, or public reader activation.
Direct code assignments additionally compare total R/X/charging recovered from
native YPrim at three operating frequencies and three line length units.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path

import numpy as np
import opendssdirect as dss
from oracle import LOCK


def layouts():
    matrix = "nphases=2 rmatrix=[1|0.1 2] xmatrix=[3|0.2 4] cmatrix=[12|-1 15]"
    sequence = "r1=0.2 x1=0.4 r0=0.8 x0=1.2 c1=12 c0=6"
    cases = {
        "default": "New Linecode.a",
        "sequence": "New Linecode.a " + sequence,
        "matrix": "New Linecode.a " + matrix,
        "matrix_frequency_after": "New Linecode.a " + matrix + " basefreq=45",
        "matrix_frequency_before": "New Linecode.a basefreq=45 " + matrix,
        "matrix_frequency_edit": "New Linecode.a "
        + matrix
        + "\nEdit Linecode.a basefreq=45",
        "matrix_c0": "New Linecode.a " + matrix + " c0=7",
        "matrix_c1": "New Linecode.a " + matrix + " c1=7",
        "matrix_b0": "New Linecode.a " + matrix + " b0=7",
        "matrix_b1": "New Linecode.a " + matrix + " b1=7",
        "matrix_r1_same_edit": "New Linecode.a " + matrix + " r1=0.7",
        "matrix_r1_new_edit": "New Linecode.a " + matrix + "\nEdit Linecode.a r1=0.7",
        "scalar_then_partial_matrix": "New Linecode.a nphases=2 r1=0.7 rmatrix=[1|0.1 2]",
        "scalar_edit_partial_matrix": "New Linecode.a nphases=2 r1=0.7\nEdit Linecode.a rmatrix=[1|0.1 2]",
        "phase_resize": "New Linecode.a " + matrix + " nphases=4",
        "phase_same_count": "New Linecode.a " + matrix + " nphases=2",
        "partial_capacitance": "New Linecode.a nphases=2 cmatrix=[12|-1 15]",
        "full_matrix": "New Linecode.a nphases=2 rmatrix=[1 0.1|0.1 2]",
        "earth_and_ratings": "New Linecode.a rg=0.04 xg=0.2 rho=250 normamps=150 emergamps=200 neutral=2 units=km",
        "copy_empty": "New Linecode.a\nNew Linecode.b like=a",
        "copy_empty_self": "New Linecode.a\nEdit Linecode.a like=a\nEdit Linecode.a like=a",
        "copy_matrix_fresh": "New Linecode.a "
        + matrix
        + " units=km\nNew Linecode.b like=a",
        "copy_matrix_destination": "New Linecode.a "
        + matrix
        + " units=km\nNew Linecode.b rmatrix=[9|0 9|0 0 9] units=ft like=a",
        "copy_then_matrix": "New Linecode.a "
        + matrix
        + "\nNew Linecode.b like=a rmatrix=[5|0.5 6]",
        "copy_prior_edit": "New Linecode.a "
        + matrix
        + "\nNew Linecode.b like=a\nEdit Linecode.b rmatrix=[5|0.5 6]",
        "copy_late_source": "New Linecode.a "
        + sequence
        + "\nNew Linecode.b like=a\nEdit Linecode.a r1=0.9 basefreq=45",
        "copy_scalar_to_matrix": "New Linecode.a "
        + sequence
        + "\nNew Linecode.b rmatrix=[9|0 9|0 0 9] like=a",
        "copy_twice_same_edit": "New Linecode.a "
        + matrix
        + "\nNew Linecode.b like=a rmatrix=[5|0.5 6] like=a",
        "copy_self_open_edit": "New Linecode.a "
        + matrix
        + "\nEdit Linecode.a r1=0.9 like=a rmatrix=[5|0.5 6]",
        "copy_nested": "New Linecode.a "
        + matrix
        + "\nNew Linecode.b like=a\nNew Linecode.c rmatrix=[9|0 9|0 0 9] like=b",
        "copy_inherited_earth": "New Linecode.a "
        + sequence
        + " rg=0.04 xg=0.2 rho=250 normamps=150 emergamps=200\nNew Linecode.b like=a",
        "frequency_changed_constructor": "New Linecode.a\nSet basefrequency=45\nNew Linecode.b like=a\nNew Linecode.c",
    }
    cases.update({
        "ieee4_short_cmatrix": "New Linecode.a nphases=4 cmatrix=(2.849 | -.922454 3.00287 | -.353318 -.587272 2.70828)\nNew Reactor.matrix_reference phases=1 bus1=sourcebus.4 r=.1 x=0",
        "short_rmatrix": "New Linecode.a rmatrix=(1|.2 2)",
        "short_xmatrix": "New Linecode.a xmatrix=(1|.2 2)",
        "upper_entries_retained": "New Linecode.a rmatrix=(1 .7 .8|.2 2)",
        "asymmetric_rows_overwrite": "New Linecode.a rmatrix=(1 .7 .8|.2 2 .9|.3 .4 3)",
        "empty_middle_row": "New Linecode.a cmatrix=(1||.3 .4 3)",
        "empty_first_row": "New Linecode.a cmatrix=(|.2 2|.3 .4 3)",
        "extra_matrix_row": "New Linecode.a cmatrix=(1|.2 2|.3 .4 3|99)",
        "empty_matrix_token": "New Linecode.a " + matrix + " rmatrix=() cmatrix=()",
        "blank_matrix_token": "New Linecode.a cmatrix=( )",
    })
    for n in (1, 2, 3, 4):
        cases[f"sequence_{n}"] = f"New Linecode.a nphases={n} " + sequence
        cases[f"susceptance_{n}"] = f"New Linecode.a nphases={n} b1=8 b0=4 basefreq=45"
        cases[f"susceptance_before_{n}"] = (
            f"New Linecode.a nphases={n} basefreq=45 b1=8 b0=4"
        )
    for unit in ("none", "mi", "kft", "km", "m", "ft", "in", "cm", "mm"):
        cases[f"units_{unit}"] = "New Linecode.a " + sequence + f" units={unit}"
    return cases


def observe(script, output, operating, line_unit):
    output.mkdir(parents=True, exist_ok=False)
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    engine.Basic.DataPath(str(output.resolve()))
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned engine")
    result = {
        "script": script,
        "engine": version,
        "sha256": hashlib.sha256(script.encode()).hexdigest(),
    }
    native_commands = script.splitlines()
    try:
        for command in script.splitlines():
            engine.Text.Command(command)
        codes = {}
        for name in engine.LineCodes.AllNames():
            engine.LineCodes.Name(name)

            def query(prop, name=name):
                engine.Text.Command(f"? linecode.{name}.{prop}")
                return float(engine.Text.Result())

            frequency = query("basefreq")
            codes[name] = {
                "n": engine.LineCodes.Phases(),
                "frequency": frequency,
                "units": engine.LineCodes.Units(),
                "sequence": engine.LineCodes.IsZ1Z0(),
                "r": engine.LineCodes.Rmatrix(),
                "x": engine.LineCodes.Xmatrix(),
                "b": [
                    value * math.tau * frequency * 1e-9
                    for value in engine.LineCodes.Cmatrix()
                ],
                "rg": query("rg"),
                "xg": query("xg"),
                "rho": query("rho"),
                "neutral": int(query("neutral")),
                "normamps": engine.LineCodes.NormAmps(),
                "emergamps": engine.LineCodes.EmergAmps(),
            }
        result["codes"] = codes
        lines = {}
        meters = [1, 1609.344, 304.8, 1000, 1, 0.3048, 0.0254, 0.01, 0.001]
        unit_names = ["none", "mi", "kft", "km", "m", "ft", "in", "cm", "mm"]
        for name, code in codes.items():
            factor = (
                meters[line_unit] / meters[code["units"]]
                if line_unit and code["units"]
                else 1
            )
            length = 0.5 / factor
            command = f"New Line.probe_{name} bus1=sourcebus bus2=end_{name} linecode={name} length={length:.17g} units={unit_names[line_unit]}"
            load = f"New Load.probe_{name} bus1=end_{name}.1 phases=1 kv=7.2 kw=0.1 model=2"
            native_commands.extend([command, load])
            engine.Text.Command(command)
            engine.Text.Command(load)
            lines[name] = {"operating": operating, "length": length, "units": line_unit}
        for command in (
            f"Edit Vsource.source frequency={operating}",
            f"Set frequency={operating} controlmode=off longlinecorrection=no",
            "Solve",
        ):
            native_commands.append(command)
            engine.Text.Command(command)
        if not engine.Solution.Converged():
            raise ValueError("native probe circuit did not converge")
        for name, line in lines.items():
            engine.Circuit.SetActiveElement(f"Line.probe_{name}")
            primitive = np.asarray(engine.CktElement.YPrim())
            n = codes[name]["n"]
            y = (primitive[0::2] + 1j * primitive[1::2]).reshape(
                (2 * n, 2 * n), order="F"
            )
            z = np.linalg.inv(-y[:n, n:])
            shunt = 2 * (y[:n, :n] + y[:n, n:])
            if not np.allclose(shunt.real, 0, rtol=0, atol=1e-12):
                raise ValueError("unexpected native shunt conductance")
            if not np.allclose(y[:n, :n], y[n:, n:], rtol=1e-12, atol=1e-12):
                raise ValueError("unequal native line endpoint blocks")
            line.update(
                r=z.real.ravel().tolist(),
                x=z.imag.ravel().tolist(),
                b=shunt.imag.ravel().tolist(),
                yprim=primitive.tolist(),
                voltages=engine.CktElement.Voltages(),
                currents=engine.CktElement.Currents(),
            )
        result["lines"] = lines
    except Exception as error:
        result["error"] = str(error)
    result["native_script"] = "\n".join(native_commands) + "\n"
    result["native_script_sha256"] = hashlib.sha256(
        result["native_script"].encode()
    ).hexdigest()
    return result


def complex_values(values):
    values = np.asarray(values)
    return values[0::2] + 1j * values[1::2]


def primitive(r, x, b):
    series = np.linalg.inv(np.asarray(r) + 1j * np.asarray(x))
    diagonal = series + 0.5j * np.asarray(b)
    return np.block([[diagonal, -series], [-series, diagonal]])


def compare(report_path, typed_path, output):
    report = json.loads(report_path.read_text())
    typed = json.loads(typed_path.read_text())
    if report["failures"] or report["cases"].keys() != typed.keys():
        raise ValueError("incomplete native or typed case set")
    comparisons = 0
    negative_controls = 0
    lines = 0
    failures = []
    for name, case in report["cases"].items():
        if case["lines"].keys() != typed[name].keys():
            raise ValueError(f"{name}: incomplete line set")
        for code, native in case["lines"].items():
            actual = typed[name][code]
            y = primitive(actual["r"], actual["x"], actual["b"])
            reference = complex_values(native["yprim"]).reshape(y.shape, order="F")
            voltage = complex_values(native["voltages"])
            current = complex_values(native["currents"])
            calculated = y @ voltage
            comparisons += y.size + current.size
            lines += 1
            if not np.allclose(y, reference, rtol=1e-10, atol=1e-11):
                failures.append(f"{name}/{code}: YPrim")
            if not np.allclose(calculated, current, rtol=1e-10, atol=1e-9):
                failures.append(f"{name}/{code}: loaded terminal current")
            parameters = case["codes"][code]
            if parameters["n"] > 1 and native["operating"] != parameters["frequency"]:
                n = parameters["n"]
                naive = primitive(
                    0.5 * np.asarray(parameters["r"]).reshape(n, n),
                    0.5 * np.asarray(parameters["x"]).reshape(n, n),
                    0.5 * np.asarray(parameters["b"]).reshape(n, n),
                )
                if np.allclose(naive, reference, rtol=1e-10, atol=1e-11):
                    failures.append(
                        f"{name}/{code}: frequency negative control did not discriminate"
                    )
                negative_controls += 1
    result = {
        "cases": len(typed),
        "lines": lines,
        "complex_observables": comparisons,
        "negative_controls": negative_controls,
        "failures": failures,
    }
    output.mkdir(parents=True, exist_ok=False)
    (output / "comparison.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    if failures:
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
            script = f"Clear\nSet defaultbasefrequency={constructor}\nNew Circuit.c\n{body}\n"
            for operating in (50, 60, 70):
                for unit in (0, 3, 5):
                    name = f"{label}/{constructor}/{operating}/{unit}"
                    result = observe(
                        script, args.output / "native" / name, operating, unit
                    )
                    if "error" in result:
                        report["failures"].append(name)
                    report["cases"][name] = result
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"cases": len(report["cases"]), "failures": report["failures"]}))
    if report["failures"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
