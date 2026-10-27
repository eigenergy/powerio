"""Pinned CIMHub balanced/unbalanced IEEE9500 DSS static import comparisons.

The two archive cases remain distinct from the electricdss-tst IEEE9500 case.
ControlMode=Off is inserted before voltage-base calculations and solves. The
original archive members and adjusted profiles are retained with hashes.
"""

import argparse
import hashlib
import io
import json
import re
import zipfile
from pathlib import Path, PurePosixPath

from ieee4_import_oracle import evaluate
from oracle import HERE


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archives", type=Path, required=True)
    parser.add_argument("--bridge", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    family = next(f for f in catalog["families"] if f["id"] == "ieee9500")
    pin = next(s for s in family["sources"] if s["repository"].endswith("/CIMHub"))
    report = dict(
        scope=__doc__,
        source_commit=pin["commit"],
        repository=pin["repository"],
        bridge_sha256=hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
        cases={},
    )
    for variant in ("bal", "unbal"):
        prefix = f"ieee9500{variant}"
        record = next(
            f for f in pin["files"] if f["path"].endswith(prefix + "_dss.zip")
        )
        data = (args.archives / Path(record["path"]).name).read_bytes()
        if (
            len(data) != record["bytes"]
            or hashlib.sha1(f"blob {len(data)}\0".encode() + data).hexdigest()
            != record["git_blob_sha1"]
        ):
            raise ValueError("archive does not match pinned source")
        folder = args.output / variant
        folder.mkdir()
        hashes, patches = {}, []
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            if sum(info.file_size for info in archive.infolist()) > 50_000_000:
                raise ValueError("archive exceeds extraction limit")
            for info in archive.infolist():
                relative = PurePosixPath(info.filename)
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError("unsafe archive member")
                if info.is_dir():
                    continue
                content = archive.read(info)
                original, fixed = (
                    folder / kind / relative for kind in ("original", "fixed")
                )
                original.parent.mkdir(parents=True, exist_ok=True)
                fixed.parent.mkdir(parents=True, exist_ok=True)
                original.write_bytes(content)
                adjusted = content
                if relative.name == prefix + "_base.dss":
                    lines = []
                    for number, line in enumerate(
                        content.decode("utf-8-sig").splitlines(keepends=True), 1
                    ):
                        code = re.split(r"!|//", line)[0].strip()
                        if re.match(
                            r"(?i)^(solve|calcv|calcvoltagebases)(?:\s|$)", code
                        ):
                            lines.append("Set ControlMode=Off\n")
                            patches.append(
                                dict(
                                    before_line=number,
                                    original_command=code,
                                    insertion="Set ControlMode=Off",
                                )
                            )
                        lines.append(line)
                    adjusted = "".join(lines).encode()
                fixed.write_bytes(adjusted)
                hashes[str(relative)] = dict(
                    bytes=len(content),
                    original_sha256=hashlib.sha256(content).hexdigest(),
                    profile_sha256=hashlib.sha256(adjusted).hexdigest(),
                )
        if not patches:
            raise ValueError("archive master has no recognized calculation")
        row = evaluate(
            (folder / "fixed/dss" / (prefix + "_base.dss")).resolve(),
            args.bridge.resolve(),
            folder.resolve(),
            1e-11,
            (folder / "fixed").resolve(),
        )
        row.update(archive=record, files=hashes, profile_patches=patches)
        report["cases"][variant] = row
        print(variant, "PASS" if row["passed"] else "FAIL", row["stage"], flush=True)
        (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    report["passed"] = all(row["passed"] for row in report["cases"].values())
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
