#!/usr/bin/env python3
"""Generate the frozen producer receipt at evals/integration/powerio-candidate.json.

The receipt names the exact tested PowerIO subject commit and the observed
PowerIO.jl release-pair commit, the version identities that apply at that
commit, the supported wasm32 surface, digests of the generated public schemas
and the value kind inventory, the browser-relevant semantics a consumer must
honor, and the public boundary changes since the previous receipt. A tracked
receipt names the tested subject commit, never the commit that carries the
receipt file.

Usage:
    python3 scripts/make-producer-receipt.py --julia-commit <sha> \
        [--subject-commit <sha>] [--out evals/integration/powerio-candidate.json]
"""

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.strip()


def workspace_version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    match = re.search(r'^version = "([^"]+)"', text, re.M)
    assert match, "workspace version not found"
    return match.group(1)


def abi_version() -> int:
    text = (ROOT / "powerio-capi/src/lib.rs").read_text()
    match = re.search(r"pub const PIO_ABI_VERSION: u32 = (\d+);", text)
    assert match, "PIO_ABI_VERSION not found"
    return int(match.group(1))


def value_kinds() -> list[str]:
    text = (ROOT / "powerio/src/value.rs").read_text()
    kinds = re.findall(r'ValueKind::[A-Za-z]+ => "([a-z0-9_]+)"', text)
    assert kinds, "value kind inventory not found in powerio/src/value.rs"
    return sorted(set(kinds))


def schema_digests() -> dict[str, str]:
    out: dict[str, str] = {}
    for path in sorted((ROOT / "docs/schema").rglob("*.json")):
        out[str(path.relative_to(ROOT))] = sha256(path)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--julia-commit", required=True,
                    help="the observed PowerIO.jl release-pair commit")
    ap.add_argument("--subject-commit", default=None,
                    help="the tested PowerIO commit (default: HEAD)")
    ap.add_argument("--out", default="evals/integration/powerio-candidate.json")
    ap.add_argument("--boundary-doc-digest", default=None,
                    help="SHA-256 of tellegen docs/POWERIO_INTEGRATION.md, read from an isolated clone")
    args = ap.parse_args()

    version = workspace_version()
    receipt = {
        "receipt": "powerio-producer/1",
        "subject_commit": args.subject_commit or git("rev-parse", "HEAD"),
        "powerio_jl_commit": args.julia_commit,
        "versions": {
            "powerio": version,
            "c_abi": abi_version(),
            "stored_module_schema": {"name": "powerio.module", "version": 1},
            "matrix_arrow": "arrow C data interface; catalog via pio_arrow_catalog_json",
            "diagnostic_wire": "structured records: code, severity, message, id?, target?, "
                               "suggested_action?, spans[], related[], details_json?",
            "mcp_schema": version,
        },
        "wasm32": {
            "target": "wasm32-wasip1 (build + smoke); wasm32-unknown-unknown (check)",
            "crates": ["powerio-core", "powerio-tx", "powerio-dist", "powerio-prob",
                        "powerio-matrix", "powerio"],
            "features": "default plus matrix on the facade; no C ABI, no gridfm parquet",
            "smoke": "powerio/examples/wasm_smoke.rs: named in-memory bytes parse, "
                     "typed narrowing, byte exact echo",
        },
        "schema_digests": schema_digests(),
        "value_kinds": value_kinds(),
        "semantics": {
            "element_identity": "source element ids verbatim; uid row identity in stored documents",
            "index_spaces": "dense positions zero based at C and Python, one based in Julia select_state",
            "dc_susceptance_sign": "branch_susceptance is a positive Laplacian edge weight; "
                                    "flow p_branch = -b*(va_f - va_t) + b*shift",
            "units": "raw source units on parse; per unit and radians after to_normalized",
            "diagnostics": "stable dotted codes; branch on code, never message text; "
                            "spans are byte ranges into retained sources",
            "unsupported_capabilities": "writers report fidelity losses as diagnostics instead of dropping silently",
        },
        "boundary_changes_since_previous": [
            "one parse family (pio_parse_file/str/bytes) returns PioModule for every value kind",
            "PioNetwork renamed PioBalancedNetwork; PioDistNetwork renamed PioMulticonductorNetwork",
            "errbuf calling convention replaced by PioError** with structured diagnostics",
            "pio_string_free renamed pio_string_release; free verbs retired",
            "pio_dist_* parse/convert family folded into the one parse and convert family",
            "structured PioDiagnostics accessors added (len, code, severity, message, id, target, "
            "suggested_action, details_json, spans, related)",
            "stored .pio.json is the version 1 module document; released 0.9 packages upgrade one way",
        ],
    }
    if args.boundary_doc_digest:
        receipt["tellegen_boundary_doc_sha256"] = args.boundary_doc_digest

    out = ROOT / args.out
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(receipt, indent=2, sort_keys=False) + "\n")
    print(f"wrote {out.relative_to(ROOT)} for subject {receipt['subject_commit'][:12]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
