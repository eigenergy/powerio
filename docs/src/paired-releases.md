# Paired releases

PowerIO and PowerIO.jl use matching versions in the 0.11 series. A maintainer
chooses the version and reviews both changelogs. Automation freezes the two
source commits, prepares the binaries and Julia artifact references, and
presents one draft release. Publishing that draft approves publication to
crates.io, PyPI, and Julia's General registry.

## Prepare and approve

1. Run **Prepare paired release**, stage `changes`, with the intended version.
   It opens draft `release/X.Y.Z` PRs in both repositories. Write the curated
   changelog entries, review the compatibility implications, and merge both
   PRs after CI passes. No version or compatibility assessment is inferred.
2. Run the same workflow, stage `candidate`. It checks both `main` versions,
   notes, and CI, then creates an annotated tag containing the exact source
   pairing. The binary workflow tests the pinned Julia source and builds the
   five platform archives.
3. **Complete paired draft** downloads and verifies the draft archives, runs
   the Julia checks, and records a candidate commit changing only
   `Artifacts.toml`. It attaches `release-manifest.json` and displays both
   changelogs, source commits, and test evidence in the draft release.
4. Inspect that complete draft and publish it. This is the publication
   approval for both repositories. Do not publish a draft that still says
   candidate preparation is in progress. Registry workflows reject a release
   without its complete, verified manifest.

The manifest records the source pairing, exact Julia candidate tree,
`Artifacts.toml` hash, binary hashes, changelog hashes, and validation run.
The tag annotation and published immutable assets identify the approved
candidate. Editing release-body prose does not authorize different code.
New `main` commits do not change the candidate or block its registration.

## Retry and recovery

The daily reconciliation also retries incomplete paired builds and drafts.
If a draft upload finishes before its validation workflow records success,
completion reruns the Julia tests and updates only the draft manifest's validation
reference. It verifies that every source commit, artifact hash, and note remains
identical. Published releases require completed validation and remain immutable.
Rerun **Complete paired draft** for an immediate retry of an interrupted draft. It preserves
existing candidate commits and assets. If source fixes require a different candidate, merge and test those fixes,
then explicitly select `replace-unpublished` in **Prepare paired release**.
That operation removes only the unpublished draft and tag, preserves the
previous Julia candidate commit, and prepares a fresh draft for review.
It refuses a published or registered version. No workflow force pushes a
release branch or replaces published data.

**Reconcile paired releases** runs after publication and daily. Manual dispatch
accepts an existing published tag. It checks the manifest, resumes missing or
failed package publication, posts a deduplicated registration request on the
exact tested Julia commit, and retries TagBot when General has accepted the
version but its Julia release is absent. Missing credentials, unexpected
assets, mismatched hashes, and a different registered tree fail explicitly.
Waiting for General is reported separately from completed publication.

A synchronization PR brings the approved artifact references back to Julia
`main`; its timing does not affect the already approved package. General
registration remains subject to that community's checks and review.

## One-time activation after v0.11.2

Keep the existing release route active until v0.11.2 finishes and the paired
workflow PRs pass CI. The new workflows require `PAIRED_RELEASES=true` in both
repositories. The legacy Julia artifact and registration jobs stop when that
variable is enabled, leaving one release authority.

[Register the dedicated GitHub App](https://github.com/organizations/eigenergy/settings/apps/new?name=eigenergy-powerio-releases&description=Coordinate+reviewed+PowerIO+and+PowerIO.jl+releases.&url=https%3A%2F%2Fgithub.com%2Feigenergy%2Fpowerio&public=false&webhook_active=false&request_oauth_on_install=false&contents=write&pull_requests=write&actions=write&workflows=write) owned by eigenergy, installed only on
`powerio` and `PowerIO.jl`. Grant repository Contents, Pull requests, and
Actions write permissions, plus Workflows write to preserve a pinned commit
when later workflow edits advance main. Generated commits do not edit workflow
files. Metadata is read-only. Disable webhooks.
Store its ID as `POWERIO_RELEASE_APP_ID` and its private key as
`POWERIO_RELEASE_APP_PRIVATE_KEY` in PowerIO Actions settings. Workflow tokens
are short-lived installation tokens. Keep existing trusted publishing and
Julia's `TAGBOT_SSH`; the App is not a package registry credential.

Run the **Release App access** workflow on current `main`. Token creation
checks that the required App permissions were granted; the read-only API calls
check access to both repositories. Then run
`python3 scripts/activate_paired_releases.py --check` to inspect the
settings, then `--activate` to enable immutable releases and paired dispatch.
The activation command preserves publishing environment names and tag rules,
and removes their separate reviewer lists only after checking that the paired
workflow files, App configuration, and immutable release setting exist.
The human Publish release action then serves as the paired publication
approval. Do not activate while a legacy release is still running.

The old `.github/powerio-release.toml` is retained only for legacy recovery.
Paired releases do not read it, require its ready flag, or ask maintainers to
refresh its checksum. Its historical helper tests remain for old releases.
