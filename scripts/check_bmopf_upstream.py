#!/usr/bin/env python3
"""Record upstream schema review changes without adopting their modeling rules."""

import base64
import json
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from paired_release import POWERIO, api, sha256

BRANCH = 'maintenance/bmopf-upstream-review'
PATH = '.github/bmopf-upstream.json'


def public_api(path):
    request = Request("https://api.github.com/" + path, headers={"Accept": "application/vnd.github+json", "User-Agent": "PowerIO schema review"})
    try:
        with urlopen(request, timeout=30) as response:
            return json.load(response)
    except HTTPError as error:
        if error.code == 404:
            return None
        raise


def observe(entry):
    repo = entry['upstream_repository']
    number = entry['review_url'].rsplit('/', 1)[1]
    pr = public_api(f'repos/{repo}/pulls/{number}')
    result = {'version': entry['version'], 'archived_sha256': entry['sha256'], 'review_url': entry['review_url']}
    if pr is None:
        return dict(result, status='unavailable')
    status = 'merged' if pr['merged'] else 'closed-unmerged' if pr['state'] == 'closed' else 'open'
    head = pr['head']['sha']
    item = public_api(f"repos/{repo}/contents/{entry['upstream_path']}?ref={head}")
    if item is None:
        return dict(result, status=status, schema_status='unavailable')
    digest = sha256(base64.b64decode(item['content']))
    return dict(result, status=status, schema_status='unchanged' if digest == entry['sha256'] else 'changed',
                observed_sha256=digest)


def main():
    root = Path(__file__).resolve().parents[1]
    entries = json.loads((root / 'powerio-dist/schemas/bmopf/manifest.json').read_text())['schemas']
    report = {'format': 1, 'schemas': [observe(entry) for entry in entries]}
    baseline = root / PATH
    if baseline.exists() and json.loads(baseline.read_text()) == report:
        print('Upstream review state and archived schema bytes remain unchanged')
        return
    base = api(f'repos/{POWERIO}/git/ref/heads/main')['object']['sha']
    ref = api(f'repos/{POWERIO}/git/ref/heads/{BRANCH}', missing=True)
    if ref is None:
        api(f'repos/{POWERIO}/git/refs', {'ref': f'refs/heads/{BRANCH}', 'sha': base})
    current = api(f'repos/{POWERIO}/contents/{PATH}?ref={BRANCH}', missing=True)
    content = (json.dumps(report, indent=2, sort_keys=True) + '\n').encode()
    unchanged = current is not None and base64.b64decode(current['content']) == content
    prior = api(f'repos/{POWERIO}/pulls?state=all&head=eigenergy:{BRANCH}&base=main')
    if unchanged and prior and prior[0]['state'] == 'closed' and not prior[0].get('merged_at'):
        print('The current upstream observation was already reviewed and closed')
        return
    if not unchanged:
        from paired_release import run
        payload = {'message': 'docs: record BMOPF upstream review state', 'branch': BRANCH,
                   'content': base64.b64encode(content).decode()}
        if current:
            payload['sha'] = current['sha']
        run('gh', 'api', '--method', 'PUT', f'repos/{POWERIO}/contents/{PATH}', '--input', '-', input=json.dumps(payload))
    prs = api(f'repos/{POWERIO}/pulls?state=open&head=eigenergy:{BRANCH}&base=main')
    if not prs:
        pr = api(f'repos/{POWERIO}/pulls', {'head': BRANCH, 'base': 'main', 'draft': True,
            'title': 'docs: review BMOPF upstream changes',
            'body': 'An upstream schema review state or schema digest differs from the recorded observation. Review the attached observations against the supported archive and the linked Task Force discussions. This PR does not adopt schema changes or claim Task Force acceptance.\n\nIf modeling rules changed, update the archive, provenance, adapters, and validation in a separate reviewed contribution. Released schema snapshots remain unchanged.'})
        print(pr['html_url'])
    else:
        print(prs[0]['html_url'])


if __name__ == '__main__':
    main()
