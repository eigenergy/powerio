#!/usr/bin/env python3
"""Publish one reviewed PowerIO release using existing GitHub CLI authentication.

No login, credential creation, force pushes, workflow bypasses, or deployment
approvals. Safe to rerun with the same immutable candidate after interruption.
"""

import argparse
import base64
import json
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

REPO = "eigenergy/powerio"
JULIA_REPO = "eigenergy/PowerIO.jl"
PLATFORMS = (
    "aarch64-apple-darwin",
    "aarch64-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-linux-gnu",
    "x86_64-w64-mingw32",
)
MAIN_WORKFLOWS = {
    "rust.yml",
    "python.yml",
    "docs.yml",
    "julia-binding.yml",
    "converter.yml",
    "powsybl.yml",
}
CRATES = (
    "powerio-core",
    "powerio-sincal",
    "powerio-tx",
    "powerio-dist",
    "powerio-prob",
    "powerio-matrix",
    "powerio",
    "powerio-cli",
)


class ReleaseError(RuntimeError):
    pass


def api(path, method="GET", data=None, missing_ok=False):
    command = ["gh", "api", "--hostname", "github.com", "--method", method, path]
    if data is not None:
        command += ["--input", "-"]
    result = subprocess.run(
        command,
        input=None if data is None else json.dumps(data),
        text=True,
        capture_output=True,
        check=False,
        timeout=60,
    )
    if result.returncode:
        if missing_ok and method == "GET" and "(HTTP 404)" in result.stderr:
            return None
        raise ReleaseError(f"GitHub {method} {path} failed: {result.stderr.strip()}")
    return json.loads(result.stdout) if result.stdout.strip() else None


def collection(path, key):
    result = []
    for page in range(1, 101):
        sep = "&" if "?" in path else "?"
        batch = api(f"{path}{sep}per_page=100&page={page}")[key]
        result.extend(batch)
        if len(batch) < 100:
            return result
    raise ReleaseError(
        "Refusing an unexpectedly large API result; nothing was inferred."
    )


def contents(repo, path, ref):
    value = api(f"repos/{repo}/contents/{path}?ref={ref}")
    if value.get("encoding") != "base64":
        raise ReleaseError(f"Unexpected content encoding for {path}")
    return base64.b64decode(value["content"]).decode("utf-8")


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def tag_commit(tag):
    ref = api(f"repos/{REPO}/git/ref/tags/{tag}", missing_ok=True)
    if ref is None:
        return None
    require(
        ref["object"]["type"] == "tag",
        "Existing tag is not annotated; refusing to replace it.",
    )
    obj = api(f"repos/{REPO}/git/tags/{ref['object']['sha']}")
    require(
        obj["tag"] == tag and obj["object"]["type"] == "commit",
        "Unexpected existing annotated tag shape; refusing to change it.",
    )
    return obj["object"]["sha"]


def validate_source_runs(runs, candidate):
    latest = {}
    for run in runs:
        if (
            run["head_sha"] == candidate
            and run["event"] == "push"
            and run["head_branch"] == "main"
        ):
            key = run["path"].split("@", 1)[0].rsplit("/", 1)[-1]
            if key not in latest or run["id"] > latest[key]["id"]:
                latest[key] = run
    require(
        MAIN_WORKFLOWS <= latest.keys(),
        "Required final-main source workflows are missing.",
    )
    for name, run in latest.items():
        require(
            run["status"] == "completed" and run["conclusion"] == "success",
            f"Final-main {name} is not successful: {run['html_url']}",
        )


def validate_release_jobs(jobs):
    required = {
        "PowerIO.jl main against this tag",
        "PowerIO.jl pins the packaged tarballs",
        "attach draft release assets",
    }
    required.update(PLATFORMS)
    required.update(f"PowerIO.jl on {platform}" for platform in PLATFORMS)
    by_name = {job["name"]: job for job in jobs}
    require(
        required <= by_name.keys(),
        "Exact tag run lacks a required source/packaged-platform gate.",
    )
    for name in required:
        require(
            by_name[name]["conclusion"] == "success",
            f"Release gate did not pass: {name}",
        )


def validate_release(release, tag):
    require(
        release["tag_name"] == tag and not release["prerelease"],
        "Release is not the intended stable tag.",
    )
    require(bool(release.get("body", "").strip()), "Release notes are empty.")
    assets = release["assets"]
    expected = {f"libpowerio_capi.{platform}.tar.gz" for platform in PLATFORMS}
    require(
        len(assets) == 5 and {asset["name"] for asset in assets} == expected,
        "Release must contain exactly the five expected native archives.",
    )
    require(
        all(asset["state"] == "uploaded" and asset["size"] > 0 for asset in assets),
        "A native archive is missing, empty, or not fully uploaded.",
    )


def wait_pause(deadline, message):
    require(
        time.monotonic() < deadline,
        f"Timed out: {message}. Rerun this same pinned command; do not move the tag.",
    )
    print(message, flush=True)
    time.sleep(30)


def wait_for_tag_run(candidate, tag, deadline):
    endpoint = f"repos/{REPO}/actions/workflows/release-binaries.yml/runs?event=push&head_sha={candidate}"
    while True:
        runs = [
            run
            for run in collection(endpoint, "workflow_runs")
            if run["head_sha"] == candidate
            and run["event"] == "push"
            and run["head_branch"] == tag
        ]
        if runs:
            run = max(runs, key=lambda value: value["id"])
            if run["status"] == "completed":
                require(
                    run["conclusion"] == "success",
                    f"Tag build failed: {run['html_url']}. No draft was published.",
                )
                jobs = collection(
                    f"repos/{REPO}/actions/runs/{run['id']}/jobs?filter=latest", "jobs"
                )
                validate_release_jobs(jobs)
                return run
            print(run["html_url"], flush=True)
        wait_pause(
            deadline,
            f"Waiting for the exact {tag} push build and all five packaged Julia tests",
        )


def public_json(url):
    request = urllib.request.Request(
        url,
        headers={
            "User-Agent": "PowerIO verified release helper (github.com/eigenergy/powerio)"
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise ReleaseError(
            f"Registry verification failed for {url}: HTTP {error.code}"
        ) from error


def published_pypi_version(document, version):
    if not document or document.get("info", {}).get("version") != version:
        return False
    files = [item for item in document.get("urls", []) if item.get("yanked") is False]
    return any(item.get("packagetype") == "bdist_wheel" for item in files) and any(
        item.get("packagetype") == "sdist" for item in files
    )


def published_crate_version(document, version):
    item = (document or {}).get("version", {})
    return item.get("num") == version and item.get("yanked") is False


def julia_registered_release(version, tag, julia_candidate):
    ref = api(f"repos/{JULIA_REPO}/git/ref/tags/{tag}", missing_ok=True)
    general = api(
        "repos/JuliaRegistries/General/contents/P/PowerIO/Versions.toml",
        missing_ok=True,
    )
    if ref is None or general is None:
        return False
    versions = base64.b64decode(general["content"]).decode()
    section = re.search(
        r'(?ms)^\["' + re.escape(version) + r'"\]\n(.*?)(?=^\[|\Z)', versions
    )
    if section is None:
        return False
    tree = re.search(r'git-tree-sha1 = "([0-9a-f]{40})"', section[1])
    require(tree is not None, "Julia registry entry lacks its immutable tree hash.")
    obj = ref["object"]
    if obj["type"] == "tag":
        obj = api(f"repos/{JULIA_REPO}/git/tags/{obj['sha']}")["object"]
    require(obj["type"] == "commit", "Unexpected Julia release tag target.")
    ancestry = api(f"repos/{JULIA_REPO}/compare/{julia_candidate}...{obj['sha']}")
    require(
        ancestry["status"] in ("ahead", "identical")
        and ancestry["base_commit"]["sha"] == julia_candidate,
        "Julia release is not descended from the reviewed companion commit.",
    )
    commit = api(f"repos/{JULIA_REPO}/git/commits/{obj['sha']}")
    require(
        commit["tree"]["sha"] == tree[1],
        "Julia tag and General registry trees disagree.",
    )
    project = contents(JULIA_REPO, "Project.toml", tag)
    require(
        re.search(r'^version = "' + re.escape(version) + r'"$', project, re.M)
        is not None,
        "Julia release tag has the wrong package version.",
    )
    artifacts = contents(JULIA_REPO, "Artifacts.toml", tag)
    for platform in PLATFORMS:
        require(
            f"/releases/download/{tag}/libpowerio_capi.{platform}.tar.gz" in artifacts,
            f"Julia tag does not pin the matching {platform} release archive.",
        )
    return True


def follow_publication(candidate, julia_candidate, version, tag, deadline):
    """Stop truthfully at required human approvals, never approve them ourselves."""
    while True:
        ready = True
        for workflow in ("crates.yml", "python.yml"):
            endpoint = f"repos/{REPO}/actions/workflows/{workflow}/runs?event=release&head_sha={candidate}"
            runs = [
                run
                for run in collection(endpoint, "workflow_runs")
                if run["event"] == "release"
                and run["head_sha"] == candidate
                and run.get("display_title", "")
                == (
                    f"PowerIO crates.io {tag}"
                    if workflow == "crates.yml"
                    else f"PowerIO Python {tag}"
                )
            ]
            if not runs:
                ready = False
                continue
            run = max(runs, key=lambda value: value["id"])
            pending = api(f"repos/{REPO}/actions/runs/{run['id']}/pending_deployments")
            if pending:
                names = ", ".join(item["environment"]["name"] for item in pending)
                print(
                    f"GitHub release is published; {names} needs maintainer approval: {run['html_url']}"
                )
                print(
                    "Approve the existing protected deployment in GitHub, then rerun this same command. No credentials or protection changes are needed."
                )
                return 3
            if run["status"] != "completed":
                ready = False
            else:
                require(
                    run["conclusion"] == "success",
                    f"GitHub release is published, but package publication failed: {run['html_url']}",
                )
        if ready:
            registries = published_pypi_version(
                public_json(f"https://pypi.org/pypi/powerio/{version}/json"), version
            )
            for crate in CRATES:
                registries = (
                    published_crate_version(
                        public_json(
                            f"https://crates.io/api/v1/crates/{crate}/{version}"
                        ),
                        version,
                    )
                ) and registries
            if registries and julia_registered_release(version, tag, julia_candidate):
                print(
                    f"Verified: {tag} GitHub release, eight crates, PyPI, Julia General registration and Julia tag are present."
                )
                return 0
        wait_pause(
            deadline,
            "GitHub release is published; waiting for crates/PyPI and Julia registration/TagBot (Julia's hourly release job is a fallback)",
        )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True)
    parser.add_argument("--julia-candidate", required=True)
    parser.add_argument("--version", default="0.11.6")
    parser.add_argument(
        "--timeout", type=int, default=10800, help="Total wait seconds; reruns are safe"
    )
    args = parser.parse_args(argv)
    require(
        re.fullmatch(r"[0-9a-f]{40}", args.candidate) is not None,
        "A full immutable Rust commit SHA is required.",
    )
    require(
        re.fullmatch(r"[0-9a-f]{40}", args.julia_candidate) is not None,
        "A full immutable Julia commit SHA is required.",
    )
    require(
        re.fullmatch(r"0\.11\.[0-9]+", args.version) is not None,
        "This helper is limited to the 0.11 patch line.",
    )
    require(args.timeout > 0, "Timeout must be positive.")
    require(
        subprocess.run(
            ["gh", "auth", "status", "--hostname", "github.com"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=30,
        ).returncode
        == 0,
        "Existing GitHub CLI authentication is required. This helper does not sign in or request credentials.",
    )
    actor = api("user")
    require(
        actor.get("type") == "User" and not actor.get("login", "").endswith("[bot]"),
        "Use existing human-user gh authentication; workflow/bot tokens can suppress release events.",
    )
    require(
        api(f"repos/{REPO}")["full_name"] == REPO, "Unexpected repository identity."
    )
    cargo = contents(REPO, "Cargo.toml", args.candidate)
    package = cargo.split("[workspace.package]", 1)[1].split("\n[", 1)[0]
    require(
        re.search(r'^version = "' + re.escape(args.version) + r'"$', package, re.M)
        is not None,
        "Pinned Cargo workspace version does not match the release.",
    )
    require(
        f"\n## {args.version}\n" in contents(REPO, "CHANGELOG.md", args.candidate),
        "Pinned changelog release heading is missing.",
    )
    require(
        contents(
            REPO, f"docs/release-notes/{args.version}.md", args.candidate
        ).startswith(f"# PowerIO {args.version} release notes"),
        "Pinned release notes are missing.",
    )
    validate_source_runs(
        collection(
            f"repos/{REPO}/actions/runs?event=push&head_sha={args.candidate}",
            "workflow_runs",
        ),
        args.candidate,
    )
    tag = f"v{args.version}"
    current = tag_commit(tag)
    require(
        current is None or current == args.candidate,
        "Existing tag points elsewhere; refusing to move or replace it.",
    )
    release = api(f"repos/{REPO}/releases/tags/{tag}", missing_ok=True)
    if release is None or release["draft"]:
        require(
            api(f"repos/{JULIA_REPO}/branches/main")["commit"]["sha"]
            == args.julia_candidate,
            "Julia main moved from the reviewed companion; obtain a fresh review before publishing.",
        )
    if current is None:
        require(
            release is None,
            "A release already exists without the expected tag; reconcile it manually.",
        )
        require(
            api(f"repos/{REPO}/branches/main")["commit"]["sha"] == args.candidate,
            "Rust main moved from the reviewed candidate; refusing to tag a different commit.",
        )
        obj = api(
            f"repos/{REPO}/git/tags",
            "POST",
            {"tag": tag, "message": tag, "object": args.candidate, "type": "commit"},
        )
        # No blind retries: on an uncertain write, rerun and inspect the ref first.
        api(
            f"repos/{REPO}/git/refs",
            "POST",
            {"ref": f"refs/tags/{tag}", "sha": obj["sha"]},
        )
    require(
        tag_commit(tag) == args.candidate,
        "Tag readback differs from the reviewed candidate.",
    )
    deadline = time.monotonic() + args.timeout
    run = wait_for_tag_run(args.candidate, tag, deadline)
    print(f"All tagged source/build/packaged gates passed: {run['html_url']}")
    release = api(f"repos/{REPO}/releases/tags/{tag}")
    validate_release(release, tag)
    require(
        tag_commit(tag) == args.candidate,
        "Tag moved during packaging; refusing publication.",
    )
    if release["draft"]:
        current_run = api(f"repos/{REPO}/actions/runs/{run['id']}")
        require(
            current_run["run_attempt"] == run["run_attempt"]
            and current_run["status"] == "completed"
            and current_run["conclusion"] == "success"
            and current_run["head_sha"] == args.candidate
            and current_run["event"] == "push"
            and current_run["head_branch"] == tag,
            "Tag build changed or was rerun after validation; rerun this command after it passes.",
        )
        require(
            api(f"repos/{JULIA_REPO}/branches/main")["commit"]["sha"]
            == args.julia_candidate,
            "Julia main moved during packaging; refusing publication pending review.",
        )
        api(
            f"repos/{REPO}/releases/{release['id']}",
            "PATCH",
            {"draft": False, "make_latest": "true"},
        )
    release = api(f"repos/{REPO}/releases/tags/{tag}")
    validate_release(release, tag)
    require(
        not release["draft"] and release.get("published_at"),
        "Publication was not confirmed; inspect the release before retrying.",
    )
    print(f"GitHub release publication confirmed: {release['html_url']}", flush=True)
    return follow_publication(
        args.candidate, args.julia_candidate, args.version, tag, deadline
    )


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (
        ReleaseError,
        OSError,
        ValueError,
        KeyError,
        IndexError,
        subprocess.TimeoutExpired,
    ) as error:
        print(f"STOP: {error}", file=sys.stderr)
        sys.exit(1)
