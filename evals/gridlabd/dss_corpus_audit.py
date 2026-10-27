"""Audit all detected circuit entries in an externally pinned DSS acquisition.

Reports parsing, IR equality, readiness and canonical target availability.
These checks are not native electrical-result qualification.
"""

import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

COUNTS = ("buses", "lines", "switches", "transformers", "loads", "shunts", "generators", "sources")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-root", type=Path, required=True, help="directory containing manifest.json and repository subdirectories")
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = args.source_root / "manifest.json"
    records = json.loads(manifest.read_text())
    catalog = json.loads((Path(__file__).parent / "ieee-corpus.json").read_text())
    pins = {(source["repository"].removeprefix("https://github.com/").removesuffix(".git"), source["commit"])
            for family in catalog["families"] for source in family.get("sources", [])}
    # Verify every acquired companion, not just each selected circuit entry.
    for item in records:
        if (item["repository"], item["commit"]) not in pins:
            raise ValueError(f"repository pin absent from catalog: {item['repository']}")
        path = args.source_root / item["repository"] / item["path"]
        data = path.read_bytes()
        if (len(data) != item["bytes"] or hashlib.sha256(data).hexdigest() != item["sha256"]
                or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest() != item["git_blob_sha1"]):
            raise ValueError(f"source pin mismatch: {path}")
    report = dict(scope=__doc__, manifest_sha256=hashlib.sha256(manifest.read_bytes()).hexdigest(),
                  bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
                  verified_files=len(records), cases=[])
    for item in records:
        path = args.source_root / item["repository"] / item["path"]
        if path.suffix.lower() not in (".dss", ".txt"):
            continue
        text = path.read_text(encoding="utf-8-sig", errors="replace")
        clean = re.sub(r"(?m)//[^\n]*|![^\n]*", "", text)
        if (not re.search(r'(?im)^\s*new\s+(?:object=)?"?circuit\.', clean)
                and path.name.lower() not in ("nevmaster.dss", "run_nev.dss")):
            continue
        folder = args.output / str(len(report["cases"]))
        folder.mkdir()
        row = dict(source=item, evidence=str(folder.resolve()))
        with (folder / "bridge.json").open("w") as stdout, (folder / "stderr.txt").open("w") as stderr:
            try:
                process = subprocess.run([str(args.bridge.resolve()), str(path.resolve()), str(folder.resolve()),
                                          str((args.source_root / item["repository"]).resolve())],
                                         stdout=stdout, stderr=stderr, timeout=120, check=False)
                row["exit"] = process.returncode
            except subprocess.TimeoutExpired:
                row["exit"] = None
                row["error"] = "bridge exceeded 120 seconds; subprocess terminated"
        if row["exit"] == 0:
            bridge = json.loads((folder / "bridge.json").read_text())
            row.update(ir_network_equal=bridge["ir_network_equal"], targets=bridge["targets"],
                       readiness=bridge["readiness"], diagnostics=bridge["diagnostics"],
                       counts={key: len(bridge["network"].get(key, [])) for key in COUNTS})
        elif row["exit"] is not None:
            row["error"] = (folder / "stderr.txt").read_text()
        report["cases"].append(row)
        report["summary"] = dict(entries=len(report["cases"]),
                                 parsed_ir_equal=sum(bool(c.get("ir_network_equal")) for c in report["cases"]),
                                 dss_writable=sum(c.get("targets", {}).get("dss", {}).get("ok", False) for c in report["cases"]),
                                 glm_writable=sum(c.get("targets", {}).get("gridlabd", {}).get("ok", False) for c in report["cases"]))
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(item["repository"], item["path"], row["exit"], flush=True)
    print(report["summary"], flush=True)


if __name__ == "__main__":
    main()
