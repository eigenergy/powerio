"""Offline release-helper safety tests. These never call GitHub or publish."""

import base64
import importlib.util
import io
import subprocess
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "release_verified", Path(__file__).with_name("release-verified.py")
)
helper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(helper)
SHA = "a" * 40


def source_run(name, **overrides):
    value = {
        "id": 1,
        "path": f".github/workflows/{name}",
        "head_sha": SHA,
        "event": "push",
        "head_branch": "main",
        "status": "completed",
        "conclusion": "success",
        "html_url": "https://github.com/example/run",
    }
    value.update(overrides)
    return value


class ReleaseSafety(unittest.TestCase):
    def test_source_workflows_require_exact_head_and_success(self):
        runs = [source_run(name) for name in helper.MAIN_WORKFLOWS]
        helper.validate_source_runs(runs, SHA)
        for change in (
            {"head_sha": "b" * 40},
            {"event": "pull_request"},
            {"conclusion": "failure"},
            {"status": "in_progress"},
        ):
            with self.subTest(change=change), self.assertRaises(helper.ReleaseError):
                helper.validate_source_runs([dict(runs[0], **change)] + runs[1:], SHA)

    def test_newer_failed_run_cannot_hide_behind_old_success(self):
        runs = [source_run(name) for name in helper.MAIN_WORKFLOWS]
        runs.append(dict(runs[0], id=2, conclusion="failure"))
        with self.assertRaises(helper.ReleaseError):
            helper.validate_source_runs(runs, SHA)

    def test_every_packaged_platform_and_binding_gate_is_required(self):
        names = {
            "PowerIO.jl main against this tag",
            "PowerIO.jl pins the packaged tarballs",
            "attach draft release assets",
            *helper.PLATFORMS,
            *(f"PowerIO.jl on {p}" for p in helper.PLATFORMS),
        }
        jobs = [{"name": name, "conclusion": "success"} for name in names]
        helper.validate_release_jobs(jobs)
        for index in range(len(jobs)):
            with self.subTest(missing=index), self.assertRaises(helper.ReleaseError):
                helper.validate_release_jobs(jobs[:index] + jobs[index + 1 :])
            with self.subTest(skipped=index), self.assertRaises(helper.ReleaseError):
                helper.validate_release_jobs(
                    jobs[:index]
                    + [dict(jobs[index], conclusion="skipped")]
                    + jobs[index + 1 :]
                )

    def test_exact_nonempty_native_assets_required(self):
        release = {
            "tag_name": "v0.11.6",
            "prerelease": False,
            "body": "Reviewed notes",
            "assets": [
                {"name": f"libpowerio_capi.{p}.tar.gz", "state": "uploaded", "size": 1}
                for p in helper.PLATFORMS
            ],
        }
        helper.validate_release(release, "v0.11.6")
        for change in (
            {"prerelease": True},
            {"tag_name": "v0.11.5"},
            {"body": ""},
            {"assets": release["assets"][:-1]},
            {"assets": [dict(a, size=0) for a in release["assets"]]},
        ):
            with self.subTest(change=change), self.assertRaises(helper.ReleaseError):
                helper.validate_release(dict(release, **change), "v0.11.6")

    def test_draft_release_is_found_by_listing_not_by_tag(self):
        # GET releases/tags/{tag} never returns a draft.
        draft = {"id": 7, "tag_name": "v0.11.6", "draft": True}
        older = {"id": 6, "tag_name": "v0.11.5", "draft": False}
        with patch.object(helper, "api", return_value=[draft, older]) as api:
            self.assertEqual(helper.release_for_tag("v0.11.6"), draft)
            self.assertIsNone(helper.release_for_tag("v0.11.7"))
            self.assertTrue(
                all("releases/tags" not in call.args[0] for call in api.call_args_list)
            )
        page = [dict(older, id=index) for index in range(100)]
        with patch.object(helper, "api", side_effect=[page, [draft]]):
            self.assertEqual(helper.release_for_tag("v0.11.6"), draft)
        with patch.object(helper, "api", return_value=[draft, dict(draft, id=8)]):
            with self.assertRaises(helper.ReleaseError):
                helper.release_for_tag("v0.11.6")

    def test_only_get_404_is_absence_not_auth_or_write_failure(self):
        with patch.object(
            helper.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                [], 1, "", "gh: Not Found (HTTP 404)"
            ),
        ):
            self.assertIsNone(helper.api("read", missing_ok=True))
            with self.assertRaises(helper.ReleaseError):
                helper.api("write", "POST", {}, missing_ok=True)
        with patch.object(
            helper.subprocess,
            "run",
            return_value=subprocess.CompletedProcess(
                [], 1, "", "gh: Bad credentials (HTTP 401)"
            ),
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.api("read", missing_ok=True)

    def test_lightweight_tag_never_accepted_or_replaced(self):
        with patch.object(
            helper, "api", return_value={"object": {"type": "commit", "sha": SHA}}
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.tag_commit("v0.11.6")

    def test_existing_annotated_tag_is_resolved_without_writes(self):
        values = [
            {"object": {"type": "tag", "sha": "c" * 40}},
            {"tag": "v0.11.6", "object": {"type": "commit", "sha": SHA}},
        ]
        with patch.object(helper, "api", side_effect=values) as api:
            self.assertEqual(helper.tag_commit("v0.11.6"), SHA)
            self.assertTrue(all(len(call.args) == 1 for call in api.call_args_list))

    def test_workflow_dispatch_is_not_a_tag_gate(self):
        run = source_run(
            "release-binaries.yml", event="workflow_dispatch", head_branch="v0.11.6"
        )
        with (
            patch.object(helper, "collection", return_value=[run]),
            patch.object(helper, "wait_pause", side_effect=helper.ReleaseError("stop")),
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.wait_for_tag_run(SHA, "v0.11.6", 0)

    def test_registry_verification_rejects_empty_wrong_or_yanked_versions(self):
        for value in (None, {}, {"info": {"version": "0.11.60"}, "urls": []}):
            self.assertFalse(helper.published_pypi_version(value, "0.11.6"))
        document = {
            "info": {"version": "0.11.6"},
            "urls": [
                {"packagetype": "bdist_wheel", "yanked": False},
                {"packagetype": "sdist", "yanked": False},
            ],
        }
        self.assertTrue(helper.published_pypi_version(document, "0.11.6"))
        document["urls"][0]["yanked"] = True
        self.assertFalse(helper.published_pypi_version(document, "0.11.6"))
        self.assertTrue(
            helper.published_crate_version(
                {"version": {"num": "0.11.6", "yanked": False}}, "0.11.6"
            )
        )
        for value in (
            None,
            {},
            {"version": {"num": "0.11.60", "yanked": False}},
            {"version": {"num": "0.11.6", "yanked": True}},
        ):
            self.assertFalse(helper.published_crate_version(value, "0.11.6"))

    def test_neighboring_patch_package_run_is_not_accepted(self):
        run = source_run(
            "python.yml",
            event="release",
            head_branch="v0.11.60",
            display_title="PowerIO Python v0.11.60",
        )
        with (
            patch.object(helper, "collection", return_value=[run]),
            patch.object(helper, "api") as api,
            patch.object(helper, "public_json") as registry,
            patch.object(
                helper, "wait_pause", side_effect=helper.ReleaseError("pending")
            ),
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.follow_publication(SHA, "b" * 40, "0.11.6", "v0.11.6", 0)
            api.assert_not_called()
            registry.assert_not_called()

    def test_julia_uses_resolved_commit_and_canonical_asset_origin(self):
        released = "d" * 40
        tree = "e" * 40
        general = base64.b64encode(
            f'["0.11.6"]\ngit-tree-sha1 = "{tree}"\n'.encode()
        ).decode()
        values = [
            {"object": {"type": "commit", "sha": released}},
            {"content": general},
            {"status": "ahead", "base_commit": {"sha": SHA}},
            {"tree": {"sha": tree}},
        ]
        for origin, valid in (
            ("https://github.com/eigenergy/powerio", True),
            ("https://example.org/eigenergy/powerio", False),
        ):
            artifacts = "\n".join(
                f"{origin}/releases/download/v0.11.6/libpowerio_capi.{p}.tar.gz"
                for p in helper.PLATFORMS
            )
            with (
                self.subTest(origin=origin),
                patch.object(helper, "api", side_effect=values),
                patch.object(
                    helper, "contents", side_effect=['version = "0.11.6"', artifacts]
                ) as content,
            ):
                if valid:
                    self.assertTrue(
                        helper.julia_registered_release("0.11.6", "v0.11.6", SHA)
                    )
                else:
                    with self.assertRaises(helper.ReleaseError):
                        helper.julia_registered_release("0.11.6", "v0.11.6", SHA)
                self.assertTrue(
                    all(c.args[2] == released for c in content.call_args_list)
                )

    def test_version_is_required_and_stable_versions_are_not_patch_line_locked(self):
        args = ["--candidate", SHA, "--julia-candidate", "b" * 40]
        with redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
            helper.main(args)
        self.assertEqual(error.exception.code, 2)
        for version in ("0.11.6", "0.12.0", "1.0.0"):
            with (
                self.subTest(version=version),
                redirect_stdout(io.StringIO()),
                patch.object(
                    helper.subprocess,
                    "run",
                    return_value=subprocess.CompletedProcess([], 1, "", ""),
                ) as gh,
                self.assertRaisesRegex(helper.ReleaseError, "authentication"),
            ):
                helper.main(args + ["--version", version])
            gh.assert_called_once()
        for version in ("v0.11.6", "01.2.3", "1.0", "1.0.0-rc1", "1.0.0/unsafe"):
            with (
                self.subTest(version=version),
                patch.object(helper.subprocess, "run") as gh,
                self.assertRaisesRegex(helper.ReleaseError, "MAJOR.MINOR.PATCH"),
            ):
                helper.main(args + ["--version", version])
            gh.assert_not_called()

    def test_progress_displays_state_changes_without_duplicate_polling_noise(self):
        with patch.object(helper, "_REPORTED", {}), patch("builtins.print") as output:
            helper.report("job:1", "Build: queued | https://github.com/example")
            helper.report("job:1", "Build: queued | https://github.com/example")
            helper.report("job:1", "Build: success | https://github.com/example")
            self.assertEqual(output.call_count, 2)

    def test_timeout_does_not_invent_success(self):
        with self.assertRaises(helper.ReleaseError):
            helper.wait_pause(0, "pending gate")


class FirstPublication(unittest.TestCase):
    def registry(self, published=False, claim=False, missing_predecessor=False):
        def lookup(url):
            crate = url.split("/crates/")[1].split("/")[0]
            if url.endswith("/0.11.6"):
                if (crate == "powerio-tnd" and not published) or (
                    missing_predecessor and crate == "powerio-tx"
                ):
                    return None
                return {"version": {"num": "0.11.6", "yanked": False}}
            return {"crate": {"name": crate}} if claim else None

        return lookup

    def test_only_named_new_crate_publishes_after_predecessors(self):
        lookup = self.registry()

        def publish(*args, **kwargs):
            nonlocal lookup
            lookup = self.registry(published=True)

        with (
            patch.object(
                helper,
                "public_json",
                side_effect=lambda u: lookup(u),  # noqa: PLW0108 -- lookup changes after the mocked publish
            ),
            patch.object(helper, "require_clean_candidate", return_value=Path("/repo")),
            patch.object(helper.subprocess, "run", side_effect=publish) as run,
        ):
            self.assertTrue(
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-tnd"])
            )
            run.assert_called_once_with(
                ["cargo", "publish", "--locked", "--all-features", "-p", "powerio-tnd"],
                cwd=Path("/repo"),
                check=True,
            )

    def test_new_synth_bootstraps_after_current_published_predecessors(self):
        published = False

        def lookup(url):
            nonlocal published
            crate = url.split("/crates/")[1].split("/")[0]
            if url.endswith("/0.11.6"):
                if crate == "powerio-synth" and not published:
                    return None
                return {"version": {"num": "0.11.6", "yanked": False}}
            if crate == "powerio-synth":
                return None
            return {"crate": {"name": crate}}

        def publish(*args, **kwargs):
            nonlocal published
            published = True

        with (
            patch.object(helper, "public_json", side_effect=lookup),
            patch.object(helper, "require_clean_candidate", return_value=Path("/repo")),
            patch.object(helper.subprocess, "run", side_effect=publish) as run,
        ):
            self.assertTrue(
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-synth"])
            )
            run.assert_called_once_with(
                ["cargo", "publish", "--locked", "--all-features", "-p", "powerio-synth"],
                cwd=Path("/repo"),
                check=True,
            )

    def test_missing_predecessor_blocks_local_publish(self):
        with (
            patch.object(
                helper,
                "public_json",
                side_effect=self.registry(missing_predecessor=True),
            ),
            patch.object(helper.subprocess, "run") as run,
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-tnd"])
            run.assert_not_called()

    def test_existing_name_cannot_be_bootstrapped(self):
        with (
            patch.object(helper, "public_json", side_effect=self.registry(claim=True)),
            patch.object(helper.subprocess, "run") as run,
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-tnd"])
            run.assert_not_called()

    def test_exact_clean_checkout_required(self):
        for head, status in (
            (SHA, " M Cargo.toml"),
            ("b" * 40, ""),
            (SHA, "?? extra.rs"),
        ):
            with (
                patch.object(
                    helper.subprocess,
                    "run",
                    side_effect=[
                        subprocess.CompletedProcess([], 0, stdout=head),
                        subprocess.CompletedProcess([], 0, stdout=status),
                    ],
                ),
                self.assertRaises(helper.ReleaseError),
            ):
                helper.require_clean_candidate(SHA)

    def test_registry_readback_required(self):
        with (
            patch.object(helper, "public_json", side_effect=self.registry()),
            patch.object(helper, "require_clean_candidate", return_value=Path("/repo")),
            patch.object(helper.subprocess, "run"),
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-tnd"])

    def test_only_failed_publish_step_permits_bootstrap(self):
        with patch.object(
            helper, "bootstrap_first_missing", return_value=True
        ) as bootstrap:
            for jobs in ([], [{"steps": [{"name": "Audit", "conclusion": "failure"}]}]):
                with self.assertRaises(helper.ReleaseError):
                    helper.bootstrap_after_approved_failure(
                        {"conclusion": "failure"}, jobs, SHA, "0.11.6", ["powerio-tnd"]
                    )
            bootstrap.assert_not_called()
            jobs = [
                {
                    "steps": [
                        {"name": "Publish in dependency order", "conclusion": "failure"}
                    ]
                }
            ]
            self.assertTrue(
                helper.bootstrap_after_approved_failure(
                    {"conclusion": "failure"}, jobs, SHA, "0.11.6", ["powerio-tnd"]
                )
            )

    def test_resume_never_republishes_live_version(self):
        with (
            patch.object(
                helper, "public_json", side_effect=self.registry(published=True)
            ),
            patch.object(helper.subprocess, "run") as run,
        ):
            self.assertFalse(
                helper.bootstrap_first_missing(SHA, "0.11.6", ["powerio-tnd"])
            )
            run.assert_not_called()

    def test_a_name_owned_by_someone_else_stops_the_release(self):
        def owners(login):
            return lambda url: (
                {"users": [{"login": login}]} if url.endswith("/powerio-tnd/owners") else None
            )

        with patch.object(helper, "public_json", side_effect=owners("squatter")):
            with self.assertRaises(helper.ReleaseError):
                helper.require_crate_ownership()
        with patch.object(helper, "public_json", side_effect=owners(helper.CRATE_OWNER)):
            helper.require_crate_ownership()
        # An absent name is the first-publication case, not a claim.
        with patch.object(helper, "public_json", return_value=None):
            helper.require_crate_ownership()

    def test_failed_publish_step_bootstraps_then_reruns_the_job_once(self):
        run = source_run(
            "crates.yml", event="release", display_title="PowerIO crates.io v0.11.6"
        )
        run.update(conclusion="failure", id=7, run_attempt=1)
        calls = []

        def api(path, method="GET", data=None, missing_ok=False):
            calls.append((path, method))
            return [] if path.endswith("/pending_deployments") else None

        class Done(Exception):
            pass

        with (
            patch.object(helper, "collection", side_effect=lambda *a: [run] if "crates.yml" in a[0] else []),
            patch.object(helper, "report_jobs", return_value=[]),
            patch.object(helper, "api", side_effect=api),
            patch.object(helper, "bootstrap_after_approved_failure", return_value=True) as boot,
            patch.object(helper, "wait_pause", side_effect=Done),
        ):
            with self.assertRaises(Done):
                helper.follow_publication(SHA, "b" * 40, "0.11.6", "v0.11.6", 0, ["powerio-tnd"])
        boot.assert_called_once()
        self.assertIn(
            (f"repos/{helper.REPO}/actions/runs/7/rerun-failed-jobs", "POST"), calls
        )

    def test_a_failure_without_bootstrap_is_not_retried_twice(self):
        run = source_run(
            "crates.yml", event="release", display_title="PowerIO crates.io v0.11.6"
        )
        run.update(conclusion="failure", id=7, run_attempt=1)
        attempts = iter([1, 2])

        def collection(endpoint, key):
            if "crates.yml" not in endpoint:
                return []
            return [dict(run, run_attempt=next(attempts, 2))]

        with (
            patch.object(helper, "collection", side_effect=collection),
            patch.object(helper, "report_jobs", return_value=[]),
            patch.object(
                helper,
                "api",
                side_effect=lambda path, *a, **k: [] if path.endswith("/pending_deployments") else None,
            ),
            patch.object(helper, "bootstrap_after_approved_failure", return_value=False),
            patch.object(helper, "wait_pause"),
        ):
            with self.assertRaises(helper.ReleaseError):
                helper.follow_publication(SHA, "b" * 40, "0.11.6", "v0.11.6", 0, ["powerio-tnd"])

    def test_pending_protection_prevents_local_publication(self):
        run = source_run(
            "crates.yml", event="release", display_title="PowerIO crates.io v0.11.6"
        )
        with (
            patch.object(helper, "collection", return_value=[run]),
            patch.object(helper, "report_jobs", return_value=[]),
            patch.object(
                helper, "api", return_value=[{"environment": {"name": "crates-io"}}]
            ),
            patch.object(helper, "bootstrap_first_missing") as bootstrap,
        ):
            self.assertEqual(
                helper.follow_publication(
                    SHA, "b" * 40, "0.11.6", "v0.11.6", 0, ["powerio-tnd"]
                ),
                3,
            )
            bootstrap.assert_not_called()


if __name__ == "__main__":
    unittest.main()
