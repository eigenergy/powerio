"""Compare edited per-branch load laws with independent native OpenDSS circuits.

The reference circuit declares one native load per electrical branch. PowerIO
receives an edited IR bank instead, with distinct coefficients and nominal
voltages; comparisons cover branch phasors and aggregate conductor currents.
"""

import argparse
import hashlib
import json
import math
import subprocess
from pathlib import Path

import opendssdirect as dss

SOLVE = "Set controlmode=off tolerance=1e-12 maxiterations=100\nSolve\n"
COEFFICIENTS = (
    (0.2, 0.3, 0.5, 0.1, 0.4, 0.5),
    (0.4, 0.1, 0.5, 0.3, 0.2, 0.5),
    (0.1, 0.7, 0.2, 0.5, 0.1, 0.4),
)


def run(cli, case, *command):
    process = subprocess.run(
        [str(cli), *map(str, command)], capture_output=True, text=True, check=False
    )
    with (case / "commands.log").open("a") as log:
        log.write(process.stdout + process.stderr)
    process.check_returncode()


def snapshot(path):
    ctx = dss.NewContext()
    ctx.Text.Command(f'Redirect "{path}"')
    if not ctx.Solution.Converged():
        raise ValueError(f"nonconverged: {path}")
    branches = {}
    currents = {}
    for name in ctx.Loads.AllNames():
        ctx.Circuit.SetActiveElement(f"Load.{name}")
        nodes = ctx.CktElement.NodeOrder()
        raw_v = ctx.CktElement.Voltages()
        raw_i = ctx.CktElement.Currents()
        if len(nodes) != 2:
            raise ValueError(f"expected one native load branch: {name}")
        voltage = complex(*raw_v[:2]) - complex(*raw_v[2:4])
        current = complex(*raw_i[:2])
        key = ".".join(map(str, nodes))
        if key in branches:
            raise ValueError(f"duplicate branch {key}")
        branches[key] = dict(
            voltage=voltage, current=current, power=voltage * current.conjugate()
        )
        for node, offset in zip(nodes, (0, 2), strict=True):
            currents[node] = currents.get(node, 0j) + complex(
                *raw_i[offset : offset + 2]
            )
    return dict(branches=branches, conductor_currents=currents)


def compare(expected, actual):
    errors = {}
    passed = bool(expected["branches"]) and bool(expected["conductor_currents"])
    passed &= expected["branches"].keys() == actual["branches"].keys()
    for sample in (expected, actual):
        numbers = list(sample["conductor_currents"].values())
        numbers.extend(
            v for branch in sample["branches"].values() for v in branch.values()
        )
        passed &= all(math.isfinite(v.real) and math.isfinite(v.imag) for v in numbers)
    passed &= (
        expected["conductor_currents"].keys() == actual["conductor_currents"].keys()
    )
    for branch, values in expected["branches"].items():
        for quantity, reference in values.items():
            error = abs(
                reference
                - actual["branches"].get(branch, {}).get(quantity, complex("inf"))
            )
            errors[f"{branch}:{quantity}"] = error
            absolute = dict(voltage=1e-6, current=1e-7, power=1e-4)[quantity]
            passed &= error <= absolute + 1e-9 * abs(reference)
    for node, reference in expected["conductor_currents"].items():
        error = abs(reference - actual["conductor_currents"].get(node, complex("inf")))
        errors[f"node{node}:current"] = error
        passed &= error <= 1e-7 + 1e-9 * abs(reference)
    return dict(passed=bool(passed), errors=errors)


def json_default(value):
    if isinstance(value, complex):
        return [value.real, value.imag]
    raise TypeError(type(value))


def prepare(cli, case, topology, pu, variation):
    split = topology == "split"
    source = (
        f"Clear\nNew Circuit.c phases=1 bus1=b.1 bus2=b.0 basekv=.12 pu={pu} r1=.01 x1=.02 r0=.01 x0=.02\n"
        f"New Vsource.leg phases=1 bus1=b.2 bus2=b.0 basekv=.12 pu={pu} angle=180 r1=.01 x1=.02 r0=.01 x0=.02\n"
        if split
        else f"Clear\nNew Circuit.c phases=3 bus1=b.1.2.3 bus2=b.0.0.0 basekv=.41569219381653055 pu={pu} r1=.01 x1=.02 r0=.03 x0=.04\n"
    )
    config = "delta" if "delta" in topology else "wye"
    nodes = {
        "wye": [1, 2, 3, 0],
        "floating": [1, 2, 3, 4],
        "split": [1, 0, 2],
        "delta": [1, 2, 3],
        "reverse_delta": [3, 2, 1],
        "open_delta": [3, 1, 2],
    }[topology]
    pairs = {
        "wye": [(1, 0), (2, 0), (3, 0)],
        "floating": [(1, 4), (2, 4), (3, 4)],
        "split": [(1, 0), (2, 0)],
        "delta": [(1, 2), (2, 3), (3, 1)],
        "reverse_delta": [(3, 2), (2, 1), (1, 3)],
        "open_delta": [(3, 1), (1, 2)],
    }[topology]
    count = len(pairs)
    nominal = 120.0 if split else 415.69219381653055 if config == "delta" else 240.0
    # Equal powers deliberately require the voltage-law difference itself to
    # trigger splitting. Separate cases isolate coefficients and voltage bases.
    p = [400.0] * count
    q = [150.0] * count
    v_nom = [
        nominal * (1 + 0.05 * i if variation != "coefficients" else 1)
        for i in range(count)
    ]
    coefficients = [
        COEFFICIENTS[i if variation != "voltages" else 0] for i in range(count)
    ]
    if variation == "powers":
        p = [400.0 + 100 * i for i in range(count)]
        q = [150.0 + 25 * i for i in range(count)]
        v_nom = [nominal] * count
        coefficients = [COEFFICIENTS[0]] * count
    if topology == "floating":
        source += "New Line.neutral phases=1 bus1=b.4 bus2=b.0 r1=.05 x1=.01 r0=.05 x0=.01 c1=0 c0=0 length=1\n"
    # The seed only supplies bus identities and the source circuit. The bank
    # is deliberately edited after public serialization; no source echo can pass.
    seed = case / "seed.dss"
    seed_nodes = [1, 2, 0] if split else nodes
    seed.write_text(
        source
        + f"New Load.bank phases={count} conn={config} bus1=b.{'.'.join(map(str, seed_nodes))} kv=.24 kw=1 kvar=.4 model=8 zipv=[.2 .3 .5 .1 .4 .5 .8]\n"
        + SOLVE
    )
    ir = case / "edited.pio.json"
    run(cli, case, "serialize", seed, "-o", ir)
    document = json.loads(ir.read_text())
    data = document["value"]["data"]
    bus = next(bus for bus in data["buses"] if bus["id"] == "b")
    ground = bus["grounded"][0] if 0 in nodes else None
    bank = data["loads"][0]
    bank.update(
        terminal_map=[ground if node == 0 else str(node) for node in nodes],
        p_nom=p,
        q_nom=q,
    )
    law = bank["voltage_model"]
    law.update(v_nom=v_nom, v_low_pu=0.5, v_min_pu=0.95, v_max_pu=1.05, cutoff_pu=0.8)
    for i, key in enumerate(
        ("alpha_z", "alpha_i", "alpha_p", "beta_z", "beta_i", "beta_p")
    ):
        law[key] = [values[i] for values in coefficients]
    ir.write_text(json.dumps(document, indent=2) + "\n")
    reference = case / "reference.dss"
    records = []
    for i, (a, b) in enumerate(pairs):
        zipv = " ".join(map(str, (*coefficients[i], 0.8)))
        records.append(
            f"New Load.ref{i} phases=1 conn={config} bus1=b.{a}.{b} kv={v_nom[i] / 1000} kw={p[i] / 1000} kvar={q[i] / 1000} model=8 zipv=[{zipv}] vlowpu=.5 vminpu=.95 vmaxpu=1.05\n"
        )
    reference.write_text(source + "".join(records) + SOLVE)
    canonical = case / "canonical.dss"
    again_ir = case / "again.pio.json"
    again = case / "again.dss"
    run(cli, case, "convert", ir, "--to", "opendss", "-o", canonical)
    run(cli, case, "serialize", canonical, "-o", again_ir)
    run(cli, case, "convert", again_ir, "--to", "opendss", "-o", again)
    return reference, canonical, again, ir


def negative_control(cli, case, ir, expected):
    """A coefficient permutation must change native currents, even at equal P/Q."""
    document = json.loads(ir.read_text())
    law = document["value"]["data"]["loads"][0]["voltage_model"]
    for key in ("alpha_z", "alpha_i", "alpha_p", "beta_z", "beta_i", "beta_p"):
        law[key] = law[key][1:] + law[key][:1]
    corrupt_ir = case / "permuted.pio.json"
    corrupt_ir.write_text(json.dumps(document, indent=2) + "\n")
    corrupt = case / "permuted.dss"
    run(cli, case, "convert", corrupt_ir, "--to", "opendss", "-o", corrupt)
    check = compare(expected, snapshot(corrupt))
    if check["passed"]:
        raise ValueError("coefficient permutation was not detected")
    return check


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
    cases = []
    for topology in (
        "wye",
        "floating",
        "split",
        "delta",
        "reverse_delta",
        "open_delta",
    ):
        for variation in ("coefficients", "voltages", "both", "powers"):
            for pu in (0.4, 0.8, 1.0, 1.2):
                name = f"{topology}-{variation}-{pu}"
                case = output / name
                case.mkdir()
                reference, canonical, again, ir = prepare(
                    cli, case, topology, pu, variation
                )
                expected = snapshot(reference)
                first = snapshot(canonical)
                second = snapshot(again)
                checks = [compare(expected, actual) for actual in (first, second)]
                negative = (
                    negative_control(cli, case, ir, expected)
                    if variation == "coefficients" and pu == 1.0
                    else None
                )
                cases.append(
                    dict(
                        id=name,
                        ir=str(ir),
                        original=expected,
                        canonical=first,
                        again=second,
                        checks=checks,
                        coefficient_permutation=negative,
                        reference_sha256=hashlib.sha256(
                            reference.read_bytes()
                        ).hexdigest(),
                        passed=all(c["passed"] for c in checks),
                        input_sha256=hashlib.sha256(ir.read_bytes()).hexdigest(),
                    )
                )
    report = dict(
        version=dss.Basic.Version(),
        cli_sha256=hashlib.sha256(cli.read_bytes()).hexdigest(),
        cases=cases,
    )
    (output / "report.json").write_text(
        json.dumps(report, default=json_default, indent=2) + "\n"
    )
    print(json.dumps(dict(passed=sum(c["passed"] for c in cases), total=len(cases))))
    if not all(c["passed"] for c in cases):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
