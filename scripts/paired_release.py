#!/usr/bin/env python3
"""Prepare and verify paired PowerIO releases using exact commits and assets."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import tomllib

POWERIO = "eigenergy/powerio"
JULIA = "eigenergy/PowerIO.jl"
ASSETS = {f"libpowerio_capi.{p}.tar.gz" for p in (
    "aarch64-apple-darwin", "aarch64-linux-gnu", "x86_64-apple-darwin",
    "x86_64-linux-gnu", "x86_64-w64-mingw32",
)}
MANIFEST = "release-manifest.json"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def run(*args, cwd=None, input=None):
    return subprocess.run(args, cwd=cwd, input=input, check=True,
                          capture_output=True, text=True).stdout.strip()


def api(path, data=None, *, missing=False):
    command = ["gh", "api", path]
    if data is not None:
        command += ["--input", "-"]
    result = subprocess.run(command, input=json.dumps(data) if data is not None else None,
                            capture_output=True, text=True)
    if result.returncode:
        if missing and "HTTP 404" in result.stderr:
            return None
        raise RuntimeError(f"GitHub request failed: {path}: {result.stderr.strip()}")
    return json.loads(result.stdout) if result.stdout.strip() else None


def pages(path):
    values = json.loads(run("gh", "api", "--paginate", "--slurp", path))
    return [item for page in values for item in page]


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def version(value):
    require(re.fullmatch(r"0|[1-9][0-9]*", value.split('.')[0]) and
            re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value),
            "version must be X.Y.Z")
    return tuple(map(int, value.split('.')))


def commit_sha(value):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{40}", value), "invalid commit SHA")
    return value


def source(repo, path, ref):
    item = api(f"repos/{repo}/contents/{path}?ref={ref}")
    require(item.get("encoding") == "base64", f"unsupported source response: {path}")
    return base64.b64decode(item["content"])


def notes(text, number):
    sections = re.split(r"^## ", text, flags=re.M)
    matches = [part.partition('\n')[2].strip() for part in sections[1:]
               if part.partition('\n')[0] == number]
    require(len(matches) == 1 and matches[0], f"missing unique changelog section {number}")
    require("REVIEW REQUIRED" not in matches[0], "release notes still need review")
    return matches[0]


def identity(repo, sha, number):
    path = "Cargo.toml" if repo == POWERIO else "Project.toml"
    data = tomllib.loads(source(repo, path, sha).decode())
    actual = data["workspace"]["package"]["version"] if repo == POWERIO else data["version"]
    require(actual == number, f"{repo} version {actual} differs from {number}")
    return notes(source(repo, "CHANGELOG.md", sha).decode(), number)


def successful_ci(repo, sha):
    checks = api(f"repos/{repo}/commits/{sha}/check-runs?per_page=100")["check_runs"]
    require(checks and len(checks) < 100, f"missing or truncated CI results for {repo}")
    require(all(c["status"] == "completed" and c["conclusion"] in ("success", "neutral", "skipped")
                for c in checks), f"CI has not passed for {repo}@{sha}")
    require(any(c["conclusion"] == "success" for c in checks), "CI has no successful check")
    return sorted({c["html_url"] for c in checks})


def tag_pair(tag):
    require(tag.startswith('v'), "tag must start with v")
    version(tag[1:])
    ref = api(f"repos/{POWERIO}/git/ref/tags/{tag}")
    require(ref["object"]["type"] == "tag", "paired releases require an annotated tag")
    obj = api(f"repos/{POWERIO}/git/tags/{ref['object']['sha']}")
    pair = json.loads(obj["message"])
    require(pair["format"] == 1 and pair["tag"] == tag, "invalid paired tag annotation")
    require(pair["powerio_sha"] == obj["object"]["sha"] and obj["object"]["type"] == "commit",
            "tag differs from frozen PowerIO source")
    commit_sha(pair["powerio_sha"])
    commit_sha(pair["julia_source_sha"])
    return pair


def create_tag(number):
    version(number)
    tag = 'v' + number
    require(api(f"repos/{POWERIO}/git/ref/tags/{tag}", missing=True) is None,
            "tag already exists; retry that candidate instead")
    pair = {"format": 1, "tag": tag}
    for repo, key in ((POWERIO, "powerio_sha"), (JULIA, "julia_source_sha")):
        sha = api(f"repos/{repo}/git/ref/heads/main")["object"]["sha"]
        pair[key] = sha
        identity(repo, sha, number)
        successful_ci(repo, sha)
        require(json.loads(source(repo, ".github/paired-release.json", sha))["format"] == 1,
                f"paired release support is not enabled in {repo}")
    for repo, key in ((POWERIO, "powerio_sha"), (JULIA, "julia_source_sha")):
        require(api(f"repos/{repo}/git/ref/heads/main")["object"]["sha"] == pair[key],
                "main advanced during preparation; retry preparation")
    obj = api(f"repos/{POWERIO}/git/tags", {"tag": tag, "message": json.dumps(pair, sort_keys=True),
              "object": pair["powerio_sha"], "type": "commit"})
    api(f"repos/{POWERIO}/git/refs", {"ref": f"refs/tags/{tag}", "sha": obj["sha"]})
    print(f"Prepared {tag}; building a draft for paired review")


def release(tag):
    return api(f"repos/{POWERIO}/releases/tags/{tag}", missing=True)


def download(tag, name, dest):
    run("gh", "release", "download", tag, "--repo", POWERIO, "--pattern", name,
        "--output", str(dest), "--clobber")


def asset_map(release_data, *, complete):
    assets = release_data["assets"]
    result = {a["name"]: a for a in assets}
    allowed = ASSETS | ({MANIFEST} if complete else set())
    require(len(result) == len(assets) and set(result) == allowed, "release asset set is incomplete or unexpected")
    return result


def validate_manifest(manifest, pair, assets):
    require(manifest["format"] == 1 and manifest["tag"] == pair["tag"], "manifest identity mismatch")
    require(manifest["powerio_sha"] == pair["powerio_sha"] and
            manifest["julia_source_sha"] == pair["julia_source_sha"], "manifest source pairing changed")
    commit_sha(manifest["julia_sha"])
    commit_sha(manifest["julia_tree_sha"])
    require(set(manifest["assets"]) == ASSETS, "manifest platform set mismatch")
    for name, digest in manifest["assets"].items():
        require(re.fullmatch(r"[0-9a-f]{64}", digest), "invalid asset SHA-256")
        require(assets[name].get("digest") == 'sha256:' + digest, f"asset digest mismatch: {name}")
    require(manifest["validation"]["status"] == "passed" and
            re.fullmatch(r"https://github.com/eigenergy/powerio/actions/runs/[0-9]+", manifest["validation"]["url"]),
            "missing candidate validation evidence")


def verify(tag, *, published=True):
    pair = tag_pair(tag)
    rel = release(tag)
    require(rel is not None and not rel["prerelease"], "stable release is missing")
    if published:
        require(not rel["draft"] and rel.get("immutable") is True, "release must be published and immutable")
    assets = asset_map(rel, complete=True)
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / MANIFEST
        download(tag, MANIFEST, path)
        manifest = json.loads(path.read_bytes())
    validate_manifest(manifest, pair, assets)
    candidate = api(f"repos/{JULIA}/git/commits/{manifest['julia_sha']}")
    require(candidate["tree"]["sha"] == manifest["julia_tree_sha"], "Julia candidate tree mismatch")
    require([p["sha"] for p in candidate["parents"]] == [pair["julia_source_sha"]], "unexpected candidate parent")
    diff = api(f"repos/{JULIA}/compare/{pair['julia_source_sha']}...{manifest['julia_sha']}")
    require([f["filename"] for f in diff["files"]] == ["Artifacts.toml"], "Julia candidate changes more than artifacts")
    artifact = source(JULIA, "Artifacts.toml", manifest["julia_sha"])
    require(sha256(artifact) == manifest["artifacts_sha256"], "artifact file changed")
    validate_artifacts(artifact, tag, manifest["assets"])
    require(sha256(identity(POWERIO, pair["powerio_sha"], tag[1:]).encode()) == manifest["notes_sha256"]["powerio"],
            "PowerIO notes mismatch")
    require(sha256(identity(JULIA, pair["julia_source_sha"], tag[1:]).encode()) == manifest["notes_sha256"]["julia"],
            "Julia notes mismatch")
    require(sha256(source(POWERIO, "powerio-dist/schemas/bmopf/manifest.json", pair["powerio_sha"])) == manifest["schemas_sha256"], "schema archive manifest mismatch")
    return manifest


def validate_artifacts(content, tag, hashes):
    stanzas = tomllib.loads(content.decode())["powerio_capi"]
    downloads = [d for item in stanzas for d in item["download"]]
    require(len(stanzas) == len(downloads) == len(ASSETS), "artifact platform count mismatch")
    found = {}
    for d in downloads:
        prefix = f"https://github.com/{POWERIO}/releases/download/{tag}/"
        require(d["url"].startswith(prefix), "artifact URL names another release")
        name = d["url"][len(prefix):]
        require(name not in found, "duplicate artifact download")
        found[name] = d["sha256"]
    require(found == hashes, "artifact hashes differ from draft binaries")


def complete_candidate(tag, julia_root):
    pair = tag_pair(tag)
    rel = release(tag)
    require(rel and not rel["prerelease"], "candidate requires a stable release")
    if any(a["name"] == MANIFEST for a in rel["assets"]):
        verify(tag, published=not rel["draft"])
        print("Candidate already prepared; no assets or commits changed")
        return
    require(rel["draft"], "unprepared published release cannot be completed")
    assets = asset_map(rel, complete=False)
    hashes = {}
    for name, item in assets.items():
        digest = item.get("digest", "")
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", digest), f"missing GitHub asset digest: {name}")
        hashes[name] = digest[7:]
    root = Path(julia_root)
    require(run("git", "rev-parse", "HEAD", cwd=root) == pair["julia_source_sha"], "wrong Julia source checkout")
    require(run("git", "diff", "--name-only", cwd=root).splitlines() == ["Artifacts.toml"],
            "preparation must change only Artifacts.toml")
    artifact = (root / "Artifacts.toml").read_bytes()
    validate_artifacts(artifact, tag, hashes)
    source_commit = api(f"repos/{JULIA}/git/commits/{pair['julia_source_sha']}")
    blob = api(f"repos/{JULIA}/git/blobs", {"content": base64.b64encode(artifact).decode(), "encoding": "base64"})
    tree = api(f"repos/{JULIA}/git/trees", {"base_tree": source_commit["tree"]["sha"], "tree": [
        {"path": "Artifacts.toml", "mode": "100644", "type": "blob", "sha": blob["sha"]}]})
    branch = f"release-candidates/{tag}"
    existing = api(f"repos/{JULIA}/git/ref/heads/{branch}", missing=True)
    if existing:
        candidate = api(f"repos/{JULIA}/git/commits/{existing['object']['sha']}")
        require(candidate["tree"]["sha"] == tree["sha"] and
                [p["sha"] for p in candidate["parents"]] == [pair["julia_source_sha"]],
                "existing candidate differs; do not replace it")
    else:
        candidate = api(f"repos/{JULIA}/git/commits", {"message": f"release: pin PowerIO {tag}",
                        "tree": tree["sha"], "parents": [pair["julia_source_sha"]]})
        api(f"repos/{JULIA}/git/refs", {"ref": f"refs/heads/{branch}", "sha": candidate["sha"]})
    pnotes = identity(POWERIO, pair["powerio_sha"], tag[1:])
    jnotes = identity(JULIA, pair["julia_source_sha"], tag[1:])
    manifest = dict(pair, julia_sha=candidate["sha"], julia_tree_sha=tree["sha"],
                    artifacts_sha256=sha256(artifact), assets=hashes,
                    schemas_sha256=sha256(source(POWERIO, "powerio-dist/schemas/bmopf/manifest.json", pair["powerio_sha"])),
                    notes_sha256={"powerio": sha256(pnotes.encode()), "julia": sha256(jnotes.encode())},
                    validation={"status": "passed", "url": os.environ["VALIDATION_URL"]})
    validate_manifest(manifest, pair, assets)
    body = (f"{pnotes}\n\n## PowerIO.jl {tag[1:]}\n\n{jnotes}\n\n## Paired release review\n\n"
            f"PowerIO commit: `{pair['powerio_sha']}`\n\nJulia commit: `{candidate['sha']}`\n\n"
            f"[Candidate validation]({manifest['validation']['url']}) passed. "
            "The attached release-manifest.json records the exact commits, notes, and binary hashes. "
            "Publishing this draft approves both packages and starts registry publication.\n")
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / MANIFEST
        path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n')
        run("gh", "release", "edit", tag, "--repo", POWERIO, "--notes", body)
        run("gh", "release", "upload", tag, str(path), "--repo", POWERIO)
    print(f"{tag} is ready for paired review and publication")


def registry_action(manifest, versions):
    number = manifest["tag"][1:]
    target = version(number)
    if number in versions:
        require(versions[number]["git-tree-sha1"] == manifest["julia_tree_sha"],
                "registered version contains a different Julia tree")
        require(not versions[number].get("yanked", False), "registered version is yanked")
        return "registered"
    require(versions, "General registry returned no versions")
    latest = max(version(v) for v in versions)
    major, minor, patch = latest
    require(target in ((major, minor, patch + 1), (major, minor + 1, 0), (major + 1, 0, 0)),
            "Julia version is not the next patch, minor, or major")
    return "register"


def register(tag):
    manifest = verify(tag)
    repair_publications(tag, manifest)
    registry = tomllib.loads(source("JuliaRegistries/General", "P/PowerIO/Versions.toml", "master").decode())
    action = registry_action(manifest, registry)
    if action == "registered":
        jl_release = api(f"repos/{JULIA}/releases/tags/{tag}", missing=True)
        if not jl_release:
            run("gh", "workflow", "run", "TagBot.yml", "--repo", JULIA, "--ref", "main")
            print(f"{tag}: registered; waiting for TagBot")
        else:
            print(f"{tag}: registered and Julia release exists")
        return
    body_notes = identity(JULIA, manifest["julia_source_sha"], tag[1:])
    major, minor, _ = max(version(v) for v in registry)
    if version(tag[1:])[:2] != (major, minor):
        require("breaking" in body_notes.lower(), "minor/major transition requires explicit compatibility notes")
    sha = manifest["julia_sha"]
    comments = pages(f"repos/{JULIA}/commits/{sha}/comments?per_page=100")
    cutoff = dt.datetime.now(dt.timezone.utc) - dt.timedelta(hours=6)
    recent = any(c["body"].startswith("@JuliaRegistrator register") and
                 dt.datetime.fromisoformat(c["created_at"].replace('Z', '+00:00')) > cutoff for c in comments)
    if not recent:
        api(f"repos/{JULIA}/commits/{sha}/comments", {"body": f"@JuliaRegistrator register\n\nRelease notes:\n{body_notes}"})
    prs = api(f"repos/{JULIA}/pulls?state=all&head=eigenergy:release-candidates/{tag}&base=main")
    if not prs:
        api(f"repos/{JULIA}/pulls", {"title": f"release: synchronize {tag} artifacts", "head": f"release-candidates/{tag}",
            "base": "main", "body": f"Synchronize the exact artifact references tested and approved in the PowerIO {tag} paired release. Registration uses commit `{sha}` independently of later main changes."})
    print(f"{tag}: waiting for General registration of {sha}; scheduled retries remain active")



def repair_publications(tag, manifest):
    for workflow in ("crates.yml", "python.yml"):
        runs = api(f"repos/{POWERIO}/actions/workflows/{workflow}/runs?per_page=100")["workflow_runs"]
        relevant = [r for r in runs if r["display_title"].endswith(" " + tag) and r["event"] in ("release", "workflow_dispatch")]
        if any(r["status"] != "completed" or r["conclusion"] == "success" for r in relevant):
            continue
        run("gh", "workflow", "run", workflow, "--repo", POWERIO, "--ref", "main", "-f", "tag=" + tag)


def registration_status(tag):
    manifest = verify(tag)
    versions = tomllib.loads(source("JuliaRegistries/General", "P/PowerIO/Versions.toml", "master").decode())
    action = registry_action(manifest, versions)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"julia_sha={manifest['julia_sha']}\n")
            output.write(f"action={action}\n")
    print(f"{tag}: {action}")


def emit_pair(tag):
    pair = tag_pair(tag)
    print(json.dumps(pair))
    rel = release(tag)
    prepared = bool(rel and any(a["name"] == MANIFEST for a in rel["assets"]))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], 'a') as output:
            output.write(f"prepared={str(prepared).lower()}\n")
            for key in ("powerio_sha", "julia_source_sha"):
                output.write(f"{key}={pair[key]}\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["tag", "metadata", "complete", "verify", "register", "status"])
    parser.add_argument("version_or_tag")
    parser.add_argument("--julia-root", default="PowerIO.jl")
    args = parser.parse_args()
    if args.command == "tag":
        create_tag(args.version_or_tag)
    elif args.command == "metadata":
        emit_pair(args.version_or_tag)
    elif args.command == "complete":
        complete_candidate(args.version_or_tag, args.julia_root)
    elif args.command == "status":
        registration_status(args.version_or_tag)
    elif args.command == "verify":
        print(json.dumps(verify(args.version_or_tag)))
    else:
        register(args.version_or_tag)


if __name__ == "__main__":
    main()
