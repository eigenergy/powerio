"""Qualify traditional delta flags and scalar load setters in both GLM forks."""

import argparse
import hashlib
import json
from pathlib import Path

from oracle import command, compare, read_glm, run_glm
from pair_load_oracle import deck as pair_deck
from pair_load_oracle import reference
from roundtrip_oracle import instrumentation


def variants():
    mixed = "constant_impedance_A 80+20j; constant_power_B 400+100j; constant_current_C 1+0.4j; constant_power_AN 100+20j; constant_impedance_AB 160+40j;"
    terms = [
        ("impedance", "AB", 80 + 20j),
        ("power", "BC", 400 + 100j),
        ("current", "CA", 1 + 0.4j),
        ("power", "AN", 100 + 20j),
        ("impedance", "AB", 160 + 40j),
    ]
    yield (
        "legacy_a_z",
        "ABCD",
        "constant_impedance_A 80+20j;",
        [("impedance", "AB", 80 + 20j)],
        False,
    )
    yield (
        "legacy_b_p",
        "ABCD",
        "constant_power_B 400+100j;",
        [("power", "BC", 400 + 100j)],
        False,
    )
    yield (
        "legacy_c_i",
        "ABCD",
        "constant_current_C 1+0.4j;",
        [("current", "CA", 1 + 0.4j)],
        False,
    )
    for pair, flag, prop in [
        ("AB", "ABD", "A"),
        ("BC", "BCD", "B"),
        ("CA", "ACD", "C"),
    ]:
        yield (
            "subset_" + pair.lower(),
            flag,
            f"constant_impedance_{prop} 80+20j;",
            [("impedance", pair, 80 + 20j)],
            False,
        )
    yield "mixed", "ABCD", mixed, terms, False
    yield "delta_root", "ABCD", mixed, terms, True
    aliases = "constant_power_A 10+20j; constant_power_A_real 0.25 kW; constant_power_A_reac 0.05 kVAr; constant_current_B_real -0.2; constant_current_B_reac -0.6; constant_impedance_C_real 100; constant_impedance_C_reac 20; constant_impedance_C 70-10j; constant_power_AN_real 100; constant_power_AN_reac 25;"
    reverse = "constant_power_A_real 1; constant_power_A_reac 2; constant_power_A 500+100j; constant_impedance_B 1+2j; constant_impedance_B_real 65; constant_impedance_B_reac 15; constant_current_CN 0+0j; constant_current_CN_real -0.3; constant_current_CN_reac 0.8;"
    for delta in (False, True):
        a, b, c = ("AB", "BC", "CA") if delta else ("A", "B", "C")
        flags = "ABCD" if delta else "ABCN"
        yield (
            "aliases_" + str(delta),
            flags,
            aliases,
            [
                ("power", a, 250 + 50j),
                ("current", b, -0.2 - 0.6j),
                ("impedance", c, 70 - 10j),
                ("power", "AN", 100 + 25j),
            ],
            False,
        )
        yield (
            "aliases_reverse_" + str(delta),
            flags,
            reverse,
            [
                ("power", a, 500 + 100j),
                ("impedance", b, 65 + 15j),
                ("current", "CN", -0.3 + 0.8j),
            ],
            False,
        )


def deck(spec, solver):
    _, flags, properties, _, root = spec
    text = pair_deck([], solver)
    if root:
        # Baseline wye demands remain explicit AN/BN/CN when their root load
        # changes to D; only the added legacy A/B/C channels become delta.
        for phase in "ABC":
            text = text.replace(
                f"constant_impedance_{phase} ", f"constant_impedance_{phase}N "
            )
        old = "name sink;  phases ABCN;"
        assert text.count(old) == 1
        text = text.replace(old, f"name sink; phases {flags}; {properties}")
        text = text.replace("name source; phases ABCN;", "name source; phases ABCD;")
    else:
        text += f"\nobject load {{ name demand; parent sink; phases {flags}; nominal_voltage 240; latitude 0; {properties} }}\n"
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--bridge", type=Path)
    parser.add_argument("--case", action="append", choices=[s[0] for s in variants()])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    report = {"scope": __doc__, "cases": {}}

    def solve(profile, directory, text, spec, edited=False):
        directory.mkdir()
        (directory / "model.glm").write_text(text)
        result = {
            "passed": False,
            "input_sha256": hashlib.sha256(text.encode()).hexdigest(),
        }
        try:
            result["version"] = run_glm(profile, directory, args.pnnl_root)
            actual = read_glm(directory, 3)
            expected, meta = reference(spec[3], edited)
            result["reference"] = meta
            result["comparison"] = compare(expected, actual)
            wrong = [
                (law, suffix[0] if suffix in ("AB", "BC", "CA") else suffix, v)
                for law, suffix, v in spec[3]
            ]
            if wrong == spec[3]:
                wrong = [
                    (law, {"A": "AB", "B": "BC", "C": "CA"}.get(suffix, suffix), v)
                    for law, suffix, v in spec[3]
                ]
            result["connection_negative_control"] = compare(
                reference(wrong, edited)[0], actual
            )
            result["passed"] = (
                result["comparison"]["passed"]
                and not result["connection_negative_control"]["passed"]
            )
            if any(law == "current" for law, _, _ in spec[3]):
                result["fixed_phasor_negative_control"] = compare(
                    reference(spec[3], edited, fixed_current=True)[0], actual
                )
                result["passed"] &= not result["fixed_phasor_negative_control"][
                    "passed"
                ]
        except Exception as error:
            result["error"] = str(error)
        return result

    def save(key, result):
        report["cases"][key] = result
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(key, "passed" if result["passed"] else "FAILED", flush=True)

    for profile in ("pnnl", "arras"):
        for solver in ("NR", "FBS"):
            for spec in variants():
                if args.case and spec[0] not in args.case:
                    continue
                key = f"{profile}/{solver}/{spec[0]}"
                directory = args.output / key
                directory.mkdir(parents=True)
                source = directory / "input.glm"
                source.write_text(deck(spec, solver))
                native = solve(profile, directory / "native", source.read_text(), spec)
                save(key + "/native", native)
                if args.bridge is None or not native["passed"]:
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
                assert (directory / "fresh.glm").read_bytes() == (
                    directory / "restored.glm"
                ).read_bytes()
                for variant in (
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ):
                    text = (
                        directory / (variant + ".glm")
                    ).read_text() + instrumentation("ABC")
                    result = solve(
                        profile, directory / variant, text, spec, variant == "edited"
                    )
                    result["diagnostics"] = decoded[variant]["diagnostics"]
                    save(key + "/" + variant, result)
    return 0 if all(v["passed"] for v in report["cases"].values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
