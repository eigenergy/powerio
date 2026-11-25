"""Check the publish logic in .github/workflows/npm.yml without a registry.

The publish step's shell is extracted from the workflow and run against a fake
`npm` on PATH, so a change to its skip, bootstrap, or failure handling fails
here. The structural checks keep the release trigger, the protected `npm`
environment, OIDC permissions, provenance, and the absence of a stored token.
This never calls npm or GitHub.
"""

import os
import re
import subprocess
import tempfile
import textwrap
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = (ROOT / ".github/workflows/npm.yml").read_text()


def job(name: str) -> str:
    match = re.search(rf"^  {re.escape(name)}:\n(.*?)(?=^  [A-Za-z0-9_-]+:\n|\Z)", WORKFLOW, re.M | re.S)
    assert match, f"job {name} is missing"
    return match.group(1)


def step_script(body: str, name: str) -> str:
    marker = f"      - name: {name}\n"
    assert marker in body, f"step {name} is missing"
    step = body.split(marker, 1)[1]
    step = re.split(r"^      - ", step, maxsplit=1, flags=re.M)[0]
    return textwrap.dedent(step.split("        run: |\n", 1)[1])


# Structure: the release event and dispatch recovery, a protected environment,
# OIDC, provenance, and no token anywhere.
assert re.search(r"^  release:\n    types: \[ published \]$", WORKFLOW, re.M)
assert "  workflow_dispatch:" in WORKFLOW
build, publish = job("build"), job("publish")
assert "github.event.release.prerelease == false" in build
assert re.search(r"^    needs: build$", publish, re.M)
assert re.search(r"^    environment: npm$", publish, re.M)
assert re.search(r"^      id-token: write", publish, re.M)
assert "id-token" not in build, "only the protected publish job may mint an OIDC token"
assert "secrets." not in WORKFLOW and "NPM_TOKEN" not in WORKFLOW and "NODE_AUTH_TOKEN" not in WORKFLOW
assert "cargo test --locked -p powerio-wasm" in build
assert "cargo install wasm-pack --version 0.15.0 --locked" in build
assert "bash scripts/build-npm-package.sh" in build
BUILD_SCRIPT = (ROOT / "scripts/build-npm-package.sh").read_text()
assert "wasm-pack build powerio-wasm --target web --out-dir pkg --release --locked" in BUILD_SCRIPT
assert "wasm-pack pack powerio-wasm" in BUILD_SCRIPT
assert "build-npm-package.sh" in (ROOT / "scripts/npm-bootstrap.sh").read_text()
assert 'v$VERSION" != "$RELEASE_TAG"' in build
assert "PACKAGE: powerio-wasm" in WORKFLOW
for uses in re.findall(r"uses: (\S+)", WORKFLOW):
    assert re.fullmatch(r"[\w.-]+/[\w.-]+@[0-9a-f]{40}", uses), f"{uses} is not pinned to a commit"

# The publish step under each registry state.
SCRIPT = step_script(publish, "Publish")
assert "--provenance" in SCRIPT
assert "sleep" in SCRIPT
SCRIPT = SCRIPT.replace("sleep 10", ":")

FAKE_NPM = r"""#!/usr/bin/env bash
echo "$*" >> "$NPM_LOG"
case "$1" in
  view)
    if [ "$2" = "powerio-wasm@1.2.3" ]; then
      case "$VERSION_STATE" in
        present) echo 1.2.3 ;;
        missing) [ -f "$NPM_PUBLISHED" ] && { echo 1.2.3; exit 0; }
                 echo "npm error code E404" >&2; exit 1 ;;
        down) echo "npm error code ETIMEDOUT" >&2; exit 1 ;;
      esac
    else
      case "$PACKAGE_STATE" in
        present) echo powerio-wasm ;;
        missing) echo "npm error code E404" >&2; exit 1 ;;
        down) echo "npm error code ETIMEDOUT" >&2; exit 1 ;;
      esac
    fi ;;
  publish)
    [ "$PUBLISH" = ok ] || { echo "npm error code E404" >&2; exit 1; }
    touch "$NPM_PUBLISHED" ;;
esac
"""


def run(version_state, package_state="present", publish_result="ok"):
    with tempfile.TemporaryDirectory() as tmp:
        bin_dir = Path(tmp, "bin")
        bin_dir.mkdir()
        npm = bin_dir / "npm"
        npm.write_text(FAKE_NPM)
        npm.chmod(0o755)
        log = Path(tmp, "npm.log")
        env = {
            "PATH": f"{bin_dir}:{os.environ['PATH']}",
            "NPM_LOG": str(log),
            "NPM_PUBLISHED": str(Path(tmp, "published")),
            "VERSION_STATE": version_state,
            "PACKAGE_STATE": package_state,
            "PUBLISH": publish_result,
            "PACKAGE": "powerio-wasm",
            "VERSION": "1.2.3",
            "RELEASE_TAG": "v1.2.3",
            "TARBALL": "dist/powerio-wasm-1.2.3.tgz",
        }
        done = subprocess.run(["bash", "-eo", "pipefail", "-c", SCRIPT], env=env, capture_output=True, text=True)
        calls = log.read_text().splitlines() if log.exists() else []
        published = [call for call in calls if call.startswith("publish")]
        return done.returncode, done.stdout + done.stderr, published


# A version already on npm is skipped: a rerun, or the run after a bootstrap.
code, out, published = run("present")
assert code == 0 and "skipping" in out and not published, (code, out)

# A new version of an existing package publishes with provenance.
code, out, published = run("missing")
assert code == 0 and published == ["publish dist/powerio-wasm-1.2.3.tgz --provenance --access public"], (code, out, published)

# The first version cannot use trusted publishing: point at the bootstrap.
code, out, published = run("missing", package_state="missing")
assert code == 1 and "scripts/npm-bootstrap.sh v1.2.3" in out and not published, (code, out)

# A refused trusted publish names the publisher settings; no token fallback.
code, out, published = run("missing", publish_result="refused")
assert code == 1 and "workflow npm.yml, environment npm" in out and len(published) == 1, (code, out)

# An unreachable registry fails without publishing.
for states in [("down", "present"), ("missing", "down")]:
    code, out, published = run(*states)
    assert code == 1 and "re-run once the registry is reachable" in out and not published, (states, code, out)

print("npm workflow checks OK")
