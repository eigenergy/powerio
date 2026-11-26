"""Fetch and verify reference files that are stored outside this repository."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

from solvers import HERE, ROOT


def download(url, expected=None):
    data = subprocess.run(["curl", "-fsSL", "--retry", "2", "--max-time", "45",
                           "--max-filesize", "10485760", url],
                          capture_output=True, check=True).stdout
    if len(data) > 10 * 1024 * 1024 or (expected and hashlib.sha256(data).hexdigest() != expected):
        raise ValueError(f"Reference file at {url} does not match its SHA-256 hash")
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target/ac-power-flow-corpus")
    parser.add_argument("--powersystems-project", type=Path,
                        help="Julia project with PowerSystems 5.12.5 and PowerFlows 0.25.5")
    args = parser.parse_args()
    corpus = args.output.resolve()
    corpus.mkdir(parents=True, exist_ok=True)
    cases = json.loads((HERE / "corpus.json").read_text())
    for case in cases + [case["companion"] for case in cases if "companion" in case]:
        if "url" in case:
            path = corpus / case["external"]
            if path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == case["sha256"]:
                continue
            try:
                data = download(case["url"], case.get("download_sha256"))
            except subprocess.CalledProcessError as error:
                if path.exists():
                    raise
                print(f"Cannot download {path.name}: {error.stderr.decode().strip()}", file=sys.stderr)
                continue
            if case.get("compression") == "zstd":
                data = subprocess.run(["zstd", "-dc"], input=data, capture_output=True, check=True).stdout
            if case.get("extract") == "activsg200":
                # The portal presents the public EPC text inside an HTML page.
                marker = b"!\ncomments\n!\nsolution parameters"
                data = data[data.index(marker):].split(b"</pre>", 1)[0]
            if hashlib.sha256(data).hexdigest() != case["sha256"]:
                raise ValueError(f"Unexpected SHA-256 after decompressing {case['external']}")
            path.write_bytes(data)
        for file in case.get("downloads", []):
            path = corpus / case["external"] / file["name"]
            if path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == file["sha256"]:
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(download(file["url"], file["sha256"]))
    if not (corpus / "pm-case30.json").exists():
        subprocess.run(["julia", "--startup-file=no", f"--project={ROOT / 'evals/validation'}",
                        ROOT / "evals/validation/pm_export.jl", ROOT / "tests/data/case30.m",
                        corpus / "pm-case30.json"], check=True)
    if args.powersystems_project and not (corpus / "powersystems-case9.json").exists():
        subprocess.run(["julia", "--startup-file=no", f"--project={args.powersystems_project.resolve()}",
                        "-e", "using PowerSystems; to_json(System(ARGS[1]; runchecks=false), ARGS[2]; force=true)",
                        ROOT / "tests/data/case9.m", corpus / "powersystems-case9.json"], check=True)


if __name__ == "__main__":
    main()
