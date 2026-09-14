#!/usr/bin/env python3
"""Check paired release setup and activate one approved publication route."""

import argparse
from paired_release import POWERIO, JULIA, api, require, successful_ci


def check():
    for repo in (POWERIO, JULIA):
        head = api(f'repos/{repo}/git/ref/heads/main')['object']['sha']
        api(f'repos/{repo}/contents/.github/paired-release.json?ref={head}')
        successful_ci(repo, head)
        runs = api(f'repos/{repo}/actions/runs?status=in_progress&per_page=100')['workflow_runs']
        require(not any(r['event'] == 'release' or r['name'] in ('Update artifacts', 'Register Package') for r in runs),
                f'{repo} has an active legacy release')
    variable = api(f'repos/{POWERIO}/actions/variables/POWERIO_RELEASE_APP_ID')
    require(variable['value'].isdigit(), 'release App ID is missing or invalid')
    names = {s['name'] for s in api(f'repos/{POWERIO}/actions/secrets?per_page=100')['secrets']}
    require('POWERIO_RELEASE_APP_PRIVATE_KEY' in names, 'release App private key is missing')
    probe = api(f'repos/{POWERIO}/actions/workflows/release-app-access.yml/runs?per_page=1')['workflow_runs']
    require(probe and probe[0]['conclusion'] == 'success', 'run Release App access successfully before activation')
    environments = {}
    for name in ('crates-io', 'pypi'):
        env = api(f'repos/{POWERIO}/environments/{name}')
        require(env.get('deployment_branch_policy') is not None, f'{name} has no branch/tag restriction')
        require(all(rule['type'] in ('required_reviewers', 'wait_timer', 'branch_policy') for rule in env['protection_rules']),
                f'{name} has custom protection; preserve it through a reviewed settings update')
        environments[name] = env
    print('Paired source CI, App setup, App access, and publishing environments checked')
    return environments


def activate():
    environments = check()
    for repo in (POWERIO, JULIA):
        # A PUT without a request body enables the repository feature.
        from paired_release import run
        run('gh', 'api', '--method', 'PUT', f'repos/{repo}/immutable-releases')
        require(api(f'repos/{repo}/immutable-releases')['enabled'], f'immutable releases not enabled for {repo}')
    for repo in (POWERIO, JULIA):
        path = f'repos/{repo}/actions/variables/PAIRED_RELEASES'
        existing = api(path, missing=True)
        if existing:
            from paired_release import run
            run('gh', 'api', '--method', 'PATCH', path, '-f', 'name=PAIRED_RELEASES', '-f', 'value=true')
        else:
            api(f'repos/{repo}/actions/variables', {'name': 'PAIRED_RELEASES', 'value': 'true'})
    for name, env in environments.items():
        wait = next((rule.get('wait_timer', 0) for rule in env['protection_rules'] if rule['type'] == 'wait_timer'), 0)
        from paired_release import run
        import json
        run('gh', 'api', '--method', 'PUT', f'repos/{POWERIO}/environments/{name}', '--input', '-',
            input=json.dumps({'wait_timer': wait, 'prevent_self_review': False, 'reviewers': [],
                              'deployment_branch_policy': env['deployment_branch_policy']}))
    print('Paired releases and immutable publication enabled; existing tag restrictions preserved')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--check', action='store_true')
    group.add_argument('--activate', action='store_true')
    args = parser.parse_args()
    activate() if args.activate else check()
