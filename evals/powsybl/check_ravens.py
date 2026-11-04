#!/usr/bin/env python3
"""Compare direct and RAVENS-mediated imports with native PowSybl tables."""

import argparse
import json
import subprocess
import tempfile
from pathlib import Path

import pandas as pd
import pypowsybl as pp
from check_outputs import check_powsybl_version, load_checked

ROOT = Path(__file__).resolve().parents[2]
FORMATS = (
    ("psse", "raw"),
    ("psse35", "raw"),
    ("psse-rawx", "rawx"),
    ("xiidm", "xiidm"),
    ("jiidm", "jiidm"),
)
TABLES = (
    "buses",
    "voltage_levels",
    "loads",
    "generators",
    "lines",
    "2_windings_transformers",
    "shunt_compensators",
    "ratio_tap_changers",
    "phase_tap_changers",
    "operational_limits",
)


def run(*args: str | Path) -> list[dict]:
    result = subprocess.run(
        [str(arg) for arg in args] + ["--diagnostics-format", "json"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, args))}\n{result.stderr}")
    return json.loads(result.stderr)


def data(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))["value"]["data"]


def assert_diagnosed_losses(
    original: dict, restored: dict, diagnostics: list[dict]
) -> None:
    expected_voltage_losses = sum(
        abs(gen["vg"] - 1.0) > 1e-12 for gen in original["generators"]
    )
    expected_rating_losses = sum(
        branch["rate_c"] != 0.0 for branch in original["branches"]
    )
    messages = [
        entry["message"]
        for entry in diagnostics
        if entry["code"] == "EMIT.RAVENS.FIELD_DROPPED"
    ]
    if (
        sum("voltage setpoint vg=" in message for message in messages)
        != expected_voltage_losses
    ):
        raise AssertionError("generator voltage-setpoint losses must each be diagnosed")
    if (
        sum("rate_c has no MG-RAVENS" in message for message in messages)
        != expected_rating_losses
    ):
        raise AssertionError("third-rating losses must each be diagnosed")
    # Pin the currently documented profile recovery, rather than accepting
    # any value in the two fields excluded from direct-native equality.
    if any(gen["vg"] != 1.0 for gen in restored["generators"]):
        raise AssertionError("review the declared RAVENS generator setpoint recovery")
    if any(branch["rate_c"] != 0.0 for branch in restored["branches"]):
        raise AssertionError("review the declared RAVENS third-rating recovery")


def compare(direct, through_ravens, label: str) -> int:
    cells = 0
    for table in TABLES:
        expected = getattr(direct, f"get_{table}")(all_attributes=True).sort_index()
        actual = getattr(through_ravens, f"get_{table}")(
            all_attributes=True
        ).sort_index()
        if table == "generators":
            # The balanced converter profile has no generator setpoint slot.
            # Its reader defaults to 1 pu; assert native target volts, rather
            # than simply omitting this column from the comparison.
            levels = direct.get_voltage_levels(all_attributes=True)
            expected["target_v"] = expected["voltage_level_id"].map(levels["nominal_v"])
        elif table == "operational_limits":
            # XIIDM/JIIDM carry all three ratings. The RAVENS converter profile
            # carries A/B; every missing C rating was checked above by code.
            expected = expected.loc[expected["name"] != "rate_c"]
        try:
            pd.testing.assert_frame_equal(
                expected,
                actual,
                check_exact=False,
                atol=1e-8,
                rtol=1e-9,
            )
        except AssertionError as error:
            raise AssertionError(f"{label}: native {table}\n{error}") from error
        cells += expected.size
    return cells


def check_fixed_q_generator(powerio: Path, scratch: Path) -> None:
    """A fixed-Q generator must not become a voltage controller via RAVENS."""
    source = scratch / "fixed-q.m"
    source.write_text(
        """function mpc = fixed_q
mpc.version = '2';
mpc.baseMVA = 100;
mpc.bus = [
1 1 0 0 0 0 1 1 0 345 1 1.1 0.9;
2 3 90 30 0 0 1 1 0 345 1 1.1 0.9;
];
mpc.gen = [
1 30 5 300 -300 1 100 1 250 10;
2 60 25 300 -300 1 100 1 250 10;
];
mpc.branch = [1 2 0 0.0576 0 250 250 0 0 0 1 -360 360];
""",
        encoding="utf-8",
    )
    original_ir = scratch / "fixed-q.pio.json"
    run(powerio, "serialize", source, "-o", original_ir)
    document = json.loads(original_ir.read_text(encoding="utf-8"))
    document["value"]["data"]["generators"][0]["voltage_regulation_on"] = False
    original_ir.write_text(json.dumps(document), encoding="utf-8")
    ravens = scratch / "fixed-q.ravens.json"
    restored_ir = scratch / "fixed-q-restored.pio.json"
    run(powerio, "convert", original_ir, "--to", "ravens-json", "-o", ravens)
    run(powerio, "serialize", ravens, "-o", restored_ir)
    if data(restored_ir)["generators"][0]["voltage_regulation_on"]:
        raise AssertionError("RAVENS silently enabled a fixed-Q generator's regulator")
    networks = []
    for name, ir in (("direct", original_ir), ("mediated", restored_ir)):
        output = scratch / f"fixed-q-{name}.xiidm"
        run(powerio, "convert", ir, "--to", "xiidm", "-o", output)
        networks.append(load_checked(output, f"fixed-Q generator {name}"))
    compare(*networks, "fixed-Q generator before load flow")
    for network in networks:
        results = pp.loadflow.run_ac(network)
        if not results or any(result.status.name != "CONVERGED" for result in results):
            raise AssertionError(
                "fixed-Q generator native AC load flow did not converge"
            )
    for table, columns in (("buses", ["v_mag", "v_angle"]), ("generators", ["p", "q"])):
        expected = getattr(networks[0], f"get_{table}")()[columns].sort_index()
        actual = getattr(networks[1], f"get_{table}")()[columns].sort_index()
        pd.testing.assert_frame_equal(
            expected, actual, check_exact=False, atol=1e-7, rtol=1e-9
        )
    print(
        "Fixed-Q generator: regulation state and converged native AC solution preserved"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("powerio", type=Path)
    args = parser.parse_args()
    check_powsybl_version()
    checks = 0
    cells = 0
    with tempfile.TemporaryDirectory(prefix="powerio-ravens-powsybl-") as directory:
        scratch = Path(directory)
        check_fixed_q_generator(args.powerio, scratch)
        for count in (9, 14, 30, 57, 118):
            stem = f"case{count}"
            original_ir = scratch / f"{stem}.pio.json"
            ravens = scratch / f"{stem}.ravens.json"
            restored_ir = scratch / f"{stem}-ravens.pio.json"
            run(
                args.powerio,
                "serialize",
                ROOT / f"tests/data/{stem}.m",
                "-o",
                original_ir,
            )
            diagnostics = run(
                args.powerio,
                "convert",
                original_ir,
                "--to",
                "ravens-json",
                "-o",
                ravens,
            )
            run(args.powerio, "serialize", ravens, "-o", restored_ir)
            assert_diagnosed_losses(data(original_ir), data(restored_ir), diagnostics)
            for target, extension in FORMATS:
                direct = scratch / f"{stem}-{target}-direct.{extension}"
                mediated = scratch / f"{stem}-{target}-ravens.{extension}"
                run(args.powerio, "convert", original_ir, "--to", target, "-o", direct)
                run(
                    args.powerio, "convert", restored_ir, "--to", target, "-o", mediated
                )
                label = f"{stem} via RAVENS and IR -> {target}"
                direct_network = load_checked(direct, f"{label} direct control")
                mediated_network = load_checked(mediated, label)
                if len(direct_network.get_buses()) != count:
                    raise AssertionError(f"{label}: expected {count} source buses")
                compared = compare(direct_network, mediated_network, label)
                cells += compared
                checks += 1
                print(f"{label}: PASS, {compared} native table cells")
    print(f"RAVENS native interoperability: {checks} routes, {cells} table cells")


if __name__ == "__main__":
    main()
