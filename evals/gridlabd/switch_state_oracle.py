"""Qualify fixed switch phase states using an independently solved parallel circuit.

A coupled two-section bypass keeps all phases energized when contacts open.
Its intermediate bus avoids the engines' duplicate parallel-admittance entries.
The bypass observables aggregate its source and sink ends and both losses. These NR cases
exercise static initialization, not control execution or protection sequences.
The oracle does not establish PowerIO switch conversion support.
"""

import argparse
import hashlib
import json
import re
from pathlib import Path

import numpy as np
from oracle import add, cases, compare, glm, recorded, run_glm


def variants():
    return [
        ("default", "ABC", "", (True, True, True)),
        (
            "individual_b_open",
            "ABC",
            "operating_mode INDIVIDUAL; phase_B_state OPEN;",
            (True, False, True),
        ),
        (
            "individual_all_open",
            "ABC",
            "operating_mode INDIVIDUAL; phase_A_state OPEN; phase_B_state OPEN; phase_C_state OPEN;",
            (False, False, False),
        ),
        (
            "individual_status_open",
            "ABC",
            "operating_mode INDIVIDUAL; status OPEN; phase_A_state CLOSED;",
            (False, False, False),
        ),
        ("bank_majority_closed", "ABC", "phase_B_state OPEN;", (True, True, True)),
        (
            "bank_majority_open",
            "ABC",
            "phase_A_state OPEN; phase_B_state OPEN;",
            (False, False, False),
        ),
        (
            "bank_open_overrides_majority",
            "ABC",
            "status OPEN; phase_B_state OPEN;",
            (False, False, False),
        ),
        (
            "bank_closed_loses_to_majority",
            "ABC",
            "status CLOSED; phase_A_state OPEN; phase_B_state OPEN;",
            (False, False, False),
        ),
        ("bank_tie_closed", "AB", "phase_B_state OPEN;", (True, True, False)),
        (
            "bank_tie_open",
            "AB",
            "status OPEN; phase_B_state OPEN;",
            (False, False, False),
        ),
        (
            "individual_ac",
            "AC",
            "operating_mode INDIVIDUAL; phase_C_state OPEN; phase_B_state CLOSED;",
            (True, False, False),
        ),
        ("single_c", "C", "", (False, False, True)),
    ]


def reference(source, bypass_z, loads, closed, switch_z):
    bypass_y = np.linalg.inv(bypass_z)
    contact_y = np.diag(np.array(closed, dtype=float) / switch_z)
    total_y = bypass_y + contact_y
    sink = np.linalg.solve(total_y + np.diag(1 / loads), total_y @ source)
    contact_i = contact_y @ (source - sink)
    bypass_i = bypass_y @ (source - sink)
    result = {}
    for i, phase in enumerate("ABC"):
        add(result, f"source.{phase}.voltage", "V", source[i])
        add(result, f"sink.{phase}.voltage", "V", sink[i])
        for name, current in [("contact", contact_i), ("bypass", bypass_i)]:
            for side, voltage, sign in [("from", source, 1), ("to", sink, -1)]:
                add(result, f"{name}.{side}.{phase}.current", "A", sign * current[i])
                add(
                    result,
                    f"{name}.{side}.{phase}.power",
                    "VA",
                    sign * voltage[i] * current[i].conjugate(),
                )
    for name, current in [("contact", contact_i), ("bypass", bypass_i)]:
        add(result, f"{name}.loss", "VA", sum((source - sink) * current.conjugate()))
    if np.max(np.abs(contact_i + bypass_i - sink / loads)) > 1e-10:
        raise ValueError("analytic circuit does not satisfy KCL")
    return result


def snapshot(directory, contact_files=("line.csv",)):
    result = {}
    for bus in ["source", "sink"]:
        values = recorded(directory / f"{bus}.csv")
        for phase in "ABC":
            add(result, f"{bus}.{phase}.voltage", "V", values[f"voltage_{phase}"])
    for name, filename in [("contact", "line.csv"), ("bypass", "bypass.csv")]:
        if name == "contact":
            rows = [recorded(directory / path) for path in contact_files]
            if not rows or any(row.keys() != rows[0].keys() for row in rows):
                raise ValueError("missing or inconsistent contact measurement channels")
            values = {key: sum(row[key] for row in rows) for key in rows[0]}
        else:
            values = recorded(directory / filename)
        end = recorded(directory / "bypass_end.csv") if name == "bypass" else values
        for phase in "ABC":
            for side, suffix, sign in [("from", "in", 1), ("to", "out", -1)]:
                side_values = values if side == "from" else end
                for quantity, unit in [("current", "A"), ("power", "VA")]:
                    add(
                        result,
                        f"{name}.{side}.{phase}.{quantity}",
                        unit,
                        sign * side_values[f"{quantity}_{suffix}_{phase}"],
                    )
        losses = values["power_losses"]
        if name == "bypass":
            losses += end["power_losses"]
        add(result, f"{name}.loss", "VA", losses)
    return result


def run_case(profile, directory, pnnl_root, phases, properties, closed):
    source, bypass_z, loads = cases()["coupled_unbalanced"]
    deck = (
        glm(source, bypass_z, loads)
        .replace(
            "object overhead_line { name line;", "object overhead_line { name bypass;"
        )
        .replace("to sink; length 5280 ft;", "to middle; length 2640 ft;")
    )
    deck += "object node { name middle; phases ABCN; nominal_voltage 240; }\n"
    deck += "object overhead_line { name bypass_end; phases ABCN; from middle; to sink; length 2640 ft; configuration cfg; }\n"
    recorder = re.search(r"object recorder \{ parent line;[^}]+\}", deck).group()
    deck += (
        recorder.replace("parent line;", "parent bypass;").replace(
            '"line.csv"', '"bypass.csv"'
        )
        + "\n"
    )
    deck += (
        recorder.replace("parent line;", "parent bypass_end;").replace(
            '"line.csv"', '"bypass_end.csv"'
        )
        + "\n"
    )
    deck += f"object switch {{ name line; phases {phases}N; from source; to sink; switch_impedance 1+0.5j; {properties} }}\n"
    deck = "#set savefile=states.glm\n" + deck
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "model.glm").write_text(deck)
    version = run_glm(
        profile,
        directory,
        pnnl_root,
        outputs=(
            "source.csv",
            "sink.csv",
            "line.csv",
            "bypass.csv",
            "bypass_end.csv",
            "states.glm",
        ),
    )
    actual = snapshot(directory)
    expected = reference(source, bypass_z, loads, closed, 1 + 0.5j)
    comparison = compare(expected, actual)
    opposite = compare(
        reference(source, bypass_z, loads, tuple(not x for x in closed), 1 + 0.5j),
        actual,
    )
    bodies = re.findall(
        r"^object [\w.:]+ \{(.*?)^\}",
        (directory / "states.glm").read_text(),
        re.M | re.S,
    )
    body = next(body for body in bodies if re.search(r'^\s*name "?line"?;', body, re.M))
    saved = {}
    implicit_defaults = []
    for phase in phases:
        match = re.search(rf'^\s*phase_{phase}_state "?(OPEN|CLOSED)"?;', body, re.M)
        if match:
            saved[phase] = match.group(1)
        elif profile == "arras":
            # Arras omits default-valued properties from saved GLM. Both pinned
            # switch constructors initialize phase states CLOSED; the electrical
            # comparison independently checks that omitted contacts conduct.
            saved[phase] = "CLOSED"
            implicit_defaults.append(phase)
        else:
            raise ValueError(f"missing native phase_{phase}_state")
    states_match = all(
        saved[phase] == ("CLOSED" if closed["ABC".index(phase)] else "OPEN")
        for phase in phases
    )
    return {
        "passed": comparison["passed"] and not opposite["passed"] and states_match,
        "version": version,
        "input_sha256": hashlib.sha256(deck.encode()).hexdigest(),
        "electrical_comparison": comparison,
        "opposite_state_negative_control": opposite,
        "declared_phases": phases,
        "expected_closed": closed,
        "saved_states": saved,
        "implicit_default_states": implicit_defaults,
        "states_match": states_match,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path)
    parser.add_argument(
        "--profiles", nargs="+", choices=("pnnl", "arras"), default=["pnnl", "arras"]
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    report = {"scope": __doc__, "cases": {}}
    for profile in args.profiles:
        for name, phases, properties, closed in variants():
            key = f"{profile}/{name}"
            try:
                result = run_case(
                    profile,
                    args.output / profile / name,
                    args.pnnl_root,
                    phases,
                    properties,
                    closed,
                )
            except Exception as error:
                result = {"passed": False, "error": str(error)}
            report["cases"][key] = result
            (args.output / "report.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
            print(f"{key}: {'passed' if result['passed'] else 'FAILED'}", flush=True)
    return 0 if all(case["passed"] for case in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
