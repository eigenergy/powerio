"""Exercise immutable release identity and recovery decisions without publication."""

import copy
import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("paired_release", Path(__file__).parents[1] / "paired_release.py")
pair = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(pair)
sys.path.insert(0, str(Path(__file__).parents[1]))
from prepare_release_prs import bump_lock_versions, example_metadata  # noqa: E402


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.frozen = {"format": 1, "tag": "v0.11.3", "powerio_sha": "a" * 40, "julia_source_sha": "b" * 40}
        self.manifest = dict(self.frozen, julia_sha="c" * 40, julia_tree_sha="d" * 40,
                             assets={name: "e" * 64 for name in pair.ASSETS},
                             validation={"status": "passed", "url": "https://github.com/eigenergy/powerio/actions/runs/123"})
        self.assets = {name: {"digest": "sha256:" + "e" * 64} for name in pair.ASSETS}

    def test_frozen_pair_does_not_read_main(self):
        with patch.object(pair, "api", side_effect=AssertionError("must not read main")):
            pair.validate_manifest(self.manifest, self.frozen, self.assets)

    def test_changed_candidate_is_rejected(self):
        for key in ("powerio_sha", "julia_source_sha", "tag"):
            changed = copy.deepcopy(self.manifest)
            changed[key] = "f" * 40
            with self.subTest(key=key), self.assertRaises(ValueError):
                pair.validate_manifest(changed, self.frozen, self.assets)

    def test_changed_binary_is_rejected(self):
        self.assets[next(iter(pair.ASSETS))]["digest"] = "sha256:" + "f" * 64
        with self.assertRaisesRegex(ValueError, "digest mismatch"):
            pair.validate_manifest(self.manifest, self.frozen, self.assets)

    def test_missing_and_extra_assets_are_rejected(self):
        values = [{"name": n} for n in pair.ASSETS | {pair.MANIFEST}]
        pair.asset_map({"assets": values}, complete=True)
        for changed in (values[:-1], values + [{"name": "unreviewed.zip"}], values + values[:1]):
            with self.assertRaises(ValueError):
                pair.asset_map({"assets": changed}, complete=True)

    def test_registered_tree_is_terminal(self):
        versions = {"0.11.3": {"git-tree-sha1": "d" * 40}}
        self.assertEqual(pair.registry_action(self.manifest, versions), "registered")

    def test_registered_wrong_tree_is_not_success(self):
        with self.assertRaisesRegex(ValueError, "different Julia tree"):
            pair.registry_action(self.manifest, {"0.11.3": {"git-tree-sha1": "f" * 40}})

    def test_next_version_and_out_of_order_dispatch(self):
        self.assertEqual(pair.registry_action(self.manifest, {"0.11.2": {}}), "register")
        with self.assertRaises(ValueError):
            pair.registry_action(self.manifest, {"0.11.4": {}})

    def test_yanked_registration_requires_attention(self):
        with self.assertRaisesRegex(ValueError, "yanked"):
            pair.registry_action(self.manifest, {"0.11.3": {"git-tree-sha1": "d" * 40, "yanked": True}})

    def test_example_bump_preserves_numeric_bytes_and_history(self):
        provenance = {('powerio_bmopf' if n == 0 else f'powerio_bmopf_{n}'): {'producer_version': '0.11.2', 'schema_commit': str(n)} for n in range(11)}
        meta = {'case_study_generator': {'tool': 'powerio', 'version': '0.11.2'}, 'provenance': provenance}
        encoded = json.dumps(meta, indent=2, sort_keys=True).replace('\n', '\n  ')
        text = '{\n  "bus": {"p": 1.0000000000000001},\n  "meta": ' + encoded + '\n}'
        changed = example_metadata(text, '0.11.3', retain_history=True)
        self.assertIn('1.0000000000000001', changed)
        result = json.loads(changed)['meta']['provenance']
        for key, record in provenance.items():
            self.assertEqual(result[key], record)
        self.assertEqual(result['powerio_bmopf_11']['schema_commit'], '10')
        self.assertEqual(result['powerio_bmopf_11']['producer_version'], '0.11.3')
        self.assertEqual(example_metadata(changed, '0.11.3', retain_history=True), changed)

    def test_release_bump_includes_nonprefixed_workspace_members(self):
        lock = '[[package]]\nname = "facade-only"\nversion = "0.11.2"\n\n[[package]]\nname = "serde"\nversion = "1.0.0"\n'
        changed = bump_lock_versions(lock, ['facade-only'], '0.11.3')
        self.assertIn('name = "facade-only"\nversion = "0.11.3"', changed)
        self.assertIn('name = "serde"\nversion = "1.0.0"', changed)

    def test_breaking_version_policy_matches_julia_semver(self):
        self.assertTrue(pair.breaking_transition((0, 11, 3), (0, 12, 0)))
        self.assertTrue(pair.breaking_transition((1, 1, 0), (2, 0, 0)))
        self.assertFalse(pair.breaking_transition((1, 1, 0), (1, 2, 0)))
        self.assertFalse(pair.breaking_transition((0, 11, 3), (0, 11, 4)))

    def test_asset_urls_cannot_drift_to_another_release(self):
        def content(tag):
            return ''.join(f'[[powerio_capi]]\n[[powerio_capi.download]]\nurl = "https://github.com/{pair.POWERIO}/releases/download/{tag}/{name}"\nsha256 = "{digest}"\n' for name, digest in self.manifest["assets"].items()).encode()
        pair.validate_artifacts(content('v0.11.3'), 'v0.11.3', self.manifest['assets'])
        with self.assertRaises(ValueError):
            pair.validate_artifacts(content('v0.11.4'), 'v0.11.3', self.manifest['assets'])

    def test_missing_credentials_fail_without_registration(self):
        with patch.object(pair, "verify", side_effect=RuntimeError("credentials expired")), patch.object(pair, "api") as api:
            with self.assertRaises(RuntimeError):
                pair.register('v0.11.3')
            api.assert_not_called()

    def replace_candidate(self, *, published=False, registered=False):
        old = dict(self.frozen, powerio_sha="d" * 40, julia_source_sha="e" * 40)
        def fake_api(path, data=None, **_kwargs):
            if data is not None:
                return {"sha": "9" * 40}
            if path.endswith('/git/ref/tags/v0.11.3'):
                return {"object": {"type": "tag", "sha": "f" * 40}}
            if path.endswith('/git/tags/' + 'f' * 40):
                import json
                return {"message": json.dumps(old), "object": {"type": "commit", "sha": old['powerio_sha']}}
            if path.endswith('/git/ref/heads/main'):
                return {"object": {"sha": "a" * 40 if pair.POWERIO in path else "b" * 40}}
            if '/releases/tags/' in path:
                return {"draft": not published, "id": 123}
            if path.endswith('/immutable-releases'):
                return {"enabled": True}
            raise AssertionError(path)
        def fake_source(_repo, path, _sha):
            if path.endswith('Versions.toml'):
                return ('["0.11.3"]' if registered else '["0.11.2"]').encode()
            return b'{"format":1}'
        with patch.object(pair, 'api', side_effect=fake_api), patch.object(pair, 'source', side_effect=fake_source), patch.object(pair, 'identity', return_value='Maintenance'), patch.object(pair, 'successful_ci'), patch.object(pair, 'run') as commands:
            if published or registered:
                with self.assertRaises(ValueError):
                    pair.create_tag('0.11.3', replace=True)
                commands.assert_not_called()
            else:
                pair.create_tag('0.11.3', replace=True)
                self.assertEqual(len(commands.call_args_list), 2)
                self.assertTrue(all('DELETE' in call.args for call in commands.call_args_list))

    def test_explicit_unpublished_replacement_preserves_registered_versions(self):
        self.replace_candidate(registered=True)

    def test_explicit_unpublished_replacement_preserves_published_releases(self):
        self.replace_candidate(published=True)

    def test_explicit_unpublished_replacement_can_reuse_the_unreleased_version(self):
        self.replace_candidate()

    def test_python_partial_publication_is_not_complete(self):
        names = ['powerio-0.11.3.tar.gz'] + ['powerio-0.11.3-cp39-abi3-' + platform + '.whl' for platform in
                 ('manylinux_2_17_x86_64', 'manylinux_2_17_aarch64', 'macosx_11_0_x86_64', 'macosx_11_0_arm64', 'win_amd64')]
        metadata = {'urls': [{'filename': name} for name in names]}
        self.assertTrue(pair.python_published(metadata, '0.11.3'))
        metadata['urls'].pop()
        self.assertFalse(pair.python_published(metadata, '0.11.3'))

    def test_completed_publications_do_not_depend_on_retained_workflow_history(self):
        names = ['powerio-0.11.3.tar.gz'] + ['powerio-0.11.3-cp39-abi3-' + platform + '.whl' for platform in
                 ('manylinux_2_17_x86_64', 'manylinux_2_17_aarch64', 'macosx_11_0_x86_64', 'macosx_11_0_arm64', 'win_amd64')]
        def metadata(url):
            return {'version': {'yanked': False}} if 'crates.io' in url else {'urls': [{'filename': name} for name in names]}
        with patch.object(pair, 'public_json', side_effect=metadata), patch.object(pair, 'api') as api, patch.object(pair, 'run') as run:
            pair.repair_publications('v0.11.3', self.manifest)
            api.assert_not_called()
            run.assert_not_called()

    def test_changelog_requires_curated_notes(self):
        self.assertEqual(pair.notes('# Changelog\n\n## 0.11.3\n\n- Maintenance.\n\n## 0.11.2\n- Older.\n', '0.11.3'), '- Maintenance.')
        with self.assertRaises(ValueError):
            pair.notes('## 0.11.3\n- REVIEW REQUIRED\n', '0.11.3')


if __name__ == '__main__':
    unittest.main()
