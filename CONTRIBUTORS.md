# Contributors

## Issue-derived contributions

Thank you to the people whose reports and proposals helped shape PowerIO.
This list records specific issue-derived contributions with a verified merged
implementation. It complements Git history rather than listing every code
contributor. Acknowledgment does not imply authorship or approval of every
change in the linked pull request.

- [archiechapman](https://github.com/archiechapman) reproduced OpenDSS
  geometry-backed lines receiving factory impedance and an invented conductor
  count. [#479](https://github.com/eigenergy/powerio/issues/479) led to the
  electrical-readiness gate and parser regressions in
  [#494](https://github.com/eigenergy/powerio/pull/494) and
  [#495](https://github.com/eigenergy/powerio/pull/495).
- [cameronkhanpour](https://github.com/cameronkhanpour) identified Windows
  checkout conversion breaking byte-exact fixture tests
  ([#72](https://github.com/eigenergy/powerio/issues/72), fixed in
  [#73](https://github.com/eigenergy/powerio/pull/73)), and proposed and
  implemented the raw DeepMind OPFData reader
  ([#257](https://github.com/eigenergy/powerio/issues/257),
  [#258](https://github.com/eigenergy/powerio/pull/258)). The dynamic-data
  proposal in [#111](https://github.com/eigenergy/powerio/issues/111) also
  informed the separate study API in
  [#660](https://github.com/eigenergy/powerio/pull/660); that issue retains
  work beyond the merged API.
- [ferrandor](https://github.com/ferrandor) reported that a PMD ENGINEERING
  export of a stock PMD case imported with no components
  ([#646](https://github.com/eigenergy/powerio/issues/646), fixed in
  [#738](https://github.com/eigenergy/powerio/pull/738)) and proposed reading
  and writing the PowerModelsDistribution MATHEMATICAL model
  ([#645](https://github.com/eigenergy/powerio/issues/645)), implemented as
  `pmd-math-json`.
- [frederikgeth](https://github.com/frederikgeth) reported a valid
  106,038-terminal solution that PowerIO could serialize but not read back
  ([#544](https://github.com/eigenergy/powerio/issues/544), addressed in
  [#550](https://github.com/eigenergy/powerio/pull/550)), and proposed and
  contributed the initial supported SINCAL reader/writer profiles
  ([#556](https://github.com/eigenergy/powerio/issues/556), completed by
  [#568](https://github.com/eigenergy/powerio/pull/568)). Fresh SINCAL writing
  remains experimental.
- [jd-foster](https://github.com/jd-foster) reported that the same PSS/E file
  parsed with a lowercase suffix but failed with an uppercase suffix
  ([#97](https://github.com/eigenergy/powerio/issues/97)), leading to
  case-insensitive extension detection in
  [#101](https://github.com/eigenergy/powerio/pull/101).
- [MohamedNumair](https://github.com/MohamedNumair) proposed CIM/CGMES
  coverage for both network families in
  [#237](https://github.com/eigenergy/powerio/issues/237), addressed by
  [#454](https://github.com/eigenergy/powerio/pull/454), contributed the
  distribution CIM reader and writer ported in
  [#631](https://github.com/eigenergy/powerio/pull/631), and wrote the
  MG-RAVENS JSON readers and writers for both profiles
  ([#27](https://github.com/eigenergy/powerio/issues/27)), whose commits
  this repository carries.
- [qian-harvard](https://github.com/qian-harvard) reported and cross-checked
  PSS/E reader defects against actual exports in
  [#592](https://github.com/eigenergy/powerio/issues/592), then implemented
  revision-34 node-column and two-winding magnetizing-admittance corrections
  in [#593](https://github.com/eigenergy/powerio/pull/593). The wider issue
  remains open for its other findings.

These are selected merged contributions for each listed person, not a complete
catalog of their work. See [Contributor credit](CONTRIBUTING.md#contributor-credit)
for how to add an acknowledgment or preserve justified commit coauthorship.
