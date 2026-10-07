#!/usr/bin/env python3
"""Check paired release setup and activate one approved publication route."""

import argparse
import json

from paired_release import JULIA, POWERIO, api, require, run, successful_ci


def active_legacy_release(repo, workflow):
    if workflow['status'] == 'completed':
        return False
    path = workflow['path'].split('@', 1)[0].rsplit('/', 1)[-1]
    if workflow['event'] == 'release':
        return True
    if repo == JULIA:
        return path in ('update-artifacts.yml', 'register.yml')
    return ((path in ('python.yml', 'crates.yml') and workflow['event'] == 'workflow_dispatch') or
            (path == 'release-binaries.yml' and workflow['event'] == 'push'))


def check_no_active_legacy_release(repo):
    # Query every unfinished status so old waiting runs cannot fall outside
    # the most recent page of normal CI runs.
    for status in ('queued', 'in_progress', 'waiting', 'pending', 'requested'):
        page = 1
        while True:
            runs = api(f'repos/{repo}/actions/runs?status={status}&per_page=100&page={page}')['workflow_runs']
            require(not any(active_legacy_release(repo, workflow) for workflow in runs),
                    f'{repo} has an active legacy release')
            if len(runs) < 100:
                break
            page += 1


def check():
    for repo in (POWERIO, JULIA):
        head = api(f'repos/{repo}/git/ref/heads/main')['object']['sha']
        api(f'repos/{repo}/contents/.github/paired-release.json?ref={head}')
        successful_ci(repo, head)
        check_no_active_legacy_release(repo)
    variable = api(f'repos/{POWERIO}/actions/variables/POWERIO_RELEASE_APP_ID')
    require(variable['value'].isdigit(), 'release App ID is missing or invalid')
    names = {s['name'] for s in api(f'repos/{POWERIO}/actions/secrets?per_page=100')['secrets']}
    require('POWERIO_RELEASE_APP_PRIVATE_KEY' in names, 'release App private key is missing')
    probe = api(f'repos/{POWERIO}/actions/workflows/release-app-access.yml/runs?per_page=1')['workflow_runs']
    require(probe and probe[0]['conclusion'] == 'success' and
            probe[0]['head_sha'] == api(f'repos/{POWERIO}/git/ref/heads/main')['object']['sha'],
            'run Release App access successfully on current main before activation')
    environments = {}
    for name in ('crates-io', 'pypi'):
        env = api(f'repos/{POWERIO}/environments/{name}')
        require(env.get('deployment_branch_policy') is not None, f'{name} has no branch/tag restriction')
        require(all(rule['type'] in ('required_reviewers', 'wait_timer', 'branch_policy') for rule in env['protection_rules']),
                f'{name} has custom protection; preserve it through a reviewed settings update')
        environments[name] = env
    print('Paired source CI, App setup, App access, and publishing environments checked')
    print(json.dumps({'immutable_releases': [POWERIO, JULIA],
                      'PAIRED_RELEASES': {POWERIO: 'true', JULIA: 'true'},
                      'publishing_environments': {name: environment_update(env)
                                                 for name, env in environments.items()}}, indent=2))
    return environments


def environment_update(env):
    wait = next((rule.get('wait_timer', 0) for rule in env['protection_rules']
                 if rule['type'] == 'wait_timer'), 0)
    return {'wait_timer': wait, 'prevent_self_review': False, 'reviewers': [],
            'can_admins_bypass': env['can_admins_bypass'],
            'deployment_branch_policy': env['deployment_branch_policy']}


def activate():
    environments = check()
    for repo in (POWERIO, JULIA):
        # A PUT without a request body enables the repository feature.
        run('gh', 'api', '--method', 'PUT', f'repos/{repo}/immutable-releases')
        require(api(f'repos/{repo}/immutable-releases')['enabled'], f'immutable releases not enabled for {repo}')
    for repo in (POWERIO, JULIA):
        path = f'repos/{repo}/actions/variables/PAIRED_RELEASES'
        existing = api(path, missing=True)
        if existing:
            run('gh', 'api', '--method', 'PATCH', path, '-f', 'name=PAIRED_RELEASES', '-f', 'value=true')
        else:
            api(f'repos/{repo}/actions/variables', {'name': 'PAIRED_RELEASES', 'value': 'true'})
    for name, env in environments.items():
        run('gh', 'api', '--method', 'PUT', f'repos/{POWERIO}/environments/{name}', '--input', '-',
            input=json.dumps(environment_update(env)))
    print('Paired releases and immutable publication enabled; existing tag restrictions preserved')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--check', action='store_true')
    group.add_argument('--activate', action='store_true')
    args = parser.parse_args()
    activate() if args.activate else check()
