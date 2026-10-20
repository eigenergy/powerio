# Contributing

PowerIO takes fixes, formats, validation evidence, and bug reports from
anyone. At present one maintainer, [@samtalki](https://github.com/samtalki),
reviews and merges every pull request, so review time is the scarce resource
here, not code. Most of this guide is about spending it well: agree on a plan
before you write a large change, keep each pull request to one change, and
show the checks you ran.

[AGENTS.md](AGENTS.md) holds the repository facts: the crate layout, the
formats, and the electrical conventions every change keeps. This file holds
the process.

## The short version

1. A small fix goes straight to a pull request against `main`, with a test
   that fails without it.
2. A change that makes a design decision starts as an issue with a plan. Wait
   for the maintainer to answer its open questions, and say on the issue that
   you are taking the work.
3. One change per pull request, targeting `main`. Past about 1,000 changed
   lines of hand written code and tests, agree on the size in the issue first.
4. Run the checks for what you changed while you work, and the full set in
   [Before you push](#before-you-push) before you ask for review.
5. Keep at most three pull requests open for review at a time, unless the
   maintainer agrees to more.
6. Push follow ups to the same branch. Never open a second pull request for
   a change that already has one.

## Reporting a bug

Open an issue with the PowerIO version (`powerio --version`,
`powerio.__version__`, or the crate version), the smallest input that shows
the problem, the command or call, what happened, and what you expected. For
a reader or writer defect, name the tool and version that produced the file,
and say what another importer (the vendor tool, PowSybl, MATPOWER,
PowerModels) does with it. If the file cannot be shared, describe the records
involved.

Report anything exploitable privately, as [SECURITY.md](SECURITY.md)
describes.

## Before you write code

### Changes that need a plan

Open an issue, or comment on the existing one, and wait for the maintainer's
answer before you write code when the change makes a design decision others
will build on:

- it adds a format, a crate, a `PioValue` type, or a `pio_*` entry point;
- it adds a public Rust type or module, rather than a function beside an
  existing one;
- it changes the PowerIO IR layout, the C ABI, or a convention results depend
  on, such as a sign, a unit, a per unit base, or an index order;
- it changes CI or the release workflows beyond the coverage the change itself
  needs, or the dependency policy;
- it implements an issue whose title starts with `RFC:`. An RFC stays open
  until the maintainer says how it concludes, and code written before then
  usually has to be redone.

A bug fix needs no plan, even when it changes numbers or a conversion matrix
baseline, as long as the description shows the evidence: the convention or
reference tool the old result disagreed with.

The plan names the target (format version, profile, or standard), says which
PowerIO types the source data maps to and what is reported as a diagnostic
instead, lists the fixtures and their licenses, says which independent tool
the result will be checked against, and says how the work splits into pull
requests. It ends with the questions the maintainer has to decide, numbered.

Issue [#456](https://github.com/eigenergy/powerio/issues/456) (distribution
CIM) is the model. The plan listed its open questions, the maintainer
answered them within a day, the base pull request merged as it stood, and the
rest of the work became a list of follow ups, each a pull request from
`main` that a contributor claims before starting.

### Claim the work

Before you start, search the open pull requests, drafts included, and the
issue thread for the same change. Then comment on the issue to say you are
taking it. If someone has already claimed it or opened a pull request, work
with them on theirs: review it, test it, or offer a commit. Two pull requests
for the same defect cost two reviews, and one of them is thrown away.

A claim with no visible progress for two weeks lapses. Ask on the issue
before you take over someone else's.

## Shape of a pull request

### One change

- A bug fix gets its own pull request against `main`. It never rides inside a
  feature or sits at the bottom of a stack, so it can merge first.
- Workflows under `.github/`, the CI and release scripts under `scripts/`, and
  dependency versions in `Cargo.toml` change only as far as the change needs:
  a new crate's build and publish steps, a validation job for a new format.
  Other CI, release, and dependency changes go in their own pull request,
  after an issue. Never weaken, skip, or work around an existing check to get
  green.
- A dependency bump rides with a change only when that change cannot build
  without it, and the description says so.
- Code you are not otherwise changing stays as it is: no reformatting,
  renames, or moves on the side.

### Size

A reviewer should be able to read a pull request in one sitting, which in
practice means up to about 1,000 changed lines of hand written code and
tests. The number starts a conversation rather than setting a limit: past it,
agree on the size in the issue before you open the pull request, and do not
split a coherent change only to get under it. Generated files do not count, as long as the description names them and the command
that regenerates them: the IR schema snapshot under `docs/schema/`,
`powerio-capi/include/powerio.h`, and regenerated examples and goldens.
Fixtures do not count either; they have their own limit under
[Test fixtures](#test-fixtures).

Larger work splits along the lines the plan agreed. A new format can land as
the model change, then the reader, then the writer, then the bindings, each
merged before the next one is opened for review.

### Stacks

Target `main` whenever you can. A pull request may build on one unmerged
parent, no deeper, and the first line of its description names the parent.
Each pull request in a stack has to make sense on its own.

A stack merges all or nothing: one stalled parent holds every child, and each
restack reruns all of CI on every pull request above it. A pull request from a
fork can only target branches of this repository, so it targets `main`; open
the next one as a draft until the first merges.

### Open pull requests per author

Keep at most three pull requests open for review at a time, maintainers
included, unless the maintainer agrees to more for a planned series. Further
work waits as a draft or on a branch. Ten green pull
requests opened in one afternoon do not merge sooner than three: they wait in
the same queue, touch the same files, and conflict with each other as each
one lands. Dependabot and the release bot are exempt.

### Update in place

Push follow up commits to the same branch. To pick up `main`, merge it into
your branch; a merge keeps the commits a reviewer has already read, which a
rebase and force push replaces. Do not rewrite a branch after review has
started unless the reviewer asks.

Never open a replacement pull request for a change that already has one. If a
branch is beyond repair, close the old pull request in the same step, with a
link to the new one, so two versions of one change are never open together.

Do not update a branch only to keep up with `main`: each push reruns about
forty CI jobs. If the only conflict is in `CHANGELOG.md`, resolve it in your
next push that changes the branch anyway, or once the reviewer says the pull
request is otherwise ready.

## Writing the pull request

### Title

Use a conventional commit subject: `feat:`, `fix:`, `perf:`, `refactor:`,
`docs:`, `test:`, `ci:`, or `chore:`, with an optional scope such as
`fix(psse):`. A squash merge makes the title the commit subject on `main`
(see [Review and merge](#review-and-merge)). Commits on the branch use the
same form.

### Description

A reviewer reads the description before the diff. Keep it as short as the
change allows, in this order:

1. What changes and why, in a few sentences, with the issue (`Fixes #N` or
   `Part of #N`).
2. The decisions you need from the maintainer, numbered.
3. What the pull request leaves out, and the follow ups.
4. Every file outside the main change and why it is there: a regenerated
   header, a schema snapshot, a baseline edit, a workflow. The description
   covers the whole diff. A reviewer who finds an unmentioned change has to
   start the review over.
5. New fixtures: the path, the bytes and lines from `wc -lc`, the source, and
   the license.
6. The checks you ran, as commands, and the checks you did not run, with the
   reason.

The [pull request template](.github/pull_request_template.md) has these
headings. Edit the description when the pull request changes instead of
appending update sections. Comment when something changes that the reviewer
needs to know, and leave out status narration.

### Changelog

A change a user can see adds one entry under `## Unreleased` in
[CHANGELOG.md](CHANGELOG.md), one to three lines long. A longer explanation
belongs in the pull request description and, at release time, in the release
notes. Never add to a released `## X.Y.Z` section.

## Before you push

While you work, run the checks that cover what you changed. Before you ask
for review, run the full set for your kind of change and list it in the
description.

For a change to Rust code, run:

```
cargo fmt --all --check
bash scripts/ci-clippy.sh
cargo test
cargo test -p powerio-capi
```

CI denies clippy warnings and requires Rust code formatted for edition 2024.
The clippy script covers the same feature combinations CI uses, including the
Python extension and the optional C features. For a change to documentation
only, run the checks under [Documentation prose](#documentation-prose) and
`bash scripts/terminology-gate.sh` instead.

`bash scripts/ci-mirror.sh` runs most of the Rust, Docs, and Python workflows
locally: the suites for each feature configuration CI tests, the terminology
and header gates, schema regeneration, packaging, the book, and the Python
tests. Run it before you mark a larger pull request ready. It leaves to CI the
MSRV build, `cargo deny`, Miri, the macOS, Windows, and FreeBSD jobs, and the
Julia binding, validation, PowSybl, and fuzz workflows.
[Testing and release checks](https://eigenergy.github.io/powerio/guide/contributor-workflow.html)
lists the extra checks for each kind of change.

For the Python bindings, build a wheel from the repository root with
`maturin build --release -o dist`, install it into a virtual environment, and
run `pytest python/tests`. Use the wheel rather than an editable install,
because the `powerio/` crate directory shadows an editable install when pytest
runs from the root.

CI is not the compiler. If your environment cannot build the workspace, for
example an agent that works only through a GitHub connector, open the pull
request as a draft, say so, and mark it ready once the checks pass somewhere.

### When CI fails for a reason outside your pull request

Some failures come from `main` or from outside the repository: a new RustSec
advisory against a locked dependency fails `cargo deny`, a dependency release
raises its minimum Rust version above the MSRV, a runner image changes. Check
whether the same job fails on `main` or on other open pull requests. If it
does, say so in one comment with a link to the issue that tracks it, and open
that issue if there is none. Do not fix it inside your pull request, and do
not open another pull request for it when one exists. Once the fix lands on
`main`, merge `main` into your branch.

The same holds for Dependabot. Comment on a failing Dependabot pull request
with what fails; do not open a pull request that replaces or repairs it.

## Coding agents

Agents are welcome, and much of PowerIO is written with them. Their output
follows every rule in this file, and these as well:

- The person who opens the pull request is its author. Read the whole diff
  before you ask for review, and be ready to answer for any line of it.
- Check the description against the diff. An agent's summary of its own work
  is often wrong about what it touched.
- Request a bot review (`@codex review`, Copilot, `@claude`) at most once
  after a substantive change. Bot reviews are advisory and replace none of
  the checks above.
- Point the agent at [AGENTS.md](AGENTS.md) for the repository facts and at
  this file for the process.

An agent makes code cheap to write and leaves it exactly as expensive to
review. The size, stack, and open pull request limits exist for that reason.

## Review and merge

- If a ready pull request has had no response for a week, comment on it once
  and mention @samtalki.
- The maintainer may push commits to your branch to finish it: a merge from
  `main`, review fixes, tests. On a fork, leave "Allow edits by maintainers"
  checked.
- The maintainer may fold your work into another pull request, with credit in
  the commit and the changelog, and says so on yours before closing it.
- Most pull requests are squash merged. A branch that carries commits from
  several people, such as one the maintainer finished, may be merged with a
  merge commit so that each author keeps their commits.
- Only maintainers apply the `release-blocker` and `semver-major` labels. Ask
  in a comment if you think one applies.
- Duplicate and superseded pull requests are closed with a link to the one
  that stays open.

### Pull requests from forks

- A first time contributor's CI waits until a maintainer approves the run.
- The conversion matrix report is posted as a comment only for branches in
  this repository. Run
  `cargo test -p powerio-cli --test conversion_matrix_report -- --nocapture`
  to see it locally.
- A PowerIO.jl companion branch has to exist in `eigenergy/PowerIO.jl`; see
  [Tandem changes with PowerIO.jl](#tandem-changes-with-poweriojl).

## Rules every change follows

### Fidelity and numerical changes

The domain rules (bus ids, per unit conventions, taps, reference buses, DC
signs) are in [AGENTS.md](AGENTS.md). Three apply to every reader, writer,
and calculation:

- Nothing is dropped silently. Whatever the target cannot represent becomes a
  `Diagnostic`.
- A conversion refuses rather than writes an electrically invalid
  placeholder, such as a line with zero impedance.
- A change to numerical semantics comes with tests and a short note in code
  or docs saying why. A performance change comes with before and after
  measurements.

`powerio-cli/tests/conversion_matrix_report.rs` pins the warning count of
every source and target pair in its `*_WARNING_BASELINE` arrays. An
intentional change edits the baseline in the same pull request, and the
description lists the cells that changed and why.

### Rust API changes

A release line takes compatible fixes, performance work, and additive change;
a break in the public Rust API goes into the next minor release. The Semver
workflow runs `scripts/check-semver.sh` on every pull request: it compares
each publishable crate with cargo-semver-checks against the newest stable
release tag at or below the workspace version. A new public field on a struct
that is not `#[non_exhaustive]`, a changed signature, or a removed item fails
it. Add instead: a new function or options type beside the old one, or a new
variant on a `#[non_exhaustive]` enum. Leave the old item without
`#[deprecated]`, because CI asserts that the workspace carries none. A break
meant for the next minor carries the `semver-major` label, which turns the
failure into a warning that still lists every break; bumping the workspace
version to that minor passes on its own.

To run the check locally, install it with
`cargo install cargo-semver-checks --locked` and fetch the release tags from
this repository (`git fetch --tags`, or from `upstream` on a fork) before
`bash scripts/check-semver.sh`.

### PowerIO IR changes

The DTOs in `powerio/src/stored/dto.rs` define the IR document layout. A new
type, field, or allowed name keeps IR version 2, and changing the meaning or
layout of an existing type waits for a new IR version. A published schema
snapshot under `docs/schema/pio-ir/2/` never changes. If `IR_SCHEMA_ID` in
`powerio/src/lib.rs` still names a published snapshot, the first layout
change after a release points it at the next release's directory and adds the
published one to the history in `powerio/tests/frozen_schemas.rs`; commit
71d3ed2 shows every file that moves with it. Regenerate the snapshot with
`cargo run -p powerio --example generate_schemas --features schema -- docs/schema`.
The [schema README](docs/schema/README.md) covers the rest.

### C ABI changes

`powerio-capi/include/powerio.h` is generated, so do not edit it by hand.
After any change to a public `pio_*` function, regenerate it:

```
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi --output powerio-capi/include/powerio.h
```

CI runs two header checks plus a C smoke test against the built library.
`scripts/capi-header-parity.sh` compares symbol names and runs in every feature
job. `scripts/capi-header-regen.sh` regenerates the header with cbindgen and
diffs it, which is what catches a reordered argument, a changed type, or a
changed struct field; it runs once. A breaking change to an existing `pio_*`
signature bumps `PIO_ABI_VERSION` (in `powerio-capi/src/lib.rs`; the header
`#define` follows from regeneration) and updates `scripts/check-capi-v7.sh`,
which pins the ABI number. The PowerIO.jl change that targets the new version
merges to PowerIO.jl `main` before the release tag, because the release
workflow tests PowerIO.jl `main` against the tagged library. Adding symbols
does not bump it. Each ABI generation gets a changelog entry.

### Tandem changes with PowerIO.jl

The `Julia binding` CI job builds this repository's C ABI and runs PowerIO.jl's
test suite against it. It runs when a pull request touches one of the library
crates the C ABI builds from. A pull request that changes something the two
projects share (JSON shapes, schema versions, `pio_*` behavior) can fail that
job against PowerIO.jl `main`. When that happens, push a PowerIO.jl branch
with the same name as the powerio branch; the job tests against the companion
branch when one exists, and a stacked pull request falls back to its parent's
companion. A new `pio_*` entry point fails that job until the companion
branch calls it or lists it in PowerIO.jl's `gen/unbound_entry_points.txt`
with a reason. Open both pull requests and merge them in either order.

The job looks for the companion only in `eigenergy/PowerIO.jl`. From a fork,
open the PowerIO.jl pull request from your fork with the same branch name and
ask on both pull requests for a maintainer to push the branch; do not change
the workflow to look elsewhere. Keep PowerIO.jl's test assertions on the
shared pieces at schema strength (same major, shape present) rather than byte
equality, so additive powerio changes do not fail the tandem job.

### Test fixtures

Use the smallest fixture that exercises the behavior under test. A new fixture
larger than 100 KiB needs explicit maintainer approval for that exact file,
given after you have put its byte count, line count, source, and license in
the pull request. Approval of a feature or test plan is not approval to vendor
its input data. Do not commit material without a license that permits
redistribution, and record the license in the fixture directory, as the
`License.md` files under `tests/data/dist/` do.

When source fidelity is itself under test, vendor real cases from the upstream
projects (MATPOWER, pglib) rather than writing bespoke fixtures: record the
source URL, upstream commit, and license, then pin the fixture bytes. For
parser and problem assembly tests, prefer synthetic fixtures. Before you
commit a fixture, run `wc -lc` on it and look at the pull request diff
statistics.

### Naming

The [language API guide](https://eigenergy.github.io/powerio/guide/languages.html)
defines the public verbs. `parse` reads a grid exchange representation, `emit`
produces one, and `serialize` and `deserialize` read and write PowerIO IR.
Derived calculations use `calc_*`, and `to_*` is reserved for semantic
transformations.

### Text encoding

Use UTF-8 with LF line endings and no BOM. CI rejects a UTF-8 BOM in any
file, and cp1252 mojibake in any file outside `tests/data`, where vendored
fixtures keep their committed bytes exactly as they are. It also rejects a
path that Windows cannot check out, such as a file named after a reserved
device (`aux`, `con`, `nul`). If you work on Windows, configure your editor
accordingly.

### Documentation prose

Public documentation describes behavior, inputs, outputs, conventions, and
failure conditions before it gets into implementation detail. Keep sentences
about ownership, units, indices, feature gates, and errors short. Use these
terms consistently: module, value, network, calculation instance, solution,
operating point, source ID, dense index, and diagnostic code.

Define an architectural rule once and link to it from everywhere else. Keep
design debate, implementation history, and proposed APIs in issues. Remove
sentences that repeat a heading or add no technical information. Reviewers
enforce these rules as prose; the project does not use an AI detector or
phrase blacklist.

After changing public prose or examples, run `mdbook build docs`,
`mdbook test docs`, and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features`;
CI documents with every feature on, so a doc comment behind a feature gate
can fail it while a default build passes. Regenerate the
committed schemas when their source rustdoc changes, and regenerate
`powerio.h` and run `scripts/capi-header-parity.sh` when C doc comments
change.

## Releasing

This section is for maintainers. A release is a version bump, a tag, and a
published draft. PowerIO.jl releases itself after each published release.

The release version lives in `[workspace.package]` and the workspace dependency
pins in `[workspace.dependencies]`. Update them together; the next Cargo command
updates `Cargo.lock`. Then:

1. Merge the bump with a `CHANGELOG.md` section headed exactly `## X.Y.Z`
   and release notes at `docs/release-notes/X.Y.Z.md` titled
   `# PowerIO X.Y.Z release notes`, and wait for main CI.
   `bash scripts/release-preflight.sh` fails on anything that still states
   the old version, such as the C ABI goldens under `tests/data/capi_matrix`.
2. Create an annotated `vX.Y.Z` tag on the tested main commit and push it.
   The release-binaries workflow tests PowerIO.jl `main` against the tagged
   library, checks the tag against the workspace version and that heading,
   and builds the C ABI tarballs. On each platform it then installs the
   tarball the way PowerIO.jl will after publication and runs the PowerIO.jl
   tests against it. Only then does it stage a draft GitHub release whose
   body is that section. If PowerIO.jl fails, merge the PowerIO.jl fix, then
   re-run the failed run. "Run workflow" builds and checks the tarballs but
   stages no draft.
3. Inspect the draft and its five binary assets, then publish the release.
   The release event fires the PyPI publish
   (python.yml) and the crates.io publish (crates.yml: powerio-core,
   powerio-tx, powerio-dist, powerio-prob, powerio-matrix, powerio, and
   powerio-cli, in dependency order). Both deploy through reviewer protected
   environments (`pypi` and `crates-io`; the protection lives in the repo
   settings). PyPI skips files it already has and crates.io skips versions
   already in the index, so if one of them fails partway you recover by
   running it again.
4. Publishing also starts PowerIO.jl's "Release" workflow, which releases
   PowerIO.jl X.Y.Z with no PowerIO.jl change: it sets the version, pins the
   release tarballs in `Artifacts.toml`, runs the Julia tests against them,
   pushes the release commit, and requests registration in General. Julia
   release notes go under `## Unreleased` in PowerIO.jl's `CHANGELOG.md`.
   [PowerIO.jl's release section](https://github.com/eigenergy/PowerIO.jl/blob/main/CONTRIBUTING.md#releasing)
   covers a failed run and how to hold a release.

A breaking C ABI change bumps `PIO_ABI_VERSION` first; see
[C ABI changes](#c-abi-changes) above.
