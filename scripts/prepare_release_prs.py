#!/usr/bin/env python3
"""Open version and changelog preparation PRs without choosing release semantics."""

import argparse
import base64
import re
import tempfile
from pathlib import Path

import tomllib
from paired_release import JULIA, POWERIO, api, notes, require, run, source, version


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



def promote_changelog(text, number):
    require(not re.search(r'^## ' + re.escape(number) + r'$', text, re.M),
            'release changelog already exists')
    first = re.search(r'^## (.+)$', text, re.M)
    require(first is not None and first[1] == 'Unreleased',
            'write curated Unreleased notes before preparing a release')
    promoted = text[:first.start()] + f'## {number}' + text[first.end():]
    notes(promoted, number)  # Refuse empty or unfinished notes before writing.
    return promoted


def generated_edits(repo, sha, edits):
    """Run the checked-in generators on the exact preparation base."""
    with tempfile.TemporaryDirectory(prefix='powerio-release-') as tmp:
        root = Path(tmp)
        run('git', 'init', '-q', cwd=root)
        run('git', 'fetch', '--depth=1', f'https://github.com/{repo}.git', sha, cwd=root)
        run('git', 'checkout', '--detach', 'FETCH_HEAD', cwd=root)
        for path, text in edits.items():
            destination = root / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(text)
        if repo == POWERIO:
            run('cargo', 'run', '--locked', '-p', 'powerio-dist', '--example', 'regen_bmopf_examples', cwd=root)
            run('cargo', 'run', '--locked', '-p', 'powerio', '--example', 'generate_schemas',
                '--features', 'schema', '--', 'docs/schema', cwd=root)
        paths = run('git', 'ls-files', '--modified', '--others', '--exclude-standard', cwd=root).splitlines()
        return {path: (root / path).read_text() for path in paths}


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
        path = 'powerio/src/lib.rs'
        library = source(repo, path, sha).decode()
        library, count = re.subn(r'(pub const IR_SCHEMA_ID: &str = "https://powerio.dev/schema/pio-ir/[0-9]+/)' + re.escape(old) + r'(/schema.json";)',
                                  lambda m: m[1] + number + m[2], library)
        require(count == 1, 'current IR schema catalog identifier is missing')
        result[path] = library
        path = 'powerio/tests/ir_reference.rs'
        result[path] = source(repo, path, sha).decode().replace(
            f'pio-ir/2/{old}/schema.json', f'pio-ir/2/{number}/schema.json')
        path = 'powerio/tests/frozen_schemas.rs'
        tests = source(repo, path, sha).decode().replace(
            f'const CURRENT_SCHEMA: &str = "pio-ir/2/{old}/schema.json";',
            f'const CURRENT_SCHEMA: &str = "pio-ir/2/{number}/schema.json";')
        tests = tests.replace('            CURRENT_SCHEMA,',
                              f'            "pio-ir/2/{old}/schema.json",\n            CURRENT_SCHEMA,')
        tests = tests.replace('    for earlier in [',
                              f'    for earlier in [\n        "pio-ir/2/{old}/schema.json",')
        history_start = tests.index('fn historical_schemas_preserve_their_original_identifiers()')
        tests = tests[:history_start] + tests[history_start:].replace(
            '    ] {', f'        (\n            "pio-ir/2/{old}/schema.json",\n'
            f'            "https://powerio.dev/schema/pio-ir/2/{old}/schema.json",\n        ),\n    ] {{', 1)
        result[path] = tests
        for path in ('scripts/check-value-types.sh', 'docs/src/ir-reference.md',
                     'AGENTS.md'):
            result[path] = source(repo, path, sha).decode().replace(
                f'pio-ir/2/{old}/schema.json', f'pio-ir/2/{number}/schema.json')
        path = 'docs/src/pio-json-schema.md'
        page = source(repo, path, sha).decode().replace(
            f'pio-ir/2/{old}/schema.json', f'pio-ir/2/{number}/schema.json', 2)
        result[path] = page.replace(f'PowerIO {old} keeps IR version', f'PowerIO {number} keeps IR version')
        path = 'docs/schema/README.md'
        readme = source(repo, path, sha).decode()
        row = f'| 2 | v{old} | `pio-ir`, version `2` | `pio-ir/2/{old}/schema.json` | yes |'
        require(row in readme, 'current schema catalog row is missing')
        readme = readme.replace(row, row + '\n' + row.replace(old, number))
        readme = readme.replace(f'PowerIO {old} keeps IR version', f'PowerIO {number} keeps IR version')
        readme = readme.replace('` beneath `https://powerio.dev/schema/`.',
                                f'`, and\n`pio-ir/2/{old}/schema.json` beneath `https://powerio.dev/schema/`.')
        readme = readme.replace(f'Read by {old}', f'Read by {number}')
        readme = readme.replace(f'Both remain 2 in {old}.', f'Both remain 2 in {number}.')
        readme = readme.replace(f'The current catalog uses `pio-ir/2/{old}/schema.json`',
                                f'The current catalog uses `pio-ir/2/{number}/schema.json`')
        readme = re.sub(r'(currently\n`pio-ir/2/)[0-9.]+(/schema.json`)',
                        lambda m: m[1] + number + m[2], readme)
        result[path] = readme
        path = 'README.md'
        result[path] = source(repo, path, sha).decode().replace(
            f'PowerIO {old} keeps PowerIO IR', f'PowerIO {number} keeps PowerIO IR')
        for name in ('case9_arrow_coo.json', 'case30_arrow_coo.json'):
            path = 'tests/data/capi_matrix/' + name
            before = source(repo, path, sha).decode()
            result[path] = before.replace(f'"powerio_version": "{old}"', f'"powerio_version": "{number}"')
        result[f'docs/release-notes/{number}.md'] = (
            f'# PowerIO {number} release notes\n\n'
            f'The [curated changelog](../../CHANGELOG.md#{number.replace(".", "")}) '
            'records this release. Review that section before candidate preparation.\n')
    else:
        project = source(repo, 'Project.toml', sha).decode()
        old = tomllib.loads(project)['version']
        require(version(number) > version(old), 'release version must increase')
        result = {'Project.toml': project.replace(f'version = "{old}"', f'version = "{number}"', 1)}
    changelog = source(repo, 'CHANGELOG.md', sha).decode()
    result['CHANGELOG.md'] = promote_changelog(changelog, number)
    if repo == JULIA:
        path = '.github/powerio-release.toml'
        intent = source(repo, path, sha).decode()
        intent = re.sub(r'^state = ".*"$', 'state = "draft"', intent, flags=re.M)
        intent = re.sub(r'^julia_version = ".*"$', f'julia_version = "{number}"', intent, flags=re.M)
        intent = re.sub(r'^powerio_tag = ".*"$', f'powerio_tag = "v{number}"', intent, flags=re.M)
        intent = re.sub(r'^source_digest = ".*"$', 'source_digest = "sha256:' + '0' * 64 + '"', intent, flags=re.M)
        result[path] = intent
    return result


def edited_tree_entries(repo, tree, edits):
    base = api(f'repos/{repo}/git/trees/{tree}?recursive=1')
    require(not base.get('truncated', False), 'base tree is truncated; cannot preserve file modes')
    modes = {entry['path']: entry['mode'] for entry in base['tree']}
    entries = []
    for path, text in edits.items():
        mode = modes.get(path, '100644')
        require(mode in ('100644', '100755'), f'release edit is not a regular file: {path}')
        blob = api(f'repos/{repo}/git/blobs', {'encoding': 'base64', 'content': base64.b64encode(text.encode()).decode()})
        entries.append({'path': path, 'mode': mode, 'type': 'blob', 'sha': blob['sha']})
    return entries


def propose(repo, number):
    branch = f'release/{number}'
    existing = api(f'repos/{repo}/pulls?state=all&head=eigenergy:{branch}&base=main')
    if existing:
        print(existing[0]['html_url'])
        return
    base = api(f'repos/{repo}/git/ref/heads/main')['object']['sha']
    require(api(f'repos/{repo}/git/ref/heads/{branch}', missing=True) is None,
            f'{repo} already has {branch}; inspect it before retrying')
    edits = generated_edits(repo, base, version_edits(repo, base, number))
    tree = api(f'repos/{repo}/git/commits/{base}')['tree']['sha']
    entries = edited_tree_entries(repo, tree, edits)
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
