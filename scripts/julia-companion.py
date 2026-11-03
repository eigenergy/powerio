#!/usr/bin/env python3
"""Read an explicit, immutable Julia companion for PR CI; main always uses upstream."""
import json
import re
import sys
from pathlib import Path


def resolve(event, path):
    if event != "pull_request" or not path.exists():
        return {"enabled": "false"}
    data = json.loads(path.read_text())
    if set(data) != {"repository", "sha", "pull_request"}:
        raise ValueError("companion must declare repository, sha and pull_request")
    repo, sha, pr = (data[key] for key in ("repository", "sha", "pull_request"))
    if not isinstance(repo, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/PowerIO\.jl", repo):
        raise ValueError("companion repository must be a GitHub owner/PowerIO.jl")
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("companion sha must be a full lowercase commit ID, not a moving branch")
    if not isinstance(pr, str) or not re.fullmatch(r"https://github\.com/eigenergy/PowerIO\.jl/pull/[1-9][0-9]*", pr):
        raise ValueError("companion must link its upstream review PR")
    return {"enabled": "true", "repository": repo, "ref": sha}


if __name__ == "__main__":
    for key, value in resolve(sys.argv[1], Path(sys.argv[2])).items():
        print(f"{key}={value}")
