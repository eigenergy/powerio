#!/usr/bin/env python3
"""Open version and changelog preparation PRs without choosing release semantics."""

import argparse
import base64
import copy
import json
import re

import tomllib
from paired_release import JULIA, POWERIO, api, require, source, version


def bump_workspace_versions(text, number):
    parsed = tomllib.loads(text)['workspace']
    old = parsed['package']['version']
    match = re.search(r'(?ms)^\[workspace.package\]\n(.*?)(?=^\[|\Z)', text)
    require(match is not None, 'workspace package section is missing')
    section, count = re.subn(r'(?m)^version = "' + re.escape(old) + r'"$', 'version = "' + number + '"', match[1])
    require(count == 1, 'workspace version declaration is ambiguous')
    text = text[:match.start(1)] + section + text[match.end(1):]
    for name, dependency in parsed['dependencies'].items():
        if isinstance(dependency, dict) and 'path' in dependency and dependency.get('version') == old:
            pattern = r'(?m)^(' + re.escape(name) + r' = \{[^\n]*version = ")' + re.escape(old) + r'("[^\n]*\})$'
            text, count = re.subn(pattern, lambda m: m[1] + number + m[2], text)
            require(count == 1, 'workspace dependency declaration is ambiguous: ' + name)
    return text


def bump_lock_versions(lock, names, number):
    for name in names:
        pattern = r'(name = "' + re.escape(name) + r'"\nversion = ")[^"]+("\n)'
        lock, count = re.subn(pattern, lambda m: m[1] + number + m[2], lock)
        require(count == 1, f'missing or ambiguous workspace package {name}')
    return lock



def example_metadata(text, number, *, retain_history):
    start = re.search(r'  "meta": ', text).end()
    meta, length = json.JSONDecoder().raw_decode(text[start:])
    require(meta['case_study_generator']['tool'] == 'powerio', 'example has another producer')
    meta['case_study_generator']['version'] = number
    provenance = meta['provenance']
    keys = [k for k in provenance if re.fullmatch(r'powerio_bmopf(?:_[0-9]+)?', k)]
    keys.sort(key=lambda k: 0 if k == 'powerio_bmopf' else int(k.rsplit('_', 1)[1]))
    record = copy.deepcopy(provenance[keys[-1]])
    record['producer_version'] = number
    if not retain_history:
        provenance['powerio_bmopf'] = record
    elif record not in provenance.values():
        index = 0
        while ('powerio_bmopf' if index == 0 else f'powerio_bmopf_{index}') in provenance:
            index += 1
        provenance['powerio_bmopf' if index == 0 else f'powerio_bmopf_{index}'] = record
    encoded = json.dumps(meta, indent=2, sort_keys=True, ensure_ascii=False).replace('\n', '\n  ')
    return text[:start] + encoded + text[start + length:]


def version_edits(repo, sha, number):
    if repo == POWERIO:
        cargo = source(repo, 'Cargo.toml', sha).decode()
        old = tomllib.loads(cargo)['workspace']['package']['version']
        require(version(number) > version(old), 'release version must increase')
        cargo = bump_workspace_versions(cargo, number)
        lock = source(repo, 'Cargo.lock', sha).decode()
        names = []
        for member in tomllib.loads(cargo)['workspace']['members']:
            manifest = tomllib.loads(source(repo, member + '/Cargo.toml', sha).decode())['package']
            if manifest['version'] == {'workspace': True}:
                names.append(manifest['name'])
        lock = bump_lock_versions(lock, names, number)
        result = {'Cargo.toml': cargo, 'Cargo.lock': lock}
        readme_path = 'powerio-dist/examples/bmopf/README.md'
        readme = source(repo, readme_path, sha).decode()
        for name in ('ieee34.json', 'ieee123.json', '4bus_dy.json'):
            path = 'powerio-dist/examples/bmopf/' + name
            before = source(repo, path, sha).decode()
            after = example_metadata(before, number, retain_history=name == '4bus_dy.json')
            result[path] = after
            readme = readme.replace(f'{len(before.encode()):,} bytes', f'{len(after.encode()):,} bytes')
        result[readme_path] = readme
        for name in ('case9_arrow_coo.json', 'case30_arrow_coo.json'):
            path = 'tests/data/capi_matrix/' + name
            before = source(repo, path, sha).decode()
            result[path] = before.replace(f'"powerio_version": "{old}"', f'"powerio_version": "{number}"')
        result[f'docs/release-notes/{number}-draft.md'] = (
            f'# PowerIO {number} release notes\n\n'
            f'The [curated changelog](../../CHANGELOG.md#{number.replace(".", "")}) '
            'records this release. Review that section before candidate preparation.\n')
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
