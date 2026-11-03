"""Qualify source-free parent flattening and explicit Arras NR refusal.

Consumes a parent_oracle.py report and rechecks its native electrical records.
Every accepted case emits fresh, IR-restored, edited, rebased and grounded-
reference output. Byte-identical canonical inputs share one native execution
per pinned fork; each case still receives its own reference comparison.
"""

import argparse
import hashlib
import json
from pathlib import Path

from oracle import command, compare, read_glm, run_glm
from pair_load_oracle import reference
from parent_oracle import deck, full_terms, omitted_terms, variants
from roundtrip_oracle import instrumentation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--native-report", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument("--case", action="append", choices=[v[0] for v in variants()])
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    native = json.loads(args.native_report.read_text())
    report = {
        "scope": __doc__,
        "native_report_sha256": hashlib.sha256(
            args.native_report.read_bytes()
        ).hexdigest(),
        "cases": {},
        "engine_runs": {},
    }
    cache = {}
    for profile in ("pnnl", "arras"):
        for solver in ("NR", "FBS"):
            for spec in variants():
                if args.case and spec[0] not in args.case:
                    continue
                key = f"{profile}/{solver}/{spec[0]}"
                baseline = native["cases"][key]
                source = args.native_report.parent / key / "model.glm"
                assert source.read_text() == deck(spec, solver)
                assert (
                    hashlib.sha256(source.read_bytes()).hexdigest()
                    == baseline["input_sha256"]
                )
                refused = profile == "arras" and solver == "NR" and spec[1] > 1
                directory = args.output / key
                directory.mkdir(parents=True)
                if not refused:
                    comparison = compare(
                        reference(full_terms(spec))[0], read_glm(source.parent, 3)
                    )
                    assert (
                        comparison["passed"]
                        and baseline["classification"] == "included"
                    ), key
                else:
                    comparison = None
                    assert baseline["classification"] in (
                        "engine_failure",
                        "omitted",
                    ), key
                    if baseline["classification"] == "omitted":
                        actual = read_glm(source.parent, 3)
                        assert compare(reference(omitted_terms(spec))[0], actual)[
                            "passed"
                        ], key
                        assert not compare(reference(full_terms(spec))[0], actual)[
                            "passed"
                        ], key
                    else:
                        # An installation/version failure is not evidence of
                        # the calibrated native parent-hierarchy crash.
                        assert (
                            "grandchildren objects are not supported"
                            in baseline["error"]
                        ), key
                        assert "Segmentation fault" in baseline["error"], key
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
                except RuntimeError as error:
                    if not refused or "nested electrical parents" not in str(error):
                        raise
                    assert not list(directory.glob("*.glm")) and not list(
                        directory.glob("*.pio.json")
                    ), key
                    report["cases"][key] = {
                        "reader_refused": True,
                        "native_classification": baseline["classification"],
                        "error": str(error),
                    }
                    continue
                assert not refused, key
                assert (directory / "fresh.glm").read_bytes() == (
                    directory / "restored.glm"
                ).read_bytes()
                item = {
                    "reader_refused": False,
                    "native_comparison": comparison,
                    "canonical": {},
                }
                for variant in (
                    "fresh",
                    "restored",
                    "edited",
                    "rebased",
                    "grounded_reference",
                ):
                    data = (
                        directory / (variant + ".glm")
                    ).read_text() + instrumentation("ABC")
                    digest = hashlib.sha256(data.encode()).hexdigest()
                    cache_key = profile + "/" + digest
                    if cache_key not in cache:
                        path = args.output / "engine_runs" / cache_key
                        path.mkdir(parents=True)
                        (path / "model.glm").write_text(data)
                        version = run_glm(profile, path, args.pnnl_root)
                        cache[cache_key] = read_glm(path, 3)
                        report["engine_runs"][cache_key] = {
                            "version": version,
                            "input_sha256": digest,
                            "directory": str(path.resolve()),
                        }
                    expected, kcl = reference(full_terms(spec), variant == "edited")
                    comparison = compare(expected, cache[cache_key])
                    omitted = compare(
                        reference(omitted_terms(spec), variant == "edited")[0],
                        cache[cache_key],
                    )
                    assert comparison["passed"] and not omitted["passed"], (
                        key,
                        variant,
                        comparison,
                    )
                    item["canonical"][variant] = {
                        "passed": True,
                        "engine_run": cache_key,
                        "comparison": comparison,
                        "omitted_load_negative_control": omitted,
                        "reference": kcl,
                        "diagnostics": decoded[variant]["diagnostics"],
                    }
                report["cases"][key] = item
                (args.output / "report.json").write_text(
                    json.dumps(report, indent=2) + "\n"
                )
                print(key, "five canonical variants passed", flush=True)
    assert len(report["cases"]) == 4 * sum(
        not args.case or v[0] in args.case for v in variants()
    )
    report["passed"] = True
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print("native unique canonical executions", len(cache), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
