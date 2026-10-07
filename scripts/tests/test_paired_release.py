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
import prepare_release_prs as preparation  # noqa: E402
from activate_paired_releases import environment_update  # noqa: E402
from prepare_release_prs import (  # noqa: E402
    bump_lock_versions,
    bump_workspace_versions,
    promote_changelog,
)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.frozen = {"format": 1, "tag": "v0.11.3", "powerio_sha": "a" * 40, "julia_source_sha": "b" * 40}
        self.manifest = dict(self.frozen, julia_sha="c" * 40, julia_tree_sha="d" * 40,
                             assets={name: "e" * 64 for name in pair.ASSETS},
                             validation={"status": "passed", "url": "https://github.com/eigenergy/powerio/actions/runs/123"})
        self.assets = {name: {"digest": "sha256:" + "e" * 64} for name in pair.ASSETS}

    def test_draft_release_is_found_when_tag_endpoint_returns_404(self):
        draft = {"id": 123, "tag_name": "v0.11.3", "draft": True}
        with patch.object(pair, "api", return_value=None), patch.object(pair, "pages", return_value=[{"tag_name": "v0.11.2"}, draft]):
            self.assertEqual(pair.release("v0.11.3"), draft)

    def test_published_release_does_not_require_draft_listing(self):
        published = {"id": 123, "tag_name": "v0.11.3", "draft": False}
        with patch.object(pair, "api", return_value=published), patch.object(pair, "pages") as listing:
            self.assertEqual(pair.release("v0.11.3"), published)
            listing.assert_not_called()

    def test_absent_release_is_not_replaced_by_another_draft(self):
        with patch.object(pair, "api", return_value=None), patch.object(pair, "pages", return_value=[{"tag_name": "v0.11.4", "draft": True}]):
            self.assertIsNone(pair.release("v0.11.3"))

    def test_ambiguous_drafts_are_rejected(self):
        drafts = [{"id": number, "tag_name": "v0.11.3", "draft": True} for number in (1, 2)]
        with patch.object(pair, "api", return_value=None), patch.object(pair, "pages", return_value=drafts):
            with self.assertRaisesRegex(ValueError, "multiple releases"):
                pair.release("v0.11.3")

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

    def test_candidate_ci_excludes_only_its_own_preparation_run(self):
        check = {'status': 'completed', 'conclusion': 'success', 'html_url': 'ci', 'check_suite': {'id': 1}}
        own = dict(check, status='in_progress', conclusion=None, check_suite={'id': 2})
        current = {'path': '.github/workflows/prepare-paired-release.yml', 'event': 'workflow_dispatch',
                   'head_sha': 'a' * 40, 'check_suite_id': 2}
        env = {'GITHUB_REPOSITORY': pair.POWERIO, 'GITHUB_RUN_ID': '123',
               'GITHUB_WORKFLOW': 'Prepare paired release'}
        with patch.dict(pair.os.environ, env), patch.object(pair, 'api', side_effect=[{'check_runs': [check, own]}, current]):
            self.assertEqual(pair.successful_ci(pair.POWERIO, 'a' * 40), ['ci'])
        for checks in ([own], [dict(check, status='in_progress'), own],
                       [dict(check, conclusion='failure'), own]):
            with self.subTest(checks=checks), patch.dict(pair.os.environ, env), patch.object(pair, 'api', side_effect=[{'check_runs': checks}, current]):
                with self.assertRaises(ValueError):
                    pair.successful_ci(pair.POWERIO, 'a' * 40)

    def test_preparation_updates_current_schema_tests_without_editing_examples(self):
        root = Path(__file__).resolve().parents[2]
        def local_source(_repo, path, _sha):
            if path == 'CHANGELOG.md':
                return b'# Changelog\n\n## Unreleased\n\n- Reviewed change.\n'
            return (root / path).read_bytes()
        old = preparation.tomllib.loads((root / 'Cargo.toml').read_text())['workspace']['package']['version']
        with patch.object(preparation, 'source', side_effect=local_source):
            edits = preparation.version_edits(pair.POWERIO, 'a' * 40, '0.11.99')
        self.assertIn('docs/release-notes/0.11.99.md', edits)
        self.assertIn('pio-ir/2/0.11.99/schema.json', edits['powerio/tests/ir_reference.rs'])
        self.assertIn(f'"pio-ir/2/{old}/schema.json",', edits['powerio/tests/frozen_schemas.rs'])
        self.assertFalse(any(path.startswith('powerio-dist/examples/bmopf/') for path in edits))
        for path in ('scripts/check-value-types.sh', 'docs/src/ir-reference.md',
                     'AGENTS.md'):
            self.assertIn('pio-ir/2/0.11.99/schema.json', edits[path])
            self.assertNotIn(f'pio-ir/2/{old}/schema.json', edits[path])
        self.assertIn('pio-ir/2/0.11.99/schema.json', edits['docs/src/pio-json-schema.md'])
        self.assertIn('`pio-ir/2/0.11.4/schema.json` adds the fixed-dispatch', edits['docs/src/pio-json-schema.md'])
        self.assertIn(f'| 2 | v{old} |', edits['docs/schema/README.md'])
        self.assertIn(f'https://powerio.dev/schema/pio-ir/2/{old}/schema.json', edits['powerio/tests/frozen_schemas.rs'])
        self.assertIn(f'`pio-ir/2/{old}/schema.json` beneath', edits['docs/schema/README.md'])
        self.assertIn('The current catalog uses `pio-ir/2/0.11.99/schema.json`', edits['docs/schema/README.md'])

    def test_activation_preserves_environment_restrictions(self):
        env = {'can_admins_bypass': False,
               'deployment_branch_policy': {'protected_branches': False, 'custom_branch_policies': True},
               'protection_rules': [{'type': 'wait_timer', 'wait_timer': 15},
                                    {'type': 'required_reviewers', 'reviewers': ['maintainer']}]}
        changed = environment_update(env)
        self.assertEqual(changed['reviewers'], [])
        self.assertEqual(changed['wait_timer'], 15)
        self.assertFalse(changed['can_admins_bypass'])
        self.assertEqual(changed['deployment_branch_policy'], env['deployment_branch_policy'])

    def test_changelog_promotion_preserves_notes_and_history(self):
        text = '# Changelog\n\n## Unreleased\n\n- New behavior.\n\n## 0.11.4\n\n- Earlier.\n'
        self.assertEqual(promote_changelog(text, '0.11.5'), text.replace('## Unreleased', '## 0.11.5'))
        for invalid in ('## Unreleased\n\n## 0.11.4\n- Earlier.\n',
                        '## 0.11.4\n- Earlier.\n',
                        '## Unreleased\n- TODO: write notes.\n',
                        '## Unreleased\n- New.\n## 0.11.5\n- Duplicate.\n'):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                promote_changelog(invalid, '0.11.5')

    def test_frozen_tools_reject_later_main(self):
        with patch.object(pair, 'run', side_effect=['f' * 40]) as command:
            with self.assertRaisesRegex(ValueError, 'frozen PowerIO'):
                pair.require_frozen_tools(self.frozen, '/later-main')
            self.assertEqual(command.call_count, 1)
        with patch.object(pair, 'run', side_effect=['a' * 40, '']) as command:
            pair.require_frozen_tools(self.frozen, '/frozen-tag')
            self.assertEqual(command.call_count, 2)

    def test_validation_evidence_must_name_frozen_powerio_commit(self):
        evidence = {'status': 'completed', 'conclusion': 'success',
                    'path': '.github/workflows/complete-paired-release.yml', 'head_sha': 'f' * 40}
        with patch.object(pair, 'api', return_value=evidence):
            self.assertFalse(pair.validation_succeeded(self.manifest))
            evidence['head_sha'] = self.frozen['powerio_sha']
            self.assertTrue(pair.validation_succeeded(self.manifest))

    def test_release_bump_does_not_upgrade_unrelated_dependencies(self):
        cargo = '[workspace.package]\nversion = "0.11.2"\n[workspace.dependencies]\npowerio = { path = "powerio", version = "0.11.2" }\nexternal = { version = "0.11.2" }\n'
        changed = bump_workspace_versions(cargo, '0.11.3')
        self.assertIn('powerio = { path = "powerio", version = "0.11.3" }', changed)
        self.assertIn('external = { version = "0.11.2" }', changed)

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

    def test_registration_dispatches_the_existing_julia_workflow(self):
        with patch.object(pair, 'verify', return_value=self.manifest), patch.object(pair, 'repair_publications'), patch.object(pair, 'sync_candidate'), patch.object(pair, 'source', return_value=b'["0.11.2"]'), patch.object(pair, 'api', return_value={'workflow_runs': []}) as api, patch.object(pair, 'run') as run:
            pair.register('v0.11.3')
            run.assert_called_once_with('gh', 'workflow', 'run', 'register.yml', '--repo', pair.JULIA, '--ref', 'main', '-f', 'version=0.11.3', '-f', 'expected_sha=' + 'c' * 40)
            self.assertTrue(all(len(call.args) == 1 for call in api.call_args_list))

    def test_registered_pair_still_synchronizes_artifacts(self):
        versions = b'["0.11.3"]\ngit-tree-sha1 = "dddddddddddddddddddddddddddddddddddddddd"\n'
        with patch.object(pair, 'verify', return_value=self.manifest), patch.object(pair, 'repair_publications'), patch.object(pair, 'sync_candidate') as sync, patch.object(pair, 'source', return_value=versions), patch.object(pair, 'api', return_value={'id': 123}), patch.object(pair, 'run') as run:
            pair.register('v0.11.3')
            sync.assert_called_once_with(self.manifest)
            run.assert_not_called()

    def test_registration_request_refuses_another_repository_or_sha(self):
        for repo, sha in ((pair.POWERIO, 'c' * 40), (pair.JULIA, 'f' * 40)):
            with self.subTest(repo=repo, sha=sha), patch.dict(pair.os.environ, {'GITHUB_REPOSITORY': repo}), patch.object(pair, 'verify', return_value=self.manifest), patch.object(pair, 'api') as api:
                with self.assertRaises(ValueError):
                    pair.request_registration('v0.11.3', sha, '.')
                api.assert_not_called()

    def test_registration_request_names_the_exact_tested_commit(self):
        with patch.dict(pair.os.environ, {'GITHUB_REPOSITORY': pair.JULIA}), patch.object(pair, 'verify', return_value=self.manifest), patch.object(pair, 'run', side_effect=['c' * 40, '']), patch.object(pair, 'source', return_value=b'["0.11.2"]'), patch.object(pair, 'identity', return_value='Maintenance.'), patch.object(pair, 'pages', return_value=[]), patch.object(pair, 'api') as api:
            pair.request_registration('v0.11.3', 'c' * 40, '.')
            api.assert_called_once_with(f'repos/{pair.JULIA}/commits/' + 'c' * 40 + '/comments', {'body': '@JuliaRegistrator register\n\nRelease notes:\nMaintenance.'})

    def replace_candidate(self, *, published=False, registered=False):
        old = dict(self.frozen, powerio_sha="d" * 40, julia_source_sha="e" * 40)
        def fake_api(path, data=None, **_kwargs):
            if data is not None:
                return {"sha": "9" * 40}
            if path.endswith('/git/ref/tags/v0.11.3'):
                return {"object": {"type": "tag", "sha": "f" * 40}}
            if path.endswith('/git/tags/' + 'f' * 40):
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
            return {'version': {'yanked': False}} if url.startswith('https://crates.io/') else {'urls': [{'filename': name} for name in names]}
        with patch.object(pair, 'public_json', side_effect=metadata), patch.object(pair, 'api') as api, patch.object(pair, 'run') as run:
            pair.repair_publications('v0.11.3', self.manifest)
            api.assert_not_called()
            run.assert_not_called()

    def test_publication_retry_runs_on_the_tag_allowed_by_deployment_rules(self):
        with patch.object(pair, 'public_json', return_value=None), patch.object(pair, 'api', return_value={'workflow_runs': []}), patch.object(pair, 'run') as run:
            pair.repair_publications('v0.11.3', self.manifest)
            self.assertEqual(run.call_count, 2)
            for call in run.call_args_list:
                self.assertEqual(call.args[call.args.index('--ref') + 1], 'v0.11.3')

    def test_changelog_requires_curated_notes(self):
        self.assertEqual(pair.notes('# Changelog\n\n## 0.11.3\n\n- Maintenance.\n\n## 0.11.2\n- Older.\n', '0.11.3'), '- Maintenance.')
        with self.assertRaises(ValueError):
            pair.notes('## 0.11.3\n- REVIEW REQUIRED\n', '0.11.3')

    def test_uploaded_manifest_with_interrupted_validation_is_retried(self):
        draft = {'draft': True, 'prerelease': False, 'tag_name': 'v0.11.3',
                 'body': '## Paired release review',
                 'assets': [{'name': n} for n in pair.ASSETS | {pair.MANIFEST}]}
        with patch.object(pair, 'pages', return_value=[draft]), patch.object(pair, 'api', return_value={'workflow_runs': []}), patch.object(pair, 'tag_pair', return_value=self.frozen), patch.object(pair, 'verify', return_value=self.manifest), patch.object(pair, 'validation_succeeded', return_value=False), patch.object(pair, 'run') as run:
            pair.recover_drafts()
            run.assert_called_once_with('gh', 'workflow', 'run', 'complete-paired-release.yml', '--repo', pair.POWERIO, '--ref', 'v0.11.3', '-f', 'tag=v0.11.3')

    def test_interrupted_or_wrong_validation_is_not_publication_evidence(self):
        for evidence in (None, {'status': 'completed', 'conclusion': 'cancelled', 'path': '.github/workflows/complete-paired-release.yml'}, {'status': 'completed', 'conclusion': 'success', 'path': '.github/workflows/unrelated.yml'}):
            with self.subTest(evidence=evidence), patch.object(pair, 'api', return_value=evidence):
                self.assertFalse(pair.validation_succeeded(self.manifest))


if __name__ == '__main__':
    unittest.main()
