"""Qualify explicit phase-pair and earth-return P/I/Z loads against KCL.

Both native solvers and pinned forks are exercised. FBS inputs include an ideal
contact so canonical output also takes the FBS path. NR uses a direct line.
An independent nonlinear fixed-point solve separates linear admittance from
power/current injections and verifies its final current-balance residual.
"""

import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
from oracle import add, cases, command, compare, glm, literal, read_glm, run_glm
from roundtrip_oracle import instrumentation


def variants():
    return {
        "ab_impedance": [("impedance", "AB", 80 + 20j)],
        "bc_impedance": [("impedance", "BC", 65 + 15j)],
        "ca_impedance": [("impedance", "CA", 70 - 10j)],
        "an_current": [("current", "AN", 2 - 0.5j)],
        "ab_current": [("current", "AB", 2 - 0.5j)],
        "ca_current": [("current", "CA", 1 + 0.4j)],
        "mixed_zip": [
            ("power", "AB", 500 + 100j),
            ("current", "BC", -0.2 - 1j),
            ("impedance", "CA", 75 + 15j),
            ("power", "A", 150 + 30j),
            ("power", "AN", 200 + 40j),
            ("current", "BN", -0.3 - 0.6j),
            ("impedance", "CN", 100 + 10j),
        ],
        "capacitive_pairs": [
            ("impedance", "AB", -400j),
            ("impedance", "BC", -500j),
            ("impedance", "CA", -600j),
            ("impedance", "AN", -350j),
        ],
    }


def incidence(suffix):
    result = np.zeros(3)
    result["ABC".index(suffix[0])] = 1
    if len(suffix) == 2 and suffix[1] != "N":
        result["ABC".index(suffix[1])] = -1
    return result


def parameters():
    source, z, loads = cases()["coupled_unbalanced"]
    z += 1.5j * np.eye(3)  # Angle shifts distinguish current from a fixed phasor.
    return source, z, loads


def reference(terms, edited=False, wrong_connection=False, fixed_current=False):
    nominal, z, loads = parameters()
    source = nominal.copy()
    scale = 1.25 if edited else 1
    if edited:
        source[0] *= 1.03
        z *= 1.1
    y = np.diag(scale / loads)
    dynamic = []
    for law, suffix, value in terms:
        m = incidence(suffix)
        if wrong_connection:
            # Move to a different phase pair. Reversing both terminals of a
            # passive impedance alone is electrically identical, not a defect.
            m = m[[2, 0, 1]]
        if law == "impedance":
            y += np.outer(m, m) * scale / value
        else:
            vref = m @ nominal
            dynamic.append((law, m, value * scale, vref / abs(vref)))

    def injection(voltage):
        result = np.zeros(3, dtype=complex)
        for law, m, value, unit in dynamic:
            v = m @ voltage
            if abs(v) < 1:
                raise ValueError("oracle reference left its qualified voltage domain")
            current = (
                (value / v).conjugate()
                if law == "power"
                else value
                if fixed_current
                else value * (v / abs(v)) / unit
            )
            result += m * current
        return result

    sink = source.copy()
    for _iteration in range(1000):
        nxt = np.linalg.solve(np.eye(3) + z @ y, source - z @ injection(sink))
        if max(abs(nxt - sink)) < 1e-11:
            sink = nxt
            break
        sink = nxt
    else:
        raise ValueError("independent reference failed to converge")
    current = np.linalg.solve(z, source - sink)
    residual = float(max(abs(current - y @ sink - injection(sink))))
    if residual > 1e-9:
        raise ValueError(f"independent KCL residual {residual}")
    result = {}
    for i, p in enumerate("ABC"):
        add(result, f"source.{p}.voltage", "V", source[i])
        add(result, f"sink.{p}.voltage", "V", sink[i])
        for side, v, sign in [("from", source, 1), ("to", sink, -1)]:
            add(result, f"line.{side}.{p}.current", "A", sign * current[i])
            add(
                result,
                f"line.{side}.{p}.power",
                "VA",
                sign * v[i] * current[i].conjugate(),
            )
        for j, q in enumerate("ABC"):
            add(result, f"line.z.{p}{q}", "ohm", z[i, j])
    add(result, "line.loss", "VA", sum((source - sink) * current.conjugate()))
    return result, {"iterations": _iteration + 1, "kcl_residual_amperes": residual}


def deck(terms, solver):
    source, z, loads = parameters()
    text = glm(source, z, loads).replace(
        "solver_method NR;", f"solver_method {solver};"
    )
    props = " ".join(
        f"constant_{law}_{suffix} {literal(value)};" for law, suffix, value in terms
    )
    text = text.replace("object load { name sink;", "object load { name sink; " + props)
    if solver == "FBS":
        text = text.replace("to sink; length", "to middle; length")
        text += "\nobject node { name middle; phases ABCN; nominal_voltage 240; maximum_voltage_error 1e-10; }\nobject switch { name ideal; phases ABCN; from middle; to sink; switch_impedance 0+0j; operating_mode INDIVIDUAL; }\n"
    return text


def solve(args, profile, path, data, terms, edited=False):
    path.mkdir()
    (path / "model.glm").write_text(data)
    result = {
        "passed": False,
        "input_sha256": hashlib.sha256(data.encode()).hexdigest(),
    }
    try:
        result["version"] = run_glm(profile, path, args.pnnl_root)
        actual = read_glm(path, 3)
        expected, meta = reference(terms, edited)
        result["reference"] = meta
        result["comparison"] = compare(expected, actual)
        wrong, _ = reference(terms, edited, wrong_connection=True)
        result["connection_negative_control"] = compare(wrong, actual)
        result["passed"] = (
            result["comparison"]["passed"]
            and not result["connection_negative_control"]["passed"]
        )
        if any(law == "current" for law, _, _ in terms):
            fixed, _ = reference(terms, edited, fixed_current=True)
            result["fixed_phasor_negative_control"] = compare(fixed, actual)
            result["passed"] &= not result["fixed_phasor_negative_control"]["passed"]
    except Exception as error:
        result["error"] = str(error)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=list(variants()))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}

    def save(key, result):
        report["cases"][key] = result
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(key, "passed" if result["passed"] else result, flush=True)

    for profile in ["pnnl", "arras"]:
        for solver in ["NR", "FBS"]:
            for name, terms in variants().items():
                if args.case and name not in args.case:
                    continue
                key = f"{profile}/{solver}/{name}"
                directory = args.output / key
                directory.mkdir(parents=True)
                data = deck(terms, solver)
                source = directory / "input.glm"
                source.write_text(data)
                native = solve(args, profile, directory / "native", data, terms)
                save(key + "/native", native)
                if not native["passed"]:
                    continue
                try:
                    decoded = json.loads(
                        command(
                            [
                                str(args.bridge.resolve()),
                                str(source.resolve()),
                                str(directory.resolve()),
                                profile,
                            ],
                            directory,
                        )
                    )
                except Exception as error:
                    save(key + "/emission", {"passed": False, "error": str(error)})
                    continue
                if (directory / "fresh.glm").read_bytes() != (
                    directory / "restored.glm"
                ).read_bytes():
                    raise ValueError("IR changed phase-pair output")
                for variant in [
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ]:
                    text = (
                        directory / (variant + ".glm")
                    ).read_text() + instrumentation("ABC")
                    result = solve(
                        args,
                        profile,
                        directory / variant,
                        text,
                        terms,
                        variant == "edited",
                    )
                    result["diagnostics"] = decoded[variant]["diagnostics"]
                    save(key + "/" + variant, result)
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
