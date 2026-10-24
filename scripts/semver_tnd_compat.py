#!/usr/bin/env python3
"""Expose moved definitions to rustdoc JSON in isolated SemVer source archives.

Rustdoc JSON omits foreign re-export definitions, even with doc(inline):
https://github.com/obi1kenobi/cargo-semver-checks/issues/638
Copy the actual tnd definitions into private facade modules for comparison.
Keep the facade's export lists, so removed exports still fail, and copy current
definitions verbatim, so changes to their fields, variants and methods are checked.
Published sources and the release baseline are never rewritten by this helper.
"""

from __future__ import annotations

import pathlib
import sys
import tempfile


def replace_once(text: str, old: str, new: str) -> str:
    if text.count(old) != 1:
        raise SystemExit(f"expected one T&D compatibility anchor: {old!r}")
    return text.replace(old, new)


def patch_root(root: pathlib.Path) -> None:
    if (root / ".git").exists():
        raise SystemExit("T&D SemVer preparation requires an isolated source archive")
    tnd = root / "powerio-tnd/src"
    if not tnd.exists():
        return
    facade = root / "powerio/src"
    transform = replace_once(
        (facade / "transform.rs").read_text(),
        "pub use powerio_tnd::transform::{",
        "pub use crate::__semver_tnd_transform::{",
    )
    codes = replace_once(
        (facade / "codes.rs").read_text(),
        "pub use powerio_tnd::diagnostics::*;",
        "pub use crate::__semver_tnd_diagnostics::*;",
    )
    implementation = replace_once(
        (tnd / "transform.rs").read_text(),
        "use crate::diagnostics as codes;",
        "use crate::__semver_tnd_diagnostics as codes;",
    )
    diagnostics = (tnd / "diagnostics.rs").read_text()
    lib = (facade / "lib.rs").read_text()
    if "__semver_tnd_" in lib:
        raise SystemExit("T&D SemVer preparation already applied")
    (facade / "transform.rs").write_text(transform)
    (facade / "codes.rs").write_text(codes)
    (facade / "__semver_tnd_transform.rs").write_text(implementation)
    (facade / "__semver_tnd_diagnostics.rs").write_text(diagnostics)
    (facade / "lib.rs").write_text(
        lib + "\nmod __semver_tnd_transform;\nmod __semver_tnd_diagnostics;\n"
    )


def self_test() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        patch_root(root)  # A release before tnd needs no preparation.
        facade = root / "powerio/src"
        tnd = root / "powerio-tnd/src"
        facade.mkdir(parents=True)
        tnd.mkdir(parents=True)
        export = "pub use powerio_tnd::transform::{Options};\n"
        implementation = "use crate::diagnostics as codes;\npub struct Options { pub base: f64 }\n"
        (facade / "transform.rs").write_text(export)
        (facade / "codes.rs").write_text("pub use powerio_tnd::diagnostics::*;\n")
        (facade / "lib.rs").write_text("pub mod transform;\npub mod codes;\n")
        (tnd / "transform.rs").write_text(implementation)
        (tnd / "diagnostics.rs").write_text("pub const CODE: u32 = 1;\n")
        patch_root(root)
        assert (facade / "transform.rs").read_text().endswith("::{Options};\n")
        assert (facade / "__semver_tnd_transform.rs").read_text() == implementation.replace(
            "crate::diagnostics", "crate::__semver_tnd_diagnostics"
        )
        assert (facade / "__semver_tnd_diagnostics.rs").read_bytes() == (
            tnd / "diagnostics.rs"
        ).read_bytes()
        try:
            patch_root(root)
        except SystemExit:
            pass
        else:
            raise AssertionError("unknown source shape must fail")
        (root / ".git").mkdir()
        try:
            patch_root(root)
        except SystemExit:
            pass
        else:
            raise AssertionError("a live checkout must not be rewritten")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
    elif len(sys.argv) > 1:
        for argument in sys.argv[1:]:
            patch_root(pathlib.Path(argument))
    else:
        raise SystemExit("usage: semver_tnd_compat.py --self-test | ROOT [ROOT ...]")
