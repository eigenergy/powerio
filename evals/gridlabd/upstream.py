"""Fetch hash-pinned upstream regressions and run their original assertions.

These are native PNNL engine sanity checks, not PowerIO conversion evidence.
Their original controls, solver methods and assertion tolerances are retained.
"""

import argparse
import hashlib
import json
import os
import re
import urllib.request
from collections import Counter
from pathlib import Path

from oracle import HERE, LOCK, command


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--pnnl-root", type=Path, required=True)
    parser.add_argument(
        "--preprocessor",
        type=Path,
        help="optionally validate PowerIO-preprocessed GLM using the original assertions",
    )
    args = parser.parse_args()
    output, root = args.output.resolve(), args.pnnl_root.resolve()
    output.mkdir(parents=True, exist_ok=False)
    for item in LOCK["pnnl"]["files"]:
        if (
            hashlib.sha256((root / item["path"]).read_bytes()).hexdigest()
            != item["sha256"]
        ):
            raise ValueError(f"engine pin mismatch: {item['path']}")
    manifest = json.loads((HERE / "upstream.json").read_text())
    for item in manifest["files"]:
        url = f"https://raw.githubusercontent.com/gridlab-d/gridlab-d/{manifest['commit']}/{item['source']}"
        with urllib.request.urlopen(url, timeout=30) as response:
            data = response.read(100 * 1024 + 1)
        if hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError(f"upstream input hash mismatch: {url}")
        path = output / item["local"]
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    env = dict(os.environ, GLPATH=f"{root / 'lib'}:{root / 'share'}")
    exe = str(root / "bin/gridlabd")
    version = command([exe, "--version"], output, env)
    if LOCK["pnnl"]["version"] not in version:
        raise ValueError("engine version mismatch")
    report = {"scope": __doc__, "engine": version.strip(), "cases": {}}
    for item in manifest["files"]:
        if not item.get("assertions"):
            # Models without assertions may also lack a stop time. Acquire
            # them here; feeder_oracle instruments a bounded numerical check.
            continue
        path = output / item["local"]
        source = path.read_text()
        if source.count("object complex_assert") != item["assertions"]:
            raise ValueError("assertion inventory mismatch")
        if (
            dict(Counter(re.findall(r"within\s+([^;]+)", source)))
            != item["tolerances_V"]
        ):
            raise ValueError("assertion tolerance inventory mismatch")
        run_path = path
        if args.preprocessor:
            expanded = command(
                [str(args.preprocessor.resolve()), str(path), "pnnl", str(output)],
                path.parent,
            )
            run_path = path.with_name(path.stem + "_expanded.glm")
            run_path.write_text(expanded)
        command([exe, run_path.name], path.parent, env)
        report["cases"][path.name] = dict(
            item,
            passed=True,
            powerio_preprocessed=bool(args.preprocessor),
            executed_sha256=hashlib.sha256(run_path.read_bytes()).hexdigest(),
        )
        print(path.name, "PASS", item["assertions"], "original assertions")
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
