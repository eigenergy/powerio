"""Tests for the MCP path containment policy (``powerio.mcp.sandbox``).

The module is the SDK free half of ``powerio.mcp``: these run on every
interpreter the wheel supports, with or without ``powerio[mcp]`` installed.
"""

import os
import subprocess
import sys
from pathlib import Path

import pytest
from powerio.mcp import sandbox

DATA = Path(__file__).resolve().parents[2] / "tests" / "data"
ENV_NAMES = (
    "POWERIO_MCP_ALLOWED_ROOTS",
    "POWERIO_MCP_ROOT",
    "POWERIO_MCP_ALLOWED_ROOT",
)


@pytest.fixture(autouse=True)
def _clear_roots(monkeypatch):
    for name in ENV_NAMES:
        monkeypatch.delenv(name, raising=False)


def test_importing_the_sandbox_does_not_pull_in_the_mcp_sdk():
    code = (
        "import sys; from powerio.mcp import sandbox; "
        "assert sandbox.ALLOWED_ROOTS_ENV; "
        "print(sorted(m for m in sys.modules if m.split('.')[0] == 'mcp'))"
    )
    out = subprocess.run(
        [sys.executable, "-c", code], check=True, capture_output=True, text=True
    )
    assert out.stdout.strip() == "[]"


def test_no_roots_configured_allows_any_path(tmp_path):
    assert sandbox.allowed_roots() == ()
    assert sandbox.checked_path(str(tmp_path / "case9.m")) == str(tmp_path / "case9.m")


def test_main_names_the_python_requirement_below_3_10(monkeypatch):
    """`powerio.mcp.main` is resolved lazily so reaching `sandbox` never
    imports the SDK; below 3.10 it must refuse before server.py's
    typing.TypeAlias import runs, with a message naming the requirement,
    rather than failing on that import with no explanation."""
    import powerio.mcp as mcp_pkg

    monkeypatch.setattr(sys, "version_info", (3, 9, 0))
    with pytest.raises(ImportError, match="Python 3.10"):
        _ = mcp_pkg.main


def test_allowed_roots_splits_on_pathsep(monkeypatch, tmp_path):
    a = tmp_path / "a"
    b = tmp_path / "b"
    a.mkdir()
    b.mkdir()
    monkeypatch.setenv(
        sandbox.ALLOWED_ROOTS_ENV, os.pathsep.join([str(a), "  ", f" {b} "])
    )
    assert sandbox.allowed_roots() == (a.resolve(), b.resolve())


@pytest.mark.parametrize("name", ENV_NAMES)
def test_every_accepted_spelling_restricts_reads(monkeypatch, tmp_path, name):
    inside = tmp_path / "case9.m"
    inside.write_text("x")
    monkeypatch.setenv(name, str(tmp_path))

    assert sandbox.checked_path(str(inside)) == str(inside)
    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        sandbox.checked_path(str(DATA / "case9.m"))


@pytest.mark.parametrize("legacy", ENV_NAMES[1:])
def test_the_primary_variable_wins_over_a_legacy_one(monkeypatch, tmp_path, legacy):
    primary = tmp_path / "primary"
    other = tmp_path / "other"
    primary.mkdir()
    other.mkdir()
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(primary))
    monkeypatch.setenv(legacy, str(other))

    assert sandbox.allowed_roots() == (primary.resolve(),)
    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        sandbox.checked_path(str(other / "case9.m"))


def test_the_first_legacy_spelling_wins_over_the_alternate(monkeypatch, tmp_path):
    first = tmp_path / "first"
    alternate = tmp_path / "alternate"
    first.mkdir()
    alternate.mkdir()
    monkeypatch.setenv("POWERIO_MCP_ROOT", str(first))
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOT", str(alternate))

    assert sandbox.allowed_roots() == (first.resolve(),)


def test_an_empty_variable_falls_through_to_the_next(monkeypatch, tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, "")
    monkeypatch.setenv("POWERIO_MCP_ALLOWED_ROOT", str(root))

    assert sandbox.allowed_roots() == (root.resolve(),)


def test_decode_local_path_reads_file_uris_and_refuses_remote_schemes():
    assert sandbox.decode_local_path("case 9.m") == Path("case 9.m")
    assert sandbox.decode_local_path("file:///data/case%209.m") == Path(
        "/data/case 9.m"
    )
    assert sandbox.decode_local_path("file://localhost/data/case.raw") == Path(
        "/data/case.raw"
    )
    with pytest.raises(sandbox.PathNotAllowed, match="local path or file:// URI"):
        sandbox.decode_local_path("https://example.com/case9.m", purpose="path")
    with pytest.raises(sandbox.PathNotAllowed, match="must be local"):
        sandbox.decode_local_path("file://server/share/case.raw")


def test_a_file_uri_is_contained_like_a_plain_path(monkeypatch, tmp_path):
    root = tmp_path / "root"
    root.mkdir()
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))

    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        sandbox.checked_path(f"file://{tmp_path}/elsewhere.m")


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_a_write_target_may_not_symlink_out_of_a_root(monkeypatch, tmp_path):
    root = tmp_path / "allowed"
    root.mkdir()
    outside = tmp_path / "outside"
    outside.mkdir()
    escape = root / "escape.json"
    os.symlink(outside / "leaked.json", escape)
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))

    with pytest.raises(ValueError, match="outside allowed MCP roots"):
        sandbox.checked_path(str(escape), purpose="out_path", for_write=True)
    assert sandbox.checked_path(
        str(root / "ok.json"), purpose="out_path", for_write=True
    ) == str(root / "ok.json")


def test_a_write_into_a_missing_directory_raises(monkeypatch, tmp_path):
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(tmp_path))
    with pytest.raises(ValueError, match="cannot resolve `out_path`"):
        sandbox.checked_path(
            str(tmp_path / "nope" / "out.json"), purpose="out_path", for_write=True
        )


def test_refusals_raise_the_exported_type(monkeypatch, tmp_path):
    # Consumers match on the type; the prose is free to change.
    assert "PathNotAllowed" in sandbox.__all__
    assert issubclass(sandbox.PathNotAllowed, ValueError)
    root = tmp_path / "root"
    root.mkdir()
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))
    with pytest.raises(sandbox.PathNotAllowed):
        sandbox.checked_path(str(tmp_path / "elsewhere.m"))
    with pytest.raises(sandbox.PathNotAllowed):
        sandbox.checked_path(
            str(root / "nope" / "out.json"), purpose="out_path", for_write=True
        )


def test_admitting_root_names_the_containing_root(monkeypatch, tmp_path):
    a = tmp_path / "a"
    b = tmp_path / "b"
    a.mkdir()
    b.mkdir()
    case = b / "case.dss"
    case.write_text("New Circuit.c\n")
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, os.pathsep.join([str(a), str(b)]))
    assert sandbox.admitting_root(case) == b.resolve()
    with pytest.raises(sandbox.PathNotAllowed):
        sandbox.admitting_root(tmp_path / "elsewhere.dss")


def test_admitting_root_is_none_when_the_policy_is_off(tmp_path):
    assert sandbox.admitting_root(tmp_path / "case.dss") is None


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_read_tree_refuses_escaping_and_broken_child_links(monkeypatch, tmp_path):
    root = tmp_path / "allowed"
    case = root / "case"
    outside = tmp_path / "outside"
    case.mkdir(parents=True)
    outside.mkdir()
    (outside / "buses.csv").write_text("outside")
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))

    os.symlink(outside / "buses.csv", case / "buses.csv")
    with pytest.raises(sandbox.PathNotAllowed, match="outside its allowed MCP root"):
        sandbox.checked_read_tree(str(case))

    (case / "buses.csv").unlink()
    os.symlink(case / "missing.csv", case / "buses.csv")
    with pytest.raises(sandbox.PathNotAllowed, match="broken link"):
        sandbox.check_allowed_read_tree(case)


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_read_tree_follows_contained_directory_links_without_looping(
    monkeypatch, tmp_path
):
    root = tmp_path / "allowed"
    case = root / "case"
    shared = root / "shared"
    case.mkdir(parents=True)
    shared.mkdir()
    (shared / "buses.csv").write_text("inside")
    os.symlink(shared, case / "tables")
    os.symlink(root, shared / "back-to-root")
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))

    assert sandbox.checked_read_tree(str(case)) == str(case)


@pytest.mark.skipif(os.name == "nt", reason="POSIX special-file semantics")
def test_read_tree_refuses_special_files(monkeypatch, tmp_path):
    root = tmp_path / "allowed"
    root.mkdir()
    os.mkfifo(root / "stream")
    monkeypatch.setenv(sandbox.ALLOWED_ROOTS_ENV, str(root))

    with pytest.raises(sandbox.PathNotAllowed, match="non-file, non-directory"):
        sandbox.check_allowed_read_tree(root)


def _write_tree(staging: str, files: dict[str, str]) -> dict:
    # Real writers create their own target; the staging path handed to the
    # callback does not exist yet.
    root = Path(staging)
    root.mkdir(parents=True, exist_ok=True)
    paths = []
    for relative, text in files.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        paths.append(str(path))
    return {"dir": staging, "files": paths}


def test_staged_directory_write_preserves_unrelated_files_and_rebases_paths(tmp_path):
    out = tmp_path / "dataset"
    out.mkdir()
    (out / "keep.txt").write_text("keep")
    (out / "replace.csv").write_text("old")

    result = sandbox.staged_directory_write(
        str(out),
        True,
        lambda staging: _write_tree(
            staging, {"replace.csv": "new", "nested/new.csv": "new"}
        ),
    )

    assert (out / "keep.txt").read_text() == "keep"
    assert (out / "replace.csv").read_text() == "new"
    assert (out / "nested/new.csv").read_text() == "new"
    assert result["dir"] == str(out)
    assert result["files"] == [str(out / "replace.csv"), str(out / "nested/new.csv")]


def test_staged_directory_write_refuses_collisions_before_install(tmp_path):
    out = tmp_path / "dataset"
    out.mkdir()
    (out / "same.csv").write_text("old")

    with pytest.raises(ValueError, match="refusing to overwrite"):
        sandbox.staged_directory_write(
            str(out),
            False,
            lambda staging: _write_tree(staging, {"same.csv": "new", "new.csv": "new"}),
        )

    assert (out / "same.csv").read_text() == "old"
    assert not (out / "new.csv").exists()


@pytest.mark.skipif(os.name == "nt", reason="POSIX symlink semantics")
def test_staged_directory_write_refuses_generated_and_existing_links(tmp_path):
    outside = tmp_path / "outside.txt"
    outside.write_text("outside")
    out = tmp_path / "dataset"

    def linked_output(staging: str) -> dict:
        Path(staging).mkdir(parents=True, exist_ok=True)
        os.symlink(outside, Path(staging) / "linked.csv")
        return {"dir": staging, "files": [str(Path(staging) / "linked.csv")]}

    with pytest.raises(sandbox.PathNotAllowed, match="contains a link"):
        sandbox.staged_directory_write(str(out), False, linked_output)
    assert not out.exists()

    out.mkdir()
    os.symlink(outside, out / "existing.csv")
    with pytest.raises(sandbox.PathNotAllowed, match="contains a link"):
        sandbox.staged_directory_write(
            str(out), True, lambda staging: _write_tree(staging, {"new.csv": "new"})
        )
    assert outside.read_text() == "outside"
    assert not (out / "new.csv").exists()


def test_staged_directory_write_rolls_back_a_failed_swap(monkeypatch, tmp_path):
    out = tmp_path / "dataset"
    out.mkdir()
    (out / "same.csv").write_text("old")
    real_replace = sandbox.os.replace
    failed = False

    def fail_install(source, target):
        nonlocal failed
        if not failed and ".install-" in Path(source).name and Path(target) == out:
            failed = True
            raise OSError("simulated install failure")
        return real_replace(source, target)

    monkeypatch.setattr(sandbox.os, "replace", fail_install)
    with pytest.raises(OSError, match="simulated install failure"):
        sandbox.staged_directory_write(
            str(out), True, lambda staging: _write_tree(staging, {"same.csv": "new"})
        )

    assert (out / "same.csv").read_text() == "old"
    assert not [
        path for path in tmp_path.iterdir() if path.name.startswith(".dataset.")
    ]
