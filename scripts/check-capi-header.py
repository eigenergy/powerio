#!/usr/bin/env python3
"""Compare powerio.h with the header of the latest release.

Within a release series the C ABI only gains symbols: no symbol is removed,
and no signature or struct layout changes. This gate holds the checked-in
header to that rule. It takes the newest stable release tag at or below the
workspace version, the same baseline scripts/check-semver.sh uses, reads that
tag's powerio.h, and fails when a function declaration, struct, enum, opaque
type, or #define of the old header is missing from the current one or
declared differently. Comments, line wrapping, and parameter names do not
count; struct field names do, because C callers read fields by name. The
three PIO_VERSION_* macros must stay, and their values move with every
release.

A workspace version in a later release series than the baseline tag may
break the C ABI, so the differences are then listed without failing. The
semver-major label lets a pull request break it before the version moves;
semver.yml applies it.

Exit status: 0 when nothing broke, 1 when something did, 2 when the baseline
cannot be read (a shallow clone without tags, for one).
"""

from __future__ import annotations

import argparse
import difflib
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
HEADER = "powerio-capi/include/powerio.h"
VERSION_MACROS = {"PIO_VERSION_MAJOR", "PIO_VERSION_MINOR", "PIO_VERSION_PATCH"}
INCLUDE_GUARD = "POWERIO_H"


def release(text: str) -> tuple[int, int, int]:
    major, minor, patch = (int(part) for part in text.split("."))
    return major, minor, patch


def series(version: tuple[int, int, int]) -> tuple[int, ...]:
    """The release series as Cargo semver defines it."""
    return version[:1] if version[0] >= 1 else version[:2]


def series_name(version: tuple[int, int, int]) -> str:
    return ".".join(str(part) for part in series(version)) + ".x"


def workspace_version() -> str:
    cargo = (ROOT / "Cargo.toml").read_text()
    match = re.search(r'^version = "(\d+\.\d+\.\d+)"', cargo, re.M)
    if match is None:
        raise SystemExit("Cargo.toml states no workspace version")
    return match.group(1)


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args], cwd=ROOT, check=True, capture_output=True, text=True
    ).stdout


def baseline_tag(version: str) -> str | None:
    """The newest stable release tag at or below `version`."""
    tags = [
        tag
        for tag in git("tag", "--list", "v*").split()
        if re.fullmatch(r"v\d+\.\d+\.\d+", tag)
        and release(tag[1:]) <= release(version)
    ]
    return max(tags, key=lambda tag: release(tag[1:]), default=None)


def collapse(text: str) -> str:
    return " ".join(text.split())


def type_key(text: str) -> str:
    """Spacing-independent spelling of a C declaration fragment."""
    text = collapse(text)
    return re.sub(r"\s*([*(),{};\[\]])\s*", r"\1", text)


def split_top_level(text: str, separator: str) -> list[str]:
    parts, depth, start = [], 0, 0
    for index, char in enumerate(text):
        if char in "({[":
            depth += 1
        elif char in ")}]":
            depth -= 1
        elif char == separator and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return [part.strip() for part in parts if part.strip()]


def parameter_type(parameter: str) -> str:
    """Drop the parameter name, which is not part of the ABI."""
    match = re.fullmatch(r"(.*?[\w*\s])\s*\b([A-Za-z_]\w*)", parameter)
    if match and match.group(1).strip() and parameter.strip() != "void":
        return type_key(match.group(1))
    return type_key(parameter)


def declarations(header: str) -> dict[str, tuple[str, str]]:
    """Map `kind name` to (comparison key, display text) for one header."""
    text = re.sub(r"/\*.*?\*/", " ", header, flags=re.S)
    text = re.sub(r"//[^\n]*", " ", text)
    found: dict[str, tuple[str, str]] = {}
    body_lines = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("#"):
            define = re.fullmatch(r"#\s*define\s+(\w+)(?:\s+(.*))?", stripped)
            if define and define.group(1) != INCLUDE_GUARD:
                name, value = define.group(1), collapse(define.group(2) or "")
                key = "" if name in VERSION_MACROS else value
                found[f"#define {name}"] = (key, f"#define {name} {value}".strip())
            continue
        if stripped in ('extern "C" {', "}"):
            continue
        body_lines.append(line)
    for statement in split_top_level("\n".join(body_lines), ";"):
        display = collapse(statement)
        opaque = re.fullmatch(r"typedef (struct|union|enum) (\w+) (\w+)", display)
        aggregate = re.fullmatch(
            r"typedef (struct|union|enum)(?: \w+)? \{(.*)\} (\w+)", display
        )
        tagged = re.fullmatch(r"(struct|union|enum) (\w+) \{(.*)\}", display)
        function = re.fullmatch(r"(.*?)\b(\w+)\((.*)\)", display)
        if opaque:
            found[f"opaque type {opaque.group(3)}"] = (type_key(display), display)
        elif aggregate:
            kind, body, name = aggregate.groups()
            found[f"{kind} {name}"] = (type_key(body), display)
        elif tagged:
            kind, name, body = tagged.groups()
            found[f"{kind} {name}"] = (type_key(body), display)
        elif function:
            result, name, parameters = function.groups()
            signature = [type_key(result)] + [
                parameter_type(parameter)
                for parameter in split_top_level(parameters, ",")
            ]
            found[f"function {name}"] = ("|".join(signature), display)
        elif display.startswith("typedef "):
            name = re.findall(r"\w+", display)[-1]
            found[f"typedef {name}"] = (type_key(display), display)
        else:
            found[f"declaration {display}"] = (type_key(display), display)
    return found


def members(display: str) -> list[str]:
    body = display[display.index("{") + 1 : display.rindex("}")]
    separator = "," if re.match(r"(typedef )?enum\b", display) else ";"
    return [collapse(member) for member in split_top_level(body, separator)]


def describe_change(old: str, new: str) -> list[str]:
    if "{" in old and "{" in new:
        diff = difflib.unified_diff(members(old), members(new), lineterm="", n=1)
        return ["    " + line for line in list(diff)[2:] if not line.startswith("@@")]
    return [f"    was: {old}", f"    now: {new}"]


def compare(old_header: str, new_header: str) -> list[list[str]]:
    """One block of report lines for each old declaration that broke."""
    old, new = declarations(old_header), declarations(new_header)
    problems = []
    for item, (old_key, old_display) in old.items():
        if item not in new:
            problems.append([f"  removed {item}: {old_display}"])
        elif new[item][0] != old_key:
            problems.append(
                [f"  changed {item}:"] + describe_change(old_display, new[item][1])
            )
    return problems


def render(problems: list[list[str]]) -> str:
    return "\n".join(line for problem in problems for line in problem)


def self_test() -> None:
    old = """
/* A comment with a ; in it. */
#define PIO_VERSION_PATCH 1
#define PIO_LIMIT 4
typedef struct PioThing PioThing;
typedef struct {
    const char *data;
    size_t len;
} PioStringView;
#ifdef __cplusplus
extern "C" {
#endif // __cplusplus
uint32_t pio_gone(void);
PioStringView pio_name(const PioThing *thing,
                       size_t index);
#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus
"""
    same = (
        old.replace("PATCH 1", "PATCH 2")
        .replace("thing,\n                       size_t index", "renamed, size_t row")
        .replace("A comment", "Another comment")
        + "void pio_new(void);\n#define PIO_NEW 1\n"
    )
    assert compare(old, same) == [], render(compare(old, same))
    broken = (
        old.replace("uint32_t pio_gone(void);\n", "")
        .replace("size_t len;", "size_t len;\n    bool owned;")
        .replace("size_t index", "uint32_t index")
        .replace("PIO_LIMIT 4", "PIO_LIMIT 5")
        .replace("typedef struct PioThing PioThing;", "")
    )
    report = render(compare(old, broken))
    for expected in (
        "removed function pio_gone",
        "changed function pio_name",
        "changed #define PIO_LIMIT",
        "changed struct PioStringView",
        "+bool owned",
        "removed opaque type PioThing",
    ):
        assert expected in report, (expected, report)
    assert "PIO_VERSION_PATCH" not in report, report
    assert "removed #define PIO_VERSION_PATCH" in render(
        compare(old, old.replace("#define PIO_VERSION_PATCH 1\n", ""))
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--baseline", help="compare with this tag instead of the latest release"
    )
    parser.add_argument("--self-test", action="store_true")
    arguments = parser.parse_args()
    if arguments.self_test:
        self_test()
        print("check-capi-header self test passes")
        return 0

    version = workspace_version()
    tag = arguments.baseline or baseline_tag(version)
    if tag is None:
        print(
            f"no stable release tag at or below {version}; fetch the tags (git fetch --tags)",
            file=sys.stderr,
        )
        return 2
    try:
        old_header = git("show", f"{tag}:{HEADER}")
    except subprocess.CalledProcessError as failure:
        print(f"cannot read {HEADER} at {tag}: {failure.stderr.strip()}", file=sys.stderr)
        return 2
    new_header = (ROOT / HEADER).read_text()

    print(f"comparing {HEADER} at {version} with the {tag} header", flush=True)
    problems = compare(old_header, new_header)
    if not problems:
        print(f"powerio.h keeps every declaration of the {tag} header")
        return 0
    if re.fullmatch(r"v\d+\.\d+\.\d+", tag) and series(release(tag[1:])) != series(
        release(version)
    ):
        print(
            f"{version} starts the {series_name(release(version))} release series, "
            f"so it may break the C ABI of {tag}; differences:"
        )
        print(render(problems))
        return 0
    print(
        f"error: powerio.h removes or changes {len(problems)} declarations of the "
        f"{tag} header, which is in the same release series:",
        file=sys.stderr,
    )
    print(render(problems), file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
