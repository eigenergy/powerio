"""Pinned FBS feeder source checks and compatibility entry point.

The original physical-pi audit is superseded by native-equation comparison.
"""

import hashlib
import json

from oracle import HERE


def pinned(path, family):
    data = path.read_bytes()
    catalog = json.loads((HERE / "ieee-corpus.json").read_text())
    source = next(
        s
        for f in catalog["families"]
        if f["id"] == family
        for s in f["sources"]
        if s["repository"].endswith("/gridlab-d/gridlab-d")
    )
    entry = next(f for f in source["files"] if f["path"].endswith(path.name))
    digest = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    if digest != entry["git_blob_sha1"] or len(data) != entry["bytes"]:
        raise ValueError("source blob pin mismatch")
    return data, {
        "repository": source["repository"],
        "commit": source["commit"],
        **entry,
        "sha256": hashlib.sha256(data).hexdigest(),
    }


if __name__ == "__main__":
    from ieee_fbs_delta_import_oracle import main

    raise SystemExit(main())
