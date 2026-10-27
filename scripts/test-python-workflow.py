"""Check the distribution gate in .github/workflows/python.yml.

The gate step is extracted from the workflow and run under every combination of
prerequisite results, so a change to its shell logic fails here. The structural
checks keep the productive workers conditional on their prerequisites and keep
publishing to release and recovery events. The platform choice is run for each
event, so a pull request builds the x86_64 Linux wheel and every other event
builds all five, and the gate's wheel count is run against both shapes.
"""

import itertools
import json
import os
import re
import subprocess
import tempfile
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / ".github/workflows/python.yml").read_text()


def job(name: str) -> str:
    match = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)", WORKFLOW, re.M | re.S)
    assert match, f"job {name} is missing"
    return match.group(1)


def condition(name: str) -> str:
    match = re.search(r"^    if: (.*)$", job(name), re.M)
    assert match, f"job {name} has no if condition"
    return match.group(1)


# The gate rejects every prerequisite that is not a success.
gate = job("validate-distributions")
marker = "      - name: Require every distribution prerequisite to succeed\n"
step = gate.split(marker, 1)[1]
env_block, run_block = step.split("        run: |\n", 1)
script = textwrap.dedent(run_block.split("      - uses:", 1)[0])
variables = re.findall(r"^          (\w+_RESULT): \$\{\{ needs\.([\w-]+)\.result \}\}$", env_block, re.M)
assert [needs for _, needs in variables] == ["release-preflight", "wheels", "wheel-smoke", "sdist"], variables
needs_line = re.search(r"^    needs: \[ (.*) \]$", gate, re.M)
assert needs_line and [n.strip() for n in needs_line.group(1).split(",")] == [n for _, n in variables]

for results in itertools.product(["success", "failure", "skipped", "cancelled", ""], repeat=len(variables)):
    env = {name: result for (name, _), result in zip(variables, results)}
    done = subprocess.run(["bash", "-euo", "pipefail", "-c", script], env=env, capture_output=True, text=True)
    expected = all(result == "success" for result in results)
    assert (done.returncode == 0) == expected, (results, done.returncode, done.stderr)

# The gate runs after failures but not after cancellation, and checks metadata.
assert condition("validate-distributions") == "${{ !cancelled() }}"
assert "twine check" in gate

# Productive workers keep the default success condition on their prerequisites.
for name in ["wheels", "wheel-smoke", "sdist"]:
    assert "always()" not in condition(name) and "cancelled()" not in condition(name), name
    assert condition(name) == "needs.release-preflight.outputs.wheels == 'true'", name


def step_script(job_text: str, name: str) -> tuple[str, str]:
    """Return the text before `run: |` and the dedented script of one step."""
    step = job_text.split(f"      - name: {name}\n", 1)[1]
    step = re.split(r"^      - ", step, maxsplit=1, flags=re.M)[0]
    head, run_block = step.split("        run: |\n", 1)
    return head, textwrap.dedent(run_block)


# The preflight chooses the wheel platforms on every event: a pull request gets
# the x86_64 Linux wheel, every other event all five release platforms.
preflight = job("release-preflight")
assert "      platforms: ${{ steps.platforms.outputs.platforms }}\n" in preflight
head, choose = step_script(preflight, "Choose the wheel platforms")
assert "        id: platforms\n" in head and "if:" not in head, head
linux = {"runner": "ubuntu-latest", "target": "x86_64"}
release_platforms = [
    linux,
    {"runner": "ubuntu-24.04-arm", "target": "aarch64"},
    {"runner": "macos-14", "target": "x86_64"},
    {"runner": "macos-14", "target": "aarch64"},
    {"runner": "windows-latest", "target": "x64"},
]
with tempfile.TemporaryDirectory() as tmp:
    for event, expected in [
        ("pull_request", [linux]),
        ("push", release_platforms),
        ("release", release_platforms),
        ("workflow_dispatch", release_platforms),
    ]:
        output = Path(tmp) / f"{event}.out"
        env = {"PATH": os.environ["PATH"], "GITHUB_EVENT_NAME": event, "GITHUB_OUTPUT": str(output)}
        done = subprocess.run(["bash", "-euo", "pipefail", "-c", choose], env=env, capture_output=True, text=True)
        assert done.returncode == 0, (event, done.stderr)
        lines = output.read_text().splitlines()
        assert len(lines) == 1 and lines[0].startswith("platforms="), lines
        assert json.loads(lines[0].removeprefix("platforms=")) == expected, (event, lines)

# Both wheel jobs run over that one list, and the smoke job downloads exactly
# the artifact the build job uploaded for its platform.
artifact = "wheels-${{ matrix.platform.runner }}-${{ matrix.platform.target }}"
for name in ["wheels", "wheel-smoke"]:
    assert "        platform: ${{ fromJSON(needs.release-preflight.outputs.platforms) }}\n" in job(name), name
assert f"          name: {artifact}\n" in job("wheels")
assert f"          name: {artifact}\n" in job("wheel-smoke")

# The gate counts one wheel per chosen platform and one sdist.
head, count = step_script(gate, "Count the built distributions")
assert "PLATFORMS: ${{ needs.release-preflight.outputs.platforms }}" in head
for platforms, wheels, sdists, expected in [
    (release_platforms, 5, 1, True),
    ([linux], 1, 1, True),
    (release_platforms, 4, 1, False),
    (release_platforms, 1, 1, False),
    ([linux], 1, 0, False),
    ([linux], 2, 1, False),
]:
    with tempfile.TemporaryDirectory() as tmp:
        dist = Path(tmp) / "dist"
        dist.mkdir()
        for i in range(wheels):
            (dist / f"powerio-{i}.whl").touch()
        for i in range(sdists):
            (dist / f"powerio-{i}.tar.gz").touch()
        env = {"PATH": os.environ["PATH"], "PLATFORMS": json.dumps(platforms)}
        done = subprocess.run(["bash", "-euo", "pipefail", "-c", count], env=env, cwd=tmp, capture_output=True, text=True)
        assert (done.returncode == 0) == expected, (len(platforms), wheels, sdists, done.stderr)

# Nothing but the release-only publish job may upload to PyPI.
assert WORKFLOW.count("pypa/gh-action-pypi-publish") == 1
assert "pypa/gh-action-pypi-publish" in job("publish")
assert "environment: pypi" in job("publish")
assert "always()" not in condition("publish")
assert "pull_request" not in condition("publish")
assert re.search(r"^  (upload|publish-[\w-]+):", WORKFLOW, re.M) is None
print("python.yml distribution gate checks passed")
