#!/usr/bin/env python3
"""Attach missing draft binaries without replacing an existing candidate."""

import argparse
from pathlib import Path

from paired_release import ASSETS, MANIFEST, POWERIO, release, require, run, verify


def attach(tag, directory):
    root = Path(directory)
    require({p.name for p in root.glob('*.tar.gz')} == ASSETS, 'expected five binary archives')
    current = release(tag)
    if current and not current['draft']:
        verify(tag)
        print('Published paired release verified; no assets changed')
        return
    if current and any(a['name'] == MANIFEST for a in current['assets']):
        verify(tag, published=False)
        print('Prepared paired draft verified; no assets changed')
        return
    if current is None:
        run('gh', 'release', 'create', tag, '--repo', POWERIO, '--verify-tag', '--draft',
            '--title', tag, '--notes', 'Candidate preparation is in progress. Wait for the paired review information and release-manifest.json before publishing.')
        current = release(tag)
    require(current['draft'] and not current['prerelease'], 'only a stable draft accepts binaries')
    present = {a['name'] for a in current['assets']}
    require(present <= ASSETS, 'unexpected draft assets')
    for name in sorted(ASSETS - present):
        run('gh', 'release', 'upload', tag, str(root / name), '--repo', POWERIO)
    print('Five draft binaries are available; existing assets were preserved')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('tag')
    parser.add_argument('directory')
    args = parser.parse_args()
    attach(args.tag, args.directory)
