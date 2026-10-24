#!/usr/bin/env python3
"""Prepare isolated source trees for cargo-semver-checks across the ZIP 9 upgrade.

The historical baseline may still use yanked ZIP 8.6 and its old reader API;
the current tree may already use ZIP 9.0.5. Only recognized old anchors are
rewritten. A tree with an unknown dependency or source shape fails closed.
"""
from __future__ import annotations

import pathlib
import re
import sys
import tempfile

OLD_ZIP = "zip = { version = \"8.6\", default-features = false, features = [\"deflate-flate2-zlib-rs\"] }"
NEW_ZIP = "zip = { version = \"9.0.5\", default-features = false, features = [\"deflate-flate2-zlib-rs\"] }"
REPLACEMENTS = [
    [
        "let raw_name = file.name().to_string();",
        "let raw_name = file\n            .name()\n            .map_err(|error| {\n                format_error(format!(\"cannot read entry name in {archive_name}: {error}\"))\n            })?\n            .into_owned();"
    ],
    [
        "return Err(format_error(format!(\n                    \"archive {archive_name} is refused as malformed: entry {} lies outside the archive's entry data\",\n                    entry.name()\n                )));",
        "let entry_name = entry.name().map_err(|error| {\n                    format_error(format!(\"cannot read entry name in {archive_name}: {error}\"))\n                })?;\n                return Err(format_error(format!(\n                    \"archive {archive_name} is refused as malformed: entry {entry_name} lies outside the archive's entry data\"\n                )));"
    ],
    [
        "let name = |index| archive.name_for_index(index).unwrap_or_default();",
        "let name = |index| {\n            archive\n                .name_for_index(index)\n                .and_then(std::result::Result::ok)\n                .map(std::borrow::Cow::into_owned)\n                .unwrap_or_default()\n        };"
    ]
]


def patch_root(root: pathlib.Path) -> None:
    """Normalize a temporary tree without downgrading an already-upgraded tree."""
    manifest = root / "Cargo.toml"
    text = manifest.read_text()
    zip_lines = [
        line.strip()
        for line in text.splitlines()
        if re.match(r"^\s*zip\s*=\s*\{", line)
    ]
    if len(zip_lines) != 1:
        raise SystemExit(f"expected exactly one workspace ZIP dependency in {root}")
    if zip_lines[0] == OLD_ZIP:
        text = text.replace(OLD_ZIP, NEW_ZIP)
        manifest.write_text(text)
    elif zip_lines[0] == NEW_ZIP:
        pass
    else:
        raise SystemExit(
            f"unsupported ZIP dependency in {root}: {zip_lines[0]!r}; "
            "expected ZIP 8.6 or ZIP 9.0.5"
        )

    source = root / "powerio-tx/src/format/cgmes/mod.rs"
    text = source.read_text()
    for old_anchor, new_anchor in REPLACEMENTS:
        old_count = text.count(old_anchor)
        new_count = text.count(new_anchor)
        if old_count == 1 and new_count == 0:
            text = text.replace(old_anchor, new_anchor)
        elif old_count == 0 and new_count == 1:
            continue
        else:
            raise SystemExit(
                f"unexpected ZIP API compatibility anchors in {root}: "
                f"old={old_count}, new={new_count}, anchor={old_anchor[:70]!r}"
            )
    source.write_text(text)


def self_test() -> None:
    """Cover old-baseline/new-current and old-baseline/old-current."""
    old_source = "\n\n".join(old for old, _ in REPLACEMENTS) + "\n"
    new_source = "\n\n".join(new for _, new in REPLACEMENTS) + "\n"

    def prepare_case(parent: pathlib.Path, case: str, versions: tuple[str, str]):
        roots = [parent / case / "baseline", parent / case / "current"]
        sources = [old_source, old_source if versions[1] == "8.6" else new_source]
        for root, version, source in zip(roots, versions, sources, strict=True):
            (root / "powerio-tx/src/format/cgmes").mkdir(parents=True)
            dep = OLD_ZIP if version == "8.6" else NEW_ZIP
            (root / "Cargo.toml").write_text(f"[workspace.dependencies]\n{dep}\n")
            (root / "powerio-tx/src/format/cgmes/mod.rs").write_text(source)
        patch_root(roots[0])
        patch_root(roots[1])
        return roots[0], roots[1]

    with tempfile.TemporaryDirectory() as temp:
        parent = pathlib.Path(temp)
        baseline, current = prepare_case(parent, "mixed", ("8.6", "9.0.5"))
        assert NEW_ZIP in (baseline / "Cargo.toml").read_text()
        current_manifest = (current / "Cargo.toml").read_text()
        assert NEW_ZIP in current_manifest and OLD_ZIP not in current_manifest
        assert (current / "powerio-tx/src/format/cgmes/mod.rs").read_text() == new_source

        baseline, current = prepare_case(parent, "legacy", ("8.6", "8.6"))
        assert NEW_ZIP in (baseline / "Cargo.toml").read_text()
        assert NEW_ZIP in (current / "Cargo.toml").read_text()
        assert (current / "powerio-tx/src/format/cgmes/mod.rs").read_text() == new_source


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) > 1:
        for argument in sys.argv[1:]:
            patch_root(pathlib.Path(argument))
    else:
        raise SystemExit("usage: semver_zip_compat.py --self-test | ROOT [ROOT ...]")
