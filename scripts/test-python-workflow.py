"""Check the distribution gate in .github/workflows/python.yml.

The gate step is extracted from the workflow and run under every combination of
prerequisite results, so a change to its shell logic fails here. The structural
checks keep the productive workers conditional on their prerequisites and keep
publishing to release and recovery events.
"""

import itertools
import re
import subprocess
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

# Nothing but the release-only publish job may upload to PyPI.
assert WORKFLOW.count("pypa/gh-action-pypi-publish") == 1
assert "pypa/gh-action-pypi-publish" in job("publish")
assert "environment: pypi" in job("publish")
assert "always()" not in condition("publish")
assert "pull_request" not in condition("publish")
assert re.search(r"^  (upload|publish-[\w-]+):", WORKFLOW, re.M) is None
print("python.yml distribution gate checks passed")
