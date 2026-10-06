# CIM bench integration and contribution record

The integration targets Haigutus/cim-bench's active Svedala CGMES 3.0 and
RealGrid CGMES 2.4.15 datasets. Distribution CIM issue #456 is separate.

## Community work reviewed (2026-10-06)

- [Mohamed Numair (@MohamedNumair)](https://github.com/MohamedNumair) suggested CIM bench in [issue #456](https://github.com/eigenergy/powerio/issues/456#issuecomment-6022871135).
  His `core/cgmes` commits `a26b60452a853126f84b9c793e2cdc603ec43081` and
  `11163b32faf87dbe8a64a94a388a771e36c6df8f` supplied the original reader
  and writer already incorporated into PowerIO. The existing
  [contribution audit](../powsybl/cgmes-contribution-audit.md) records that
  lineage. His `dist/cim-61968` work remains outside this integration.
- Burhan Abdullah's active geometry and electrical-readiness branches concern
  distribution modeling. The electrical-readiness comparison changes only
  `powerio-dist`; no code from those branches is used here.
- `krishnasandeepaxe190/powerio` has no commits ahead of upstream main.
- Markus Mirz's [CIM bench PR #18](https://github.com/Haigutus/cim-bench/pull/18),
  head `4fe634afd0ab30c2437a2c47a4e2772970eb57a0`, adds cimoxide export and
  changes its query paths. Measure that pending revision separately from
  upstream's older adapter; do not claim victory over a stale comparator.
- Casper Eijkens' CIMD fork and PR #3 concern the separate CLI family;
  upstream PR #10 records its incomplete-profile limitation. Alex Anderson's
  CIM-Graph contribution was merged as PR #1; his fork has no ahead commits.
- CIM bench's adapter interface and benchmark generator are existing work by
  Kristjan / Haigutus and the repository's contributors, under its MIT license.

No unmerged community implementation is copied into the new PowerIO APIs.
If subsequent optimization imports a patch, preserve its author and commit
provenance. If substantially adapting code, acknowledge the adaptation and
add verified coauthor credit to the relevant commit. The prerequisite and CGMES follow-up commits credit Mohamed Numair as coauthor
using the identity verified from his original commit.

## Input provenance

Keep datasets in CIM bench's submodules, not PowerIO fixtures:

- `entsoe/relicapgrid`: `f8f1290611ef3192996773e49c0dd679b9d7b5ab`.
  Its LICENSE.md states CC BY-SA 4.0 and credits ENTSO-E, Svenska kraftnät,
  and the collaborating organizations named there. Preserve those notices.
- `Haigutus/triplets`: `cbcc1bdc143b7199416f456c007e56f4619117a7`.
  RealGrid LFS SHA-256 is
  `960025ef0f4aba40b6989c8c61736d551062a6d042d20a9089dbc0224eb7e100`;
  its ZIP contains four XML files totaling 86,523,278 bytes.
  The enclosing repository's MIT license is not treated as a relicensing
  of ENTSO-E test configurations; do not vendor the dataset here.

## Runtime interface

Primary input and cumulative acquisition default to 1 GiB, configured through
`POWERIO_MAX_PRIMARY_BYTES` and `POWERIO_MAX_REFERENCED_BYTES`. Expanded
archive content uses the acquisition limit. There is no CGMES-specific setting.
Existing archive path, entry-count, nesting, and compression-ratio checks remain.

Python `BalancedNetwork.component_counts()` returns native lines (excluding
transformer branches), generators, loads, and source-hierarchy substations
without constructing row dictionaries. Native loads can include equivalent
injections: Svedala has 73 load objects and seven such injections, yielding
80 native loads. RealGrid counts are 7,561 lines, 1,347 generators, 6,687
loads, and 4,875 substations. Svedala has 97 lines, 39 generators, and 57
substations including the common/boundary data.

`PioModule.sever_source()` returns a copy without retained input buffers,
preserving values and common records. Use it outside fresh-export timing and
report its preparation time/memory. `from_value()` preserves source retention
and does not force fresh output. Replay must be labeled separately from
fresh CGMES 3.0 serialization.

Performance claims require matched local reruns and correctness checks;
historical upstream Linux timings are not comparable to this Mac/VM host.
