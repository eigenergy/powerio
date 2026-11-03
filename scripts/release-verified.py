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
from pathlib import Path

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
# The account that owns every published PowerIO crate. A crate name that
# exists under another owner was claimed by someone else.
CRATE_OWNER = "samtalki"
CRATES = (
    "powerio-core",
    "powerio-tx",
    "powerio-dist",
    "powerio-tnd",
    "powerio-prob",
    "powerio-synth",
    "powerio-matrix",
    "powerio",
    "powerio-cli",
)


_REPORTED = {}


def report(key, message):
    """Show every state change once, without repeating unchanged polling output."""
    if _REPORTED.get(key) != message:
        print(message, flush=True)
        _REPORTED[key] = message


def report_run(run):
    report(
        f"run:{run['id']}",
        f"{run.get('name', run.get('display_title', 'Workflow'))} "
        f"(attempt {run.get('run_attempt', 1)}): "
        f"{run.get('conclusion') or run['status']} | {run['html_url']}",
    )


def report_jobs(run):
    jobs = collection(
        f"repos/{REPO}/actions/runs/{run['id']}/jobs?filter=latest", "jobs"
    )
    for job in jobs:
        report(
            f"job:{job['id']}",
            f"  {job['name']}: {job.get('conclusion') or job['status']} "
            f"| {job.get('html_url', run['html_url'])}",
        )
    return jobs


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


def collection(path, key=None):
    result = []
    for page in range(1, 101):
        sep = "&" if "?" in path else "?"
        batch = api(f"{path}{sep}per_page=100&page={page}")
        if key is not None:
            batch = batch[key]
        result.extend(batch)
        if len(batch) < 100:
            return result
    raise ReleaseError(
        "Refusing an unexpectedly large API result; nothing was inferred."
    )


def release_for_tag(tag):
    """The draft or published release for tag, or None.

    GET releases/tags/{tag} never returns a draft, so the draft this helper
    publishes is found by listing releases.
    """
    matches = [
        release
        for release in collection(f"repos/{REPO}/releases")
        if release["tag_name"] == tag
    ]
    require(
        len(matches) <= 1,
        f"Several releases claim {tag}; reconcile them manually.",
    )
    return matches[0] if matches else None


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
        report_run(run)
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
    report("wait", message)
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
            report_run(run)
            jobs = report_jobs(run)
            if run["status"] == "completed":
                require(
                    run["conclusion"] == "success",
                    f"Tag build failed: {run['html_url']}. No draft was published.",
                )
                validate_release_jobs(jobs)
                return run
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
    project = contents(JULIA_REPO, "Project.toml", obj["sha"])
    require(
        re.search(r'^version = "' + re.escape(version) + r'"$', project, re.M)
        is not None,
        "Julia release tag has the wrong package version.",
    )
    artifacts = contents(JULIA_REPO, "Artifacts.toml", obj["sha"])
    for platform in PLATFORMS:
        require(
            f"https://github.com/{REPO}/releases/download/{tag}/libpowerio_capi.{platform}.tar.gz"
            in artifacts,
            f"Julia tag does not pin the matching {platform} release archive.",
        )
    return True


def require_clean_candidate(candidate):
    """Local first publication must use the exact reviewed source tree."""
    root = Path(__file__).resolve().parent.parent
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=root,
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    status = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=root,
        text=True,
        capture_output=True,
        check=True,
    ).stdout.strip()
    require(
        head == candidate and not status,
        "First publication requires a clean local checkout of the exact candidate.",
    )
    return root


def require_crate_ownership():
    """Refuse a release whose crate names someone else has claimed.

    Run before the tag and again before the draft is published, so a
    squatted new name stops the release before any crate is published.
    """
    for crate in CRATES:
        owners = public_json(f"https://crates.io/api/v1/crates/{crate}/owners")
        if owners is None:
            continue
        logins = {user.get("login") for user in owners.get("users", [])}
        require(
            CRATE_OWNER in logins,
            f"{crate} on crates.io is not owned by {CRATE_OWNER}; refusing to publish under a claimed name.",
        )


def bootstrap_first_missing(candidate, version, allowed):
    """Publish only an explicitly named, unclaimed crate after its dependencies."""
    already_bootstrapped = False
    for crate in CRATES:
        if published_crate_version(
            public_json(f"https://crates.io/api/v1/crates/{crate}/{version}"), version
        ):
            already_bootstrapped = already_bootstrapped or crate in allowed
            continue
        if crate not in allowed:
            require(
                already_bootstrapped,
                f"Predecessor {crate} {version} is not live; inspect the protected publishing job.",
            )
            return False
        require(
            public_json(f"https://crates.io/api/v1/crates/{crate}") is None,
            f"{crate} already exists; refusing local publication of an existing crate.",
        )
        root = require_clean_candidate(candidate)
        subprocess.run(
            # Every feature compiles from the packaged tarball before the
            # new name is permanent on crates.io.
            ["cargo", "publish", "--locked", "--all-features", "-p", crate],
            cwd=root,
            check=True,
        )
        require(
            published_crate_version(
                public_json(f"https://crates.io/api/v1/crates/{crate}/{version}"),
                version,
            ),
            f"First publication of {crate} was not verified in the registry.",
        )
        print(
            f"Published {crate} {version}. For future releases, configure its crates.io "
            "trusted publisher: owner eigenergy, repository powerio, workflow crates.yml, "
            "environment crates-io.",
            flush=True,
        )
        return True
    return False


def bootstrap_after_approved_failure(run, jobs, candidate, version, allowed):
    """Reaching the publish step proves protected approval and archive gates ran."""
    require(
        run.get("conclusion") == "failure"
        and any(
            step.get("name") == "Publish in dependency order"
            and step.get("conclusion") == "failure"
            for job in jobs
            for step in job.get("steps", [])
        ),
        "First publication requires a failure in the approved crates publishing step.",
    )
    return bootstrap_first_missing(candidate, version, allowed)


def follow_publication(
    candidate, julia_candidate, version, tag, deadline, bootstrap=()
):
    """Stop truthfully at required human approvals, never approve them ourselves."""
    retried = set()
    no_publish_retry = set()
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
            report_run(run)
            jobs = report_jobs(run)
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
            elif run["conclusion"] != "success":
                key = (run["id"], run.get("run_attempt", 1))
                if key in retried:
                    ready = False
                    continue
                require(
                    workflow == "crates.yml" and bootstrap,
                    f"GitHub release is published, but package publication failed: {run['html_url']}",
                )
                published = bootstrap_after_approved_failure(
                    run, jobs, candidate, version, bootstrap
                )
                require(
                    published or run["id"] not in no_publish_retry,
                    f"Package publication still fails after bootstrap; inspect {run['html_url']}",
                )
                if not published:
                    no_publish_retry.add(run["id"])
                api(f"repos/{REPO}/actions/runs/{run['id']}/rerun-failed-jobs", "POST")
                retried.add(key)
                ready = False
        if ready:
            registries = published_pypi_version(
                public_json(f"https://pypi.org/pypi/powerio/{version}/json"), version
            )
            report(
                "registry:pypi",
                f"PyPI {version}: {'verified' if registries else 'pending'} | https://pypi.org/project/powerio/{version}/",
            )
            for crate in CRATES:
                present = published_crate_version(
                    public_json(f"https://crates.io/api/v1/crates/{crate}/{version}"),
                    version,
                )
                report(
                    f"registry:{crate}",
                    f"crates.io {crate} {version}: {'verified' if present else 'pending'} | https://crates.io/crates/{crate}/{version}",
                )
                registries = present and registries
            julia = registries and julia_registered_release(
                version, tag, julia_candidate
            )
            report(
                "registry:julia",
                f"Julia General/TagBot {version}: {'verified' if julia else 'pending'} | https://github.com/{JULIA_REPO}/actions",
            )
            if julia:
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
    parser.add_argument(
        "--version",
        required=True,
        help="Stable release version; must match the reviewed candidate",
    )
    parser.add_argument(
        "--timeout", type=int, default=10800, help="Total wait seconds; reruns are safe"
    )
    parser.add_argument(
        "--bootstrap-crate",
        action="append",
        default=[],
        choices=CRATES,
        help="First-publish this new crate using existing local Cargo authentication after protected release gates",
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
        re.fullmatch(
            r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", args.version
        )
        is not None,
        "Use an explicit stable MAJOR.MINOR.PATCH version.",
    )
    require(args.timeout > 0, "Timeout must be positive.")
    print(
        f"Release {args.version} | Rust {args.candidate} | Julia {args.julia_candidate}",
        flush=True,
    )
    print(
        "Phase 1: verify existing authentication, candidate identity and source CI.",
        flush=True,
    )
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
    if args.bootstrap_crate:
        require_clean_candidate(args.candidate)
    missing = [
        crate
        for crate in CRATES
        if public_json(f"https://crates.io/api/v1/crates/{crate}") is None
    ]
    require(
        set(missing) <= set(args.bootstrap_crate),
        f"New crates require explicit --bootstrap-crate flags: {missing}",
    )
    require_crate_ownership()
    tag = f"v{args.version}"
    print(
        f"Phase 2: reconcile annotated tag {tag}; never move an existing tag.",
        flush=True,
    )
    current = tag_commit(tag)
    require(
        current is None or current == args.candidate,
        "Existing tag points elsewhere; refusing to move or replace it.",
    )
    release = release_for_tag(tag)
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
    print(
        "Phase 3: wait for exact tag build, native archives and five packaged Julia tests.",
        flush=True,
    )
    deadline = time.monotonic() + args.timeout
    run = wait_for_tag_run(args.candidate, tag, deadline)
    print(f"All tagged source/build/packaged gates passed: {run['html_url']}")
    release = release_for_tag(tag)
    require(release is not None, f"The tag build passed but no {tag} release exists.")
    validate_release(release, tag)
    require(
        tag_commit(tag) == args.candidate,
        "Tag moved during packaging; refusing publication.",
    )
    print("Phase 4: reconcile draft and publish only after verified gates.", flush=True)
    if release["draft"]:
        require_crate_ownership()
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
    release = api(f"repos/{REPO}/releases/{release['id']}")
    validate_release(release, tag)
    require(
        not release["draft"] and release.get("published_at"),
        "Publication was not confirmed; inspect the release before retrying.",
    )
    print(f"GitHub release publication confirmed: {release['html_url']}", flush=True)
    print(
        "Phase 5: follow crates/PyPI publication and Julia registration/TagBot.",
        flush=True,
    )
    return follow_publication(
        args.candidate,
        args.julia_candidate,
        args.version,
        tag,
        deadline,
        args.bootstrap_crate,
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
        subprocess.CalledProcessError,
    ) as error:
        print(f"STOP: {error}", file=sys.stderr)
        sys.exit(1)
