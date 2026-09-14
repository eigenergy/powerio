#!/usr/bin/env python3
"""Open version and changelog preparation PRs without choosing release semantics."""

import argparse
import base64
import re
import tomllib

from paired_release import JULIA, POWERIO, api, require, source, version


def version_edits(repo, sha, number):
    if repo == POWERIO:
        cargo = source(repo, 'Cargo.toml', sha).decode()
        old = tomllib.loads(cargo)['workspace']['package']['version']
        require(version(number) > version(old), 'release version must increase')
        cargo = cargo.replace(f'version = "{old}"', f'version = "{number}"')
        lock = source(repo, 'Cargo.lock', sha).decode()
        lock = re.sub(r'(name = "powerio(?:-[^"]+)?"\nversion = ")[^"]+("\n)',
                      lambda m: m[1] + number + m[2], lock)
        result = {'Cargo.toml': cargo, 'Cargo.lock': lock}
    else:
        project = source(repo, 'Project.toml', sha).decode()
        old = tomllib.loads(project)['version']
        require(version(number) > version(old), 'release version must increase')
        result = {'Project.toml': project.replace(f'version = "{old}"', f'version = "{number}"', 1)}
    changelog = source(repo, 'CHANGELOG.md', sha).decode()
    position = changelog.index('## ')
    require(f'## {number}\n' not in changelog, 'release changelog already exists')
    result['CHANGELOG.md'] = changelog[:position] + f'## {number}\n\n- REVIEW REQUIRED: write the curated release notes and compatibility assessment.\n\n' + changelog[position:]
    return result


def propose(repo, number):
    branch = f'release/{number}'
    existing = api(f'repos/{repo}/pulls?state=all&head=eigenergy:{branch}&base=main')
    if existing:
        print(existing[0]['html_url'])
        return
    base = api(f'repos/{repo}/git/ref/heads/main')['object']['sha']
    require(api(f'repos/{repo}/git/ref/heads/{branch}', missing=True) is None,
            f'{repo} already has {branch}; inspect it before retrying')
    edits = version_edits(repo, base, number)
    tree = api(f'repos/{repo}/git/commits/{base}')['tree']['sha']
    entries = []
    for path, text in edits.items():
        blob = api(f'repos/{repo}/git/blobs', {'encoding': 'base64', 'content': base64.b64encode(text.encode()).decode()})
        entries.append({'path': path, 'mode': '100644', 'type': 'blob', 'sha': blob['sha']})
    tree = api(f'repos/{repo}/git/trees', {'base_tree': tree, 'tree': entries})['sha']
    commit = api(f'repos/{repo}/git/commits', {'message': f'release: prepare {number}', 'tree': tree, 'parents': [base]})['sha']
    api(f'repos/{repo}/git/refs', {'ref': f'refs/heads/{branch}', 'sha': commit})
    pr = api(f'repos/{repo}/pulls', {'title': f'release: prepare {number}', 'head': branch, 'base': 'main', 'draft': True,
             'body': f'Prepare version {number} for the paired PowerIO release. Complete and review the changelog before merging. The matching `release/{number}` branch in the companion repository supplies the paired version change.\n\nMerging this PR does not publish packages. Candidate preparation runs only after both reviewed PRs merge and CI passes. Publishing the resulting draft release is the paired publication approval.'})
    print(pr['html_url'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('version')
    args = parser.parse_args()
    version(args.version)
    for repo in (JULIA, POWERIO):
        propose(repo, args.version)
