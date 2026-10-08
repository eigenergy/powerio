"""Compare staged Rust source decomposition with pinned native loaded circuits.

Preparation writes only synthetic inputs and a manifest. The ignored Rust test
reads those inputs through the real element reader with the staged source hook.
Evaluation solves the resulting typed records independently, including coupled
impedances and explicit voltage references. No source extras supply physics.
This does not enable the staged path in ordinary parsing or qualify its writer.
"""

import argparse
import copy
import hashlib
import json
from pathlib import Path

import numpy as np
import opendssdirect as dss
from dss_source_oracle import (
    PATTERNS,
    complex_array,
    deck,
    reference,
    typed_linear_snapshot,
    typed_linear_solution,
)
from oracle import LOCK, add, command, compare


def coupled_deck(phases, frequency, specification, returns):
    order = list(range(phases, 0, -1))
    reference = {
        "earth": [0] * phases,
        "common": [7] * phases,
        "separate": [7 + i if i % 2 == 0 else 0 for i in range(phases)],
    }[returns]
    lines = [
        "clear",
        f"set defaultbasefrequency={frequency}",
        f"New Circuit.c phases={phases} basekv=0.48 "
        f"bus1=feed.{'.'.join(map(str, order))} "
        f"bus2=return.{'.'.join(map(str, reference))} {specification}",
    ]
    nominal = 0.48 / (2 * np.sin(np.pi / phases)) if phases > 1 else 0.48
    for node in sorted(set(reference) - {0}):
        lines.append(
            f"New Line.ground{node} phases=1 bus1=return.{node} bus2=return.0 "
            "r1=3 x1=0.7 r0=3 x0=0.7 c1=0 c0=0 length=1 units=none"
        )
    for i, node in enumerate(order):
        lines += [
            f"New Line.wire{i} phases=1 bus1=feed.{node} bus2=sink.{node} "
            f"r1={0.3 + 0.1 * i} x1=0.6 r0=0.3 x0=0.6 c1=0 c0=0 length=1 units=none",
            f"New Load.l{i} phases=1 bus1=sink.{node}.0 conn=wye model=2 "
            f"kv={nominal:.17g} kw={0.3 + 0.1 * i} kvar=0.08 vminpu=0 vmaxpu=2",
        ]
    lines += ["set controlmode=off tolerance=1e-12 maxiterations=100", "solve"]
    return "\n".join(lines) + "\n"


def prepare(output):
    output.mkdir(parents=True, exist_ok=False)
    manifest = {}
    for pattern in PATTERNS:
        for floating in (False, True):
            for frequency in (50, 60):
                for resistance in (1.0, 10.0, 100.0):
                    key = f"equivalent/{pattern}/{floating}/{frequency}/{resistance}"
                    path = output / key / "case.dss"
                    path.parent.mkdir(parents=True)
                    path.write_text(deck(pattern, "Z", floating, frequency, resistance))
                    manifest[key] = str(path.resolve())
    specs = {
        "default": "",
        "rx": "r1=0.8 x1=1.7 r0=2.1 x0=3.4",
        "equal": "z1=[0.8 1.7] z0=[2.1 3.4]",
        "distinct": "z1=[0.8 1.7] z2=[1.2 2.3] z0=[2.1 3.4]",
        "pu": "basemva=5 puz1=[0.08 0.17] puz0=[0.21 0.34] puz2=[0.12 0.23]",
        "mva": "mvasc3=25 mvasc1=30 x1r1=5 x0r0=4",
        "current": "isc3=1000 isc1=1200 x1r1=5 x0r0=4",
        "quasi": "model=ideal puzideal=[0.01 0.02]",
    }
    for phases in (1, 2, 3, 4):
        for frequency in (50, 60):
            for kind, spec in specs.items():
                for returns in ("earth", "common", "separate"):
                    key = f"coupled/{phases}/{frequency}/{kind}/{returns}"
                    path = output / key / "case.dss"
                    path.parent.mkdir(parents=True)
                    path.write_text(coupled_deck(phases, frequency, spec, returns))
                    manifest[key] = str(path.resolve())
    (output / "inputs.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Prepared {len(manifest)} circuits")


def native(path):
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    version = engine.Basic.Version()
    if LOCK["dss"]["version"] not in version:
        raise ValueError("unpinned OpenDSS engine")
    engine.Text.Command(f'redirect "{path.resolve()}"')
    if not engine.Solution.Converged():
        raise ValueError("native circuit did not converge")
    expected, elements = {}, {}
    for native_name in engine.Circuit.AllElementNames():
        name = native_name.lower()
        if not name.startswith(("line.", "vsource.")):
            continue
        engine.Circuit.SetActiveElement(name)
        n = engine.CktElement.NumConductors()
        assert engine.CktElement.NumTerminals() == 2
        buses = engine.CktElement.BusNames()
        nodes = engine.CktElement.NodeOrder()
        elements[name] = [
            (buses[i // n].split(".")[0], str(node)) for i, node in enumerate(nodes)
        ]
        for label, unit, values in [
            ("voltage", "V", complex_array(engine.CktElement.Voltages())),
            ("current", "A", complex_array(engine.CktElement.Currents())),
            ("power", "VA", complex_array(engine.CktElement.Powers()) * 1000),
        ]:
            for i, value in enumerate(values):
                add(expected, f"{name}.{i}.{label}", unit, value)
    return expected, elements, version


def typed(document, elements):
    volts, currents, residual = typed_linear_solution(document)
    result = {}
    for name, ports in elements.items():
        cls, identifier = name.split(".", 1)
        key = f"_pio_vsource_{identifier}_impedance" if cls == "vsource" else identifier
        coupled = f"_pio_vsource__pio_coupled_{key}_impedance"
        if coupled in currents:
            current = -currents[coupled]
        else:
            current = currents[key]
        if cls == "vsource":
            current = -current
        current = np.r_[current, -current]
        assert len(ports) == len(current), (
            f"{name}: typed equivalent conductor count changed"
        )
        for i, ((bus, node), amp) in enumerate(zip(ports, current, strict=True)):
            voltage = 0j if node == "0" else volts[(bus, node)]
            add(result, f"{name}.{i}.voltage", "V", voltage)
            add(result, f"{name}.{i}.current", "A", amp)
            add(result, f"{name}.{i}.power", "VA", voltage * amp.conjugate())
    return result, residual


def canonical_snapshot(path, elements):
    engine = dss.NewContext()
    engine.Basic.AllowChangeDir(False)
    if LOCK["dss"]["version"] not in engine.Basic.Version():
        raise ValueError("unpinned OpenDSS engine")
    engine.Text.Command(f'redirect "{path.resolve()}"')
    if not engine.Solution.Converged():
        raise ValueError("canonical native circuit did not converge")
    known = {name.lower() for name in engine.Circuit.AllElementNames()}
    result = {}
    for name, ports in elements.items():
        cls, identifier = name.split(".", 1)
        target = (
            f"line._pio_vsource_{identifier}_impedance" if cls == "vsource" else name
        )
        if target not in known:
            target = "vsource._pio_coupled_" + target.split(".", 1)[1]
            if target not in known:
                raise ValueError(f"canonical circuit lost {target}")
        engine.Circuit.SetActiveElement(target)
        current = complex_array(engine.CktElement.Currents())
        if cls == "vsource":
            current = -current
        assert len(ports) == len(current), f"{target}: conductor count changed"
        for i, ((bus, node), amp) in enumerate(zip(ports, current, strict=True)):
            voltage = 0j
            if node != "0":
                if engine.Circuit.SetActiveBus(bus) < 0:
                    raise ValueError(f"canonical circuit lost bus {bus}")
                voltages = dict(
                    zip(
                        engine.Bus.Nodes(),
                        complex_array(engine.Bus.Voltages()),
                        strict=True,
                    )
                )
                voltage = voltages[int(node)]
            add(result, f"{name}.{i}.voltage", "V", voltage)
            add(result, f"{name}.{i}.current", "A", amp)
            add(result, f"{name}.{i}.power", "VA", voltage * amp.conjugate())
    return result


def evaluate(output, powerio, canonical=None):
    inputs = json.loads((output / "inputs.json").read_text())
    networks = json.loads((output / "networks.json").read_text())
    assert inputs.keys() == networks.keys() and inputs
    report = {"scope": __doc__, "cases": {}}
    for name, path in inputs.items():
        result = {
            "passed": False,
            "input_sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
        }
        try:
            expected, elements, version = native(Path(path))
            directory = output / name
            directory.mkdir(parents=True, exist_ok=True)
            original = directory / "original.pio.json"
            command(
                [
                    str(powerio.resolve()),
                    "serialize",
                    path,
                    "-o",
                    str(original.resolve()),
                ],
                directory,
            )
            document = json.loads(original.read_text())
            assert document["value"]["type"] == "powerio.MulticonductorNetwork"
            if name.startswith("equivalent/"):
                _, pattern, floating, _, _ = name.split("/")
                floating = floating == "True"
                expected_voltage, _ = reference(pattern, "Z", floating)
                original_voltage, _ = typed_linear_snapshot(
                    document, len(PATTERNS[pattern]), floating
                )
                result["production_reimport"] = compare(
                    {
                        k: v
                        for k, v in expected_voltage.items()
                        if k.endswith(".voltage")
                    },
                    original_voltage,
                )
            document["value"]["data"] = networks[name]
            encoded = directory / "staged.pio.json"
            encoded.write_text(json.dumps(document, indent=2) + "\n")
            decoded = json.loads(
                command(
                    [str(powerio.resolve()), "summary", str(encoded.resolve())],
                    directory,
                )
            )
            assert decoded["model"] == "multiconductor"
            actual, residual = typed(document, elements)
            check = compare(expected, actual)
            wrong = copy.deepcopy(document)
            _, currents, _ = typed_linear_solution(document)
            candidates = [
                (abs(amp), line, i)
                for line, amps in currents.items()
                if line.startswith("_pio_vsource_")
                for i, amp in enumerate(amps)
            ]
            amplitude, chosen, phase = max(candidates)
            if amplitude < 1e-6:
                raise ValueError("negative control requires a loaded source conductor")
            network = wrong["value"]["data"]
            line = next(v for v in network["lines"] if v["name"] == chosen)
            code = next(
                c for c in network["linecodes"] if c["name"] == line["linecode"]
            )
            # A finite extra ohm changes the physical source; convergence
            # alone must not count as detecting this deliberate error.
            code["r_series"][phase][phase] += 1
            negative, _ = typed(wrong, elements)
            control = compare(expected, negative)
            result.update(
                version=version,
                comparison=check,
                observed=actual,
                mna_residual=residual,
                negative_control=control,
                negative_line=chosen,
                negative_conductor=phase,
                facade_deserialization=decoded,
                passed=check["passed"] and not control["passed"],
                typed_sha256=hashlib.sha256(
                    json.dumps(networks[name], sort_keys=True).encode()
                ).hexdigest(),
            )
            if canonical is not None:
                written = canonical / name / "case.dss"
                observed = canonical_snapshot(written, elements)
                canonical_check = compare(expected, observed)
                result["canonical_comparison"] = canonical_check
                reimport = json.loads((canonical / name / "reimport.json").read_text())
                roundtrip = {
                    "value": {"type": "powerio.MulticonductorNetwork", "data": reimport}
                }
                reimported, residual = typed(roundtrip, elements)
                reimport_check = compare(expected, reimported)
                result.update(
                    canonical_comparison=canonical_check,
                    canonical_reimport_comparison=reimport_check,
                    canonical_sha256=hashlib.sha256(written.read_bytes()).hexdigest(),
                    canonical_reimport_residual=residual,
                    passed=result["passed"]
                    and canonical_check["passed"]
                    and reimport_check["passed"],
                )
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
            result["passed"] = False
        report["cases"][name] = result
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    passed = sum(v["passed"] for v in report["cases"].values())
    print(f"{passed}/{len(inputs)} staged typed circuits passed")
    return int(passed != len(inputs))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "evaluate"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--powerio", type=Path)
    parser.add_argument("--canonical", type=Path)
    args = parser.parse_args()
    if args.action == "evaluate" and args.powerio is None:
        parser.error("evaluate requires --powerio for facade IR validation")
    raise SystemExit(
        prepare(args.output)
        if args.action == "prepare"
        else evaluate(args.output, args.powerio, args.canonical)
    )
