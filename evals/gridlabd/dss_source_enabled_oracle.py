"""Qualify inactive sources through analytic, typed and native port equations.

Two unequal parallel sources feed a line and impedance load. Ordered properties,
Enable/Disable and Like select the active sources. Disabled sources contribute
no admittance or EMF; every inactive record remains present in typed metadata.
"""

import argparse
import cmath
import hashlib
import json
from pathlib import Path

import opendssdirect as dss
from dss_source_lowering_oracle import canonical_snapshot, native, typed
from dss_source_oracle import complex_array
from oracle import LOCK, add, command, compare

MAIN = (240, 0, 2, 1)
AUX = (210, 17, 3, 0.5)


def profiles():
    # name, final commands, enabled states, whether aux copies main's physics
    yield "both", "", (True, True), False
    yield "wildcard_before_new", "", (False, True), False
    for token, state in [
        ("no", False),
        ("false", False),
        ("1", False),
        ("0", False),
        ("on", False),
        ("yep", True),
        ("T", True),
    ]:
        yield (
            "property_" + token,
            f"Edit Vsource.aux enabled={token}",
            (True, state),
            False,
        )
    for name, script, flags, copied in [
        ("disable", "Disable Vsource.aux", (True, False), False),
        ("enable", "Disable Vsource.aux\nEnable Vsource.aux", (True, True), False),
        ("abbreviations", "dis Vsource.aux\nena Vsource.aux", (True, True), False),
        (
            "property_after_command",
            "Disable Vsource.aux\nEdit Vsource.aux enabled=yes",
            (True, True),
            False,
        ),
        (
            "command_after_property",
            "Edit Vsource.aux enabled=yes\nDisable Vsource.aux",
            (True, False),
            False,
        ),
        ("wildcard_off", "Disable Vsource.*", (False, False), False),
        ("wildcard_on", "Disable Vsource.*\nEnable Vsource.*", (True, True), False),
        (
            "wildcard_single",
            "Disable Vsource.*\nEnable Vsource.aux",
            (False, True),
            False,
        ),
        (
            "self_like",
            "Disable Vsource.aux\nEdit Vsource.aux like=aux",
            (True, True),
            False,
        ),
        (
            "disabled_template",
            "Disable Vsource.source\nEdit Vsource.aux like=source",
            (False, True),
            True,
        ),
        (
            "disabled_target",
            "Disable Vsource.aux\nEdit Vsource.aux like=source",
            (True, True),
            True,
        ),
        (
            "like_then_off",
            "Edit Vsource.aux like=source enabled=no",
            (True, False),
            True,
        ),
        ("no_class_noop", "Disable aux", (True, True), False),
        ("object_prefix", "Disable o=Vsource.aux", (True, False), False),
        ("ignored_trailing", "Disable Vsource.aux enabled=true", (True, False), False),
        (
            "wildcard_selection",
            "Disable Vsource.*\nEnable Vsource.*\n~ enabled=no",
            (True, False),
            False,
        ),
        (
            "explicit_selection",
            "Disable Vsource.source\n~ enabled=yes",
            (True, True),
            False,
        ),
    ]:
        yield name, script, flags, copied


def prepare(output):
    output.mkdir(parents=True, exist_ok=False)
    manifest, descriptions = {}, {}
    for frequency in (50, 60):
        for name, commands, flags, copied in profiles():
            key = f"{frequency}/{name}"
            path = output / key / "input.dss"
            path.parent.mkdir(parents=True)
            lines = ["Clear", f"Set defaultbasefrequency={frequency}"]
            for cls, identifier, spec in [
                ("Circuit", "c", MAIN),
                ("Vsource", "aux", AUX),
            ]:
                if name == "wildcard_before_new" and identifier == "aux":
                    lines.append("Disable Vsource.*")
                magnitude, angle, r, x = spec
                lines.append(
                    f"New {cls}.{identifier} phases=1 bus1=feed.1 bus2=feed.0 basekv={magnitude / 1000} angle={angle} z1=[{r} {x}]"
                )
            # Keep aux active before commands, so selection side effects have witnesses.
            lines += [
                commands,
                "New Line.wire phases=1 bus1=feed.1 bus2=sink.1 r1=0.3 x1=0.2 r0=0.3 x0=0.2 c1=0 c0=0 length=1 units=none",
                "New Load.load phases=1 bus1=sink.1.0 conn=wye kv=0.24 kw=1.2 kvar=0.3 model=2 vminpu=0 vmaxpu=2",
                "Set controlmode=off tolerance=1e-12 maxiterations=100",
                "Solve",
            ]
            path.write_text("\n".join(lines) + "\n")
            manifest[key] = str(path.resolve())
            descriptions[key] = {
                "states": dict(zip(("source", "aux"), flags, strict=True)),
                "specs": {"source": MAIN, "aux": MAIN if copied else AUX},
            }
    (output / "inputs.json").write_text(json.dumps(manifest, indent=2) + "\n")
    (output / "profiles.json").write_text(json.dumps(descriptions, indent=2) + "\n")
    print(f"Prepared {len(manifest)} circuits")


def analytic(profile):
    zload, zwire = 240**2 / complex(1200, -300), complex(0.3, 0.2)
    admittance, injection = 1 / (zload + zwire), 0j
    active = {}
    for name, enabled in profile["states"].items():
        magnitude, angle, r, x = profile["specs"][name]
        e, z = cmath.rect(magnitude, angle * cmath.pi / 180), complex(r, x)
        if enabled:
            active[name] = (e, z)
            admittance += 1 / z
            injection += e / z
    feed = injection / admittance
    current = feed / (zwire + zload)
    sink = current * zload
    result = {}
    ports = {"line.wire": ([feed, sink], [current, -current])}
    for name, (e, z) in active.items():
        amp = (feed - e) / z
        ports["vsource." + name] = ([feed, 0j], [amp, -amp])
    for name, (volts, amps) in ports.items():
        for i, (v, a) in enumerate(zip(volts, amps, strict=True)):
            add(result, f"{name}.{i}.voltage", "V", v)
            add(result, f"{name}.{i}.current", "A", a)
            add(result, f"{name}.{i}.power", "VA", v * a.conjugate())
    return result


def admittance_check(engine, profile):
    # The pinned C-API crashes if Currents is requested from a source that was
    # disabled before its first solve. Check the assembled circuit matrix and
    # energized ports instead of invoking that unsafe inactive-object getter.
    names = [s.lower() for s in engine.Circuit.YNodeOrder()]
    assert set(names) == {"feed.1", "sink.1"}
    actual = complex_array(engine.Circuit.SystemY()).reshape(2, 2)
    series = 1 / complex(0.3, 0.2)
    source = sum(
        1 / complex(*profile["specs"][name][2:])
        for name, on in profile["states"].items()
        if on
    )
    expected = {
        ("feed.1", "feed.1"): series + source,
        ("sink.1", "sink.1"): series + complex(1200, -300) / 240**2,
        ("feed.1", "sink.1"): -series,
        ("sink.1", "feed.1"): -series,
    }
    ref, measured = {}, {}
    for i, start in enumerate(names):
        for j, end in enumerate(names):
            key = f"{start}/{end}"
            add(ref, key, "S", expected[start, end])
            add(measured, key, "S", actual[i, j])
    return compare(ref, measured)


def evaluate(output, canonical, powerio):
    inputs = json.loads((output / "inputs.json").read_text())
    profiles = json.loads((output / "profiles.json").read_text())
    networks = json.loads((output / "networks.json").read_text())
    report = {"scope": __doc__, "cases": {}}
    for key, path in inputs.items():
        result = {
            "passed": False,
            "input_sha256": hashlib.sha256(Path(path).read_bytes()).hexdigest(),
        }
        try:
            expected, elements, version = native(Path(path))
            engine = dss.NewContext()
            assert LOCK["dss"]["version"] in engine.Basic.Version()
            engine.Basic.AllowChangeDir(False)
            engine.Text.Command(f'redirect "{path}"')
            observed_states = {}
            for name in profiles[key]["states"]:
                engine.Circuit.SetActiveElement("vsource." + name)
                enabled = engine.CktElement.Enabled()
                observed_states[name] = enabled
            assert observed_states == profiles[key]["states"]
            document = {
                "schema": "pio-ir",
                "version": 2,
                "producer": {"name": "powerio-oracle", "version": "1"},
                "value": {
                    "type": "powerio.MulticonductorNetwork",
                    "data": networks[key],
                },
            }
            encoded = output / key / "typed.pio.json"
            encoded.write_text(json.dumps(document))
            decoded = json.loads(
                command(
                    [str(powerio.resolve()), "summary", str(encoded.resolve())],
                    output / key,
                )
            )
            assert decoded["model"] == "multiconductor"
            inactive = {
                u["name"] for u in networks[key]["untyped"] if u["class"] == "vsource"
            }
            assert inactive == {n for n, on in observed_states.items() if not on}
            actual, residual = typed(document, elements)
            checks = {
                "analytic": compare(analytic(profiles[key]), expected),
                "typed": compare(expected, actual),
                "assembled_admittance": admittance_check(engine, profiles[key]),
            }
            for stage, filename in [
                ("canonical", "case.dss"),
                ("second", "case.reimport.dss"),
            ]:
                checks[stage] = compare(
                    expected, canonical_snapshot(canonical / key / filename, elements)
                )
            reimport = json.loads((canonical / key / "reimport.json").read_text())
            reimport_doc = dict(
                document,
                value={"type": "powerio.MulticonductorNetwork", "data": reimport},
            )
            restored, restored_residual = typed(reimport_doc, elements)
            checks["typed_reimport"] = compare(expected, restored)
            # Flip one source while keeping the control circuit energized.
            # Native convergence reporting fails on an energized-to-all-off
            # transition, so that transition is not a measured control.
            control_source = next(
                (name for name, on in observed_states.items() if not on), "aux"
            )
            engine.Text.Command(
                ("Disable" if observed_states[control_source] else "Enable")
                + " Vsource."
                + control_source
            )
            engine.Text.Command("Solve")
            assert engine.Solution.Converged()
            engine.Circuit.SetActiveElement("Line.wire")
            v = engine.CktElement.Voltages()
            negative = {"feed": {"unit": "V", "value": v[:2]}}
            control = compare({"feed": expected["line.wire.0.voltage"]}, negative)
            assert not control["passed"]
            result.update(
                version=version,
                states=observed_states,
                checks=checks,
                negative_control=control,
                control_source=control_source,
                mna_residual=residual,
                reimport_residual=restored_residual,
                facade_deserialization=decoded,
                passed=all(c["passed"] for c in checks.values()),
            )
        except Exception as error:
            result["error"] = f"{type(error).__name__}: {error}"
        report["cases"][key] = result
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    passed = sum(v["passed"] for v in report["cases"].values())
    print(f"{passed}/{len(inputs)} source-state circuits passed")
    return int(passed != len(inputs))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "evaluate"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--canonical", type=Path)
    parser.add_argument("--powerio", type=Path)
    args = parser.parse_args()
    if args.action == "evaluate" and (args.canonical is None or args.powerio is None):
        parser.error("evaluate requires --canonical and --powerio")
    raise SystemExit(
        prepare(args.output)
        if args.action == "prepare"
        else evaluate(args.output, args.canonical, args.powerio)
    )
