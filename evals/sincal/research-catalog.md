# Native SINCAL research corpus

Research update: 2026-10-07. This catalog records discoveries and inspection
results, not supported PowerIO formats. The additional CSIRO and repository
cases were not added to `tests/data`; their downloads and decoded tables remain
outside the repository in `/private/tmp/powerio-sincal-research`. The earlier
small licensed SimBench fixture is tracked separately.

## Search coverage and disposition

The [second publication-focused pass](#publication-focused-search-2026-10-07)
records additional paper, thesis and repository checks. It found no additional
confirmed, redistribution-cleared native unbalanced collection.

The search covered GitHub repository names, descriptions and README content;
native extensions and archive markers; SimBench; CSIRO and Research Data
Australia; targeted Zenodo, Figshare and Mendeley searches; IEEE feeder papers;
vendor documentation; and converter implementations. Broad GitHub searches
stemmed “sincal” into unrelated “since” matches, so their result totals are not
an inventory of SINCAL projects. Native-file search also misses binary archives.
Recursive repository trees and archive inventories supplied the useful evidence.
The archive-service searches produced no additional verified native collection;
this is a search outcome, not proof that none exists.

| Source | Native material found | License assessment | Intended use |
| --- | --- | --- | --- |
| [CSIRO Australian feeders](https://doi.org/10.4225/08/5631B1DF6F1A0) | All 19 Access databases inventoried, paired PowerFactory models and profiles; seven databases contain mixed-phase terminals and unbalanced results | Publisher explicitly applies CC BY 4.0 to the collection; attribution and modification notices required | Best new licensed unbalanced corpus; external validation first |
| [MilosFTN IEEE 18/33](https://github.com/MilosFTN/Models-of-the-IEEE-18--and-33-bus-test-systems) | Two RAR archives containing SQLite projects, other tool models/results, and one coupling sidecar | No license in repository root, README or archive inventory; a citation request is not redistribution permission | Temporary inspection only; no fixtures or copied sidecar |
| [Brandalik MATLAB LPC tool](https://github.com/lik1212/Matlab2Sincal_LPC-Tool) | European LV and S1a Access projects, both inspected | Root MIT license; underlying benchmark ownership/third-party notices still need checking | Candidate small mixed-phase model; do not assume code licensing resolves inherited model rights |
| [Benedikt Schmidt LoadFlow](https://github.com/benediktibk/loadflow) | 67 Access model databases, a C# connector, and Siemens manuals | `source/README` licenses applications/libraries under GPL, with GPL-3.0 text in `source/LICENSE`; no blanket license established for Siemens manuals or all model data | Schema reference and candidate external cases; no copied implementation or vendored manuals |
| [Student power-flow study](https://github.com/onuremirhancon/Power-Flow-Analysis-by-Using-PSS-Sincal) | SQLite project and report; database inspected | No redistribution license found | Temporary schema-16.0 inspection only |
| [ForwardBackwardSweep](https://github.com/Mehmet-Emre-Dogan/ForwardBackwardSweep) | SINCAL comparison CSV/XLSX and MATLAB analysis; no native project in inspected tree | No license found | Possible external comparison reference, not a fixture |
| [Sincal FaultReport](https://github.com/chaiwat-api/Sincal_FaultReport) | Automation code and presentation; no native model in inspected tree | MIT | Automation reference, not a test-case source |
| [SimBench](https://simbench.de/en/download/datasets/) | Native SQLite archives and matching CSV models | ODbL 1.0 / DbCL 1.0 for data | Existing small licensed balanced regression remains useful |

Existing references remain relevant: pandapower's BSD-3-Clause converter
branch uses SINCAL COM to initialize projects; the MIT MATLAB grid creator
copies templates; GPL Sigrid operates through CIM. Zepben documents a
template-based exporter, but its public documentation does not establish an
open-source exporter or redistributable template. None independently proves
our proposed standalone writer contract.

Siemens' April 2015 *Database Interface and Automation* manual, §3.2,
printed pp. 37–39, documents `SinDBCreate.exe`, supplied with the SINCAL
installation, for creating blank databases without the graphical interface.
It accepts electrical network type `E`, defaults to network database `NET`,
and can create a `.sin` companion. The documented database systems are Access,
Oracle, SQL Server and SQL Express; SQLite is absent from that revision.
This is a concrete future source of authoritative blank projects, but it
still needs an installation and does not establish standalone SQLite output.
[Siemens manual, hosted copy](https://manualzz.com/doc/2231110/pss-sincal-database-interface-and-automation).

## CSIRO: authentic unbalanced evidence

Publisher: Berry, Adam; Collins, Lyle; Oliver, Erin; Perfumo, Cristian (2015),
*Representative Australian Electricity Feeders with load and solar generation
profiles*, v1, CSIRO, DOI `10.4225/08/5631B1DF6F1A0`.
[Primary collection](https://data.csiro.au/collection/csiro:15331),
[license](https://creativecommons.org/licenses/by/4.0/).
The collection describes SINCAL 11.5-or-later models obtained from the National
Feeder Taxonomy Study, with PowerFactory conversions by DIgSILENT. The user
explicitly authorized acceptance of the publisher's download disclaimer for
this research. This does not authorize committing oversized fixtures.

Files are under `DataRelease/FeederModels/Sincal/Representative NN_files/`.
MDB Tools 1.0.1 decoded the files read-only. An independent preliminary
`access-parser` 0.0.6 pass agreed with Representative 01's topology counts and
terminal codes, but that Python reader has misdecoded some variable-length
text elsewhere; MDB Tools output is the working research source.

| Observation | Representative 01 | Representative 19 |
| --- | --- | --- |
| Database bytes | 60,686,336 | 4,476,928 |
| `Version_No` | 11.5 | 11.5 |
| Base variants | 1 | 1 |
| Nodes / elements / terminals | 795 / 1,033 / 1,818 | 26 / 33 / 58 |
| Lines / loads / two-winding transformers / infeeder | 533 / 247 / 252 / 1 | 25 / 7 / 0 / 1 |
| Terminal codes | 1–7, including single phases and phase pairs | 7 only |
| Unbalanced node result rows | 38,220 | 0 |
| Balanced node result rows | 0 | 26 |
| Coupling / neutral-point records | 0 / 0 | 0 / 0 |

Representative 01 SHA-256:
`eb1fb68ba654f70f5dfb0a39dcd04831b3f1db42aa0448c29824da2997e42b40`.
Representative 19 SHA-256:
`5540e956effe9a8e6e691f834e79f62c86d96924fdd38092e5fd5c6fa84789c4`.

Representative 01's terminal histogram is L1=516, L2=435, L3=405,
L12=83, L23=80, L31=75 and L123=224. Node `Flag_Phase` is NULL throughout:
requiring that field to encode every connected phase would reject this case.
All 247 loads use constant impedance (`Flag_LoadType=1`) with S/power-factor/
absolute-voltage input (`Flag_Lf=4`). Its transformer codes are 14, 59, 71 and
73. Some per-winding tap columns differ, but `Flag_Tap=0` selects the common
tap and the historical tap results agree with it. Those differing columns
are inactive inputs, not evidence of enabled independent regulation. Lines have direct nonzero zero-sequence
parameters. This is substantially more demanding than constant-PQ loads on
three-phase lines.

The accompanying `LF.001.html` identifies Platform 11.5 Update 5 and an
unbalanced component-method calculation completed on 2015-10-20. Its loading
counts agree with the database topology. However, `ULFNodeResult` contains
780 nodes at each of 49 `ResTime` values (0 through 24 hours in half-hour steps),
all dated 2013-06-17. Those historical states must be matched to their actual
profiles, switching, taps and source settings before using them as numerical
oracles. They are not the result of a fresh calculation performed here, nor
automatically the results of that 2015 log. All 15 nodes absent from those
results have no attached terminal in the selected variant; no connected node
is missing. Their IDs are 565, 566, 675, 676, 1322, 1323, 1590, 1591, 1671,
1672, 1673, 1682, 1683, 1863 and 1864.

Further table inspection finds 741 daily profiles (three per load) and 35,568
profile points (48 half-hour points per profile, 0 through 23.5 hours).
`OpSer.Flag_Typ=3` means absolute power, not a multiplier; `OpSerVal.P/Q` use
kW/kvar, unlike the load table's MW/Mvar. The first profile is named
`96 - summer`, and the load references it through `DayOpSer_ID`. The database
manual documents these units and enums on printed pp. 84–85 / PDF pp. 90–91;
the input manual discusses absolute profiles and cyclic duration on printed
pp. 292–295 / PDF pp. 302–305. Do not apply the load table's SI scale to these
profile rows or treat the unused seasons as simultaneous load contributions.

There are 12,348 tap-result rows (252 per state), 89,033 unbalanced branch
rows (1,817 per state), and 49 accuracy records with the same historical date
and time grid. Every accuracy record has `Flag_Result=1` (load-profile run)
and `Flag_State=2` (limit violation), as documented on printed p. 168 / PDF
p. 174. This is not the success-state code. For example, the first record
reports 206 low-voltage violations. These observations strengthen the state
inventory but do not establish that the present inputs reproduce those
results, or that a limit violation is a failure to converge.

The repeatable external state audit in `audit_states.py` confirms all 49 states
have the same connected-node coverage and no missing load-profile references.
Only terminal 197 lacks branch results; it belongs to load 197, connected as
L12 at node 674. The incident line connects only L1 there, and the stored node
result reports phase L1 only. This is consistent with an unavailable load
phase pair, but is not proof of the solver's missing-phase rule.

`check_z_profiles.py` compares all available 246 load results at all 49 times
against their referenced absolute profiles and recorded voltages. All 12,054
comparisons match at `1e-9` relative / `1e-6` W/var absolute tolerance; the
maximum absolute residual is `8.731149137020111e-11` W/var. The 24-hour state
uses the cyclic zero-hour sample. Phase-pair loads use their recorded line-line
voltage divided by `Ul`; single phases use phase voltage divided by
`Ul/sqrt(3)`; three-phase total power is divided equally among phases. The
small residual supports this local interpretation and the profile's absolute
power units. Recorded voltages are inputs to this check, so it cannot certify
network equations, a fresh solve, or historical transformer/source alignment.

The transformer winding tables in the manuals also resolve a topology trap:
terminal codes select coil pairs rather than direct phase sets. In this case,
233 YNd1 transformers use a single W1, W2 or W3 coil pair, so a terminal code
of 1 on both sides does not mean a one-conductor secondary. Its delta side
requires two phase conductors. The four Y0 and two D0 devices are
autotransformers; their galvanic paths must not be replaced by isolated YY/DD
transformers. No copied manual or additional database was vendored for these
implementation checks.

Representative 19 is the smallest project folder shown by the portal
(approximately 4.51 MiB), but its actual database is still far above 100 KiB
and is balanced. Do not substitute it for unbalanced acceptance coverage just
because it is smaller. Keep all these cases in the external corpus. A
future reduced CC BY case must record exactly what was removed, retain credit,
and remain distinguished from an original SINCAL export.

## Complete CSIRO database inventory

All 19 databases were downloaded to temporary research storage and decoded
read-only with MDB Tools on 2026-10-07. Their combined size is 870,703,104 bytes.
The repository contains only the derived [inventory](csiro-inventory.json),
with source paths, SHA-256 hashes, exact sizes, raw terminal-flag histograms
and table counts. No additional native file or exported table was vendored.
Every database has `Version_No=11.5` and `Calc_Type=1`.

| Representative | Bytes | Variants | Nodes | Lines | Loads | Two-winding transformers | Unbalanced node-result rows |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 01 | 60,686,336 | 1 | 795 | 533 | 247 | 252 | 38,220 |
| 02 | 136,704,000 | 1 | 1,749 | 1,179 | 574 | 575 | 85,701 |
| 03 | 76,427,264 | 1 | 785 | 774 | 297 | 11 | 38,465 |
| 04 | 118,075,392 | 1 | 625 | 390 | 235 | 235 | 30,625 |
| 05 | 88,170,496 | 1 | 922 | 912 | 458 | 7 | 42,630 |
| 06 | 4,820,992 | 1 | 167 | 113 | 53 | 51 | 164 |
| 07 | 55,185,408 | 1 | 343 | 232 | 112 | 111 | 16,562 |
| 08 | 18,518,016 | 1 | 263 | 263 | 39 | 3 | 0 |
| 09 | 69,181,440 | 1 | 621 | 620 | 67 | 0 | 0 |
| 10 | 62,193,664 | 14 | 200 | 99 | 322 | 0 | 0 |
| 11 | 53,116,928 | 14 | 164 | 85 | 210 | 0 | 0 |
| 12 | 29,548,544 | 1 | 191 | 187 | 26 | 0 | 0 |
| 13 | 19,111,936 | 1 | 1,769 | 1,763 | 190 | 0 | 0 |
| 14 | 39,718,912 | 16 | 57 | 55 | 112 | 2 | 0 |
| 15 | 6,701,056 | 1 | 90 | 89 | 11 | 1 | 0 |
| 16 | 7,385,088 | 1 | 87 | 88 | 11 | 0 | 0 |
| 17 | 14,192,640 | 1 | 126 | 129 | 10 | 0 | 0 |
| 18 | 6,488,064 | 1 | 251 | 242 | 13 | 38 | 0 |
| 19 | 4,476,928 | 1 | 26 | 25 | 7 | 0 | 0 |

These are **all stored rows**, not effective network sizes after variant
inheritance. In particular, 10, 11 and 14 contain multiple variants and load
overrides. Count CSV records with `csv.DictReader` over `mdb-export` output,
not physical lines, and check database hashes before reproducing the inventory.

Representatives 01–07 have mixed terminal flags and unbalanced results.
The other databases have three-phase terminal flags, except 13, where 3,666
of 3,717 terminal flags are blank. Do not infer a default for those blanks
from this inventory. Across **all 19** databases, `CouplingData`, `CoupledLine`,
`NeutralPointImp` and `LineSeg` are empty, and no terminal uses neutral code 8.
Thus expanding this collection did not supply the linked explicit-neutral or
coupling case required by the roadmap. This observation does not exclude
implicit grounding or zero-sequence behavior in other tables.

Representative 06 is the smallest newly verified mixed-phase candidate:
4,820,992 bytes, still above the fixture limit. Its single historical result
state has raw date `07/03/13 00:00:00`, time zero and result kind 0; there are
no `OpSer` or `OpSerVal` rows. The existing state audit finds 164 result nodes,
one missing connected node (55), two missing unconnected nodes (7 and 8),
three terminals without branch results (44, 149, 150), and all 51 tap rows.
The accuracy state is 2 with 17 low-voltage violations. The initial profile-only line harness compared zero states here. After
checking the documented ordinary load-flow kind 0, the circuit harness now
admits kinds 0 and 1 separately and still refuses event sub-times and other
calculation kinds. The subsequent comparisons are recorded below.

Next corpus priorities are 06 for a compact static unbalanced case, 02–05
and 07 for independent source/transformer/line counterexamples, and 10/11/14
for variant inheritance. Keep explicit-neutral/coupling acquisition and fresh
writer acceptance as separate requirements; these 19 databases do not close
either one.

## Representative 06 ordinary load-flow checks

The April 2014 Database Description manual defines `Flag_Result=0` as ordinary
load flow and `1` as load profiles in `LFAccurResult` (printed p. 168),
`ULFNodeResult` (p. 176), `ULFBranchResult` (p. 178) and transformer tap results
(p. 172). Local line/source/transformer checks now accept both kinds while
retaining the complete date/time/kind/event key. A profile row cannot supply a
missing ordinary-result row at the same time. Event sub-times and other kinds
remain outside the check; constant-Z profile scaling still requires kind 1.

For Representative 06, all 112 closed-port lines pass the stated pi equations:
102 three-phase and ten independent single-phase lines. The maximum port
current error is 3.032758e-7 A (below the unchanged 1e-6 A threshold), and the
maximum shunt-balance residual is 5.388950e-11 A. The remaining line, element
111, has an open port at node 54; its remote node 55 and its two ports have no
results. This explains their relation to the missing-result inventory but
is not a general native de-energization rule.

Source element 1 provides another counterexample to applying the stored direct
zero-sequence impedance as the load-flow impedance. The input is
`0.366006+j6.123122` ohm, while the historical zero-sequence voltage is nearly
zero and zero-sequence current is nonzero. The resulting direct-impedance
residual is 0.7823293 V; positive/negative voltage residuals are below
3.81e-9 V and current magnitudes agree within 2.85e-14 A. Do not replace that
impedance with the observed ratio. Historical alignment and native source
semantics remain unproved. The source checker now also rejects unavailable
node/branch statuses and preserves status 2 (limit violation).

No transformer is compared: 17 fail the full-winding-port requirement, and
all 34 full-winding Dyn11 candidates have a blank `Flag_Tap`. All 51 current
rows have blank `Flag_Tap`, `Flag_roh=1` and nominal `roh=rohm=0`. Treating
blank as zero would be an unverified schema-default assumption, even though
these other fields look nominal. It remains a specific follow-up for schema
11.5, not a reason to weaken the checker silently.

Reports and CSV hashes remain in temporary research storage as
`csiro06-state-audit.json`, `csiro06-line-check.json`,
`csiro06-source-check.json` and `csiro06-transformer-check.json`.
These local equations consume stored voltages; they establish neither a fresh
solve nor native writer acceptance. Four new original synthetic tests cover
ordinary states, kind isolation, event rejection and source result status;
the full Python evidence suite has 46 tests. No new native fixture was added.

## Additional independent checks: Representatives 03, 04 and 07

The unchanged profile-identity audit rejects 03 and 04 before numerical
comparison. These are conflicting values, not harmless duplicate exports:

- 03: profile 592 at 5.5 hours has `OpSerVal_ID` 66396 and 28380, with
  P/Q of 1.338553/0.648291 and 1.41389/0.332185 respectively.
- 04: profile 315 at 0.5 hours has IDs 41762 and 74018, with P/Q of
  1.392128/0.674239 and 1.375517/0.666193 respectively.

Both pairs have variant 1, `Flag_Variant=1`, `Flag_Curve=1`, `Factor=1` and
blank `Op_ID`. Do not select a row by export order or average its values.
Profile precedence or source-data repair needs independent evidence.

Representative 07 completes the identity audit. Of its 232 lines, six have
open ports and one full-phase line (1061) lacks usable required phase results
at all 49 states. The remaining comparisons provide a broader counterexample:

| Connection | Compared states | Mismatched states | Maximum current residual | Maximum shunt-balance residual |
| --- | ---: | ---: | ---: | ---: |
| Full three-phase | 5,635 | 44 | 1.600259e-5 A | 4.356717e-9 A |
| Coupled L23 | 1,176 | 1,176 | 0.001311462 A | 0.002619118 A |
| Coupled L31 | 1,715 | 1,715 | 0.001885875 A | 0.002454716 A |
| Coupled L12 | 2,499 | 2,499 | 0.008209021 A | 0.009770451 A |

The 110 coupled phase-pair lines fail all 5,390 comparisons. Three-phase
discrepancies belong to element 977 and occur in 44 of its 49 states; this
element's shunt-balance residual remains below tolerance. The unchanged
1e-6 A criterion therefore reports 5,591 passing and 5,434 mismatching states,
plus the separately unavailable 49 states. Recovered current magnitudes
agree within 1.422e-13 A. This reinforces the charging discrepancy for coupled
reduced phases while also requiring input/type alignment for line 977.

Source element 3 has direct input `0.143141+j1.659248` ohm and zero-sequence
hypothesis residuals of 0.0518342–0.0528234 V across 49 states. Transformer
checks compare no states: 81 devices lack two full-winding ports, while 30
full-winding candidates have blank `Flag_Tap`. No numerical default was
substituted. The full external report, including CSV hashes, is
`csiro07-circuit-checks.json`; 03 and 04 remain explicit audit refusals.

## CSIRO line-current evidence and reduced-phase counterexamples

`check_line_circuits.py` audits all 533 Representative 01 lines against the
same variant-local historical state inventory. Six lines have an open port
and are outside the stated closed-port hypothesis. The other 527 lines have
49 usable states each. Current magnitudes recovered from port powers and node
voltages agree with recorded magnitudes within 2.85e-14 A.

The phase-domain pi model applies current voltage-level temperatures and the
documented frequency, length, parallel and dielectric corrections. Its series
impedance and half-shunt matrices come from the stored positive/zero sequence
inputs. The reduced-phase hypothesis takes their principal submatrices in
native phase order, including L31 as `[3, 1]`.

| Connection class | Lines / states | Maximum port-current error | Result at 1e-6 A tolerance |
| --- | --- | --- | --- |
| Independent single-phase | 438 / 21,462 | 4.75e-10 A | All match |
| Full three-phase | 87 / 4,263 | 9.83e-9 A | All match |
| Coupled L31 and L23 | 2 / 98 | 0.002642 A | All disagree |

The single-phase inputs have `r0=r`, `x0=x`, `c0=c` and zero dielectric loss,
so each phase is an independent circuit. They cannot distinguish competing
missing-conductor treatments for coupled lines. Elements 976 and 1042 provide
that counterexample: maximum current discrepancies are approximately 0.0026411
A and 0.0022220 A. A preliminary calculation retaining the full pi circuit
with floating unconnected phase coordinates also failed (maximum 0.002680 A).
Neither hypothesis is promoted to runtime support for coupled reduced phases.
Do not fit new parameters or treat the mismatch as proof of a particular
native algorithm or historical input change.

The complete report remains external as `csiro01-line-circuit-check.json`.
It records raw calculation settings, unresolved equipment type references,
missing/unusable states and CSV hashes. This is local circuit evidence using
recorded voltages, not a fresh solve, input-state alignment proof or schema
compatibility claim. Additional MDB Tools exports have these SHA-256 values:

- `Line`: `507cb49e86e080a2ef60509f470f48e9376e98ecb870fa6cd30a3ecd176602d0`.
- `VoltageLevel`: `dc094da8a63b892975fa39b6d70a4c0995b191fbc813dd93609b62653550a849`.
- `LineSeg`: `4b36711b2dfd9e7fe72c10e404e48b24ac36026a98c0c777380148a72d0cae2d` (no records).

The added shunt-balance check evaluates `If+It-Yhalf*(Vf+Vt)`, independently of
series R/X. Elements 976 and 1042 fail it at all 49 states, with maximum
residuals 0.0024569 A and 0.0025508 A. Thus changing only series impedance
cannot repair both port-current equations of this pi hypothesis. This narrows
the evidence needed to charging/input-state alignment without inferring a
replacement circuit.

The newly inspected [April 2014 Siemens Multiple Faults manual](https://raw.githubusercontent.com/benediktibk/loadflow/2d95494327123de7eaa9c6fe0bc09990038ee364/documents/external/sincal/Mehrfachfehler.pdf),
printed pp. 8–9, explicitly eliminates absent series currents. Its formulas
are equivalent to restricting the series impedance matrix. This supports
series-only reduced-phase mapping, but does not resolve the combination with
pi charging. The 554,281-byte manual was downloaded to temporary research
storage and its equations inspected visually; it was not vendored.

Seven original synthetic tests exercise the checker. No CSIRO database, CSV,
historical result or copied manual was added to the fixture directory.

## Single-coil transformer experiments and measurement bounds

The documented coil topology alone does not establish how the full-transformer
positive/zero-sequence measurements become a partial-winding circuit.
`check_transformer_coils.py` explicitly tests an independent grounded-primary /
phase-pair-secondary YNd1 pi circuit under two rating interpretations. With
`Z=Un2²/Sn*(ur+j*sqrt(uk²-ur²))/100` and the nominal no-load `Y`, it tests
coil `Z,Y` and coil `3Z,Y/3` separately, using the coil voltage ratio
`Un1/(sqrt(3)*Un2)`. Neither is declared the native rule. The native zero-sequence
measurements are not reproduced by these independent-coil hypotheses.

The check requires a fixed nominal common tap, matching W1/W2/W3 selection at
both ports, no neutral-point reference, and the corresponding historical
`Flag_Wind`/tap-side/position. It checks the available result phases/statuses,
opposite terminal identities, both secondary wire currents and stored current
magnitudes. Source-export hashes and unresolved type references remain in the
report. Missing or inconsistent inputs do not become fitted parameters.

Representative 01 contains 233 nominal single-coil YNd1 devices. **72** have
core watts materially larger than the apparent no-load power implied by
`i0*Sn/100`; for example, element 1982 states 30 W versus 27 VA. Those inputs
cannot define a passive complex no-load admittance by the stated equations,
so the checker excludes them. **85** other devices hit only a binary
rounding boundary. Comparing their source decimal measurements before SI
floating-point arithmetic avoids misclassifying them as inconsistent.

The remaining **161 devices / 7,889 states** have complete usable node,
branch and nominal coil-tap records. Both hypotheses fail every comparison:

| Hypothesis | Maximum wire-current residual | Maximum core-balance residual |
| --- | ---: | ---: |
| Single-coil rating (`Z,Y`) | 4.526987 A | 0.157249 A |
| Three-coil-bank rating (`3Z,Y/3`) | 10.486783 A | 0.098084 A |

Recovered current magnitudes agree within 1.599e-14 A, and the two secondary
wire currents sum to zero within 4.435e-11 A. The core-balance residual
`ratio*Ip+Is-Ycoil*(Vp/ratio+Vs)/2` is independent of leakage impedance.
Thus adjusting series Z alone cannot make these independent-coil pi circuits
match. This does not establish a replacement circuit, historical input
alignment or type resolution. The complete report remains external as
`csiro01-coil-experiments.json`. Partial-winding runtime mapping stays refused.

The measurement check also exposed a numeric boundary in the existing Rust
sequence decoder. Independently scaled nameplate quantities at exact
`P=|S|` can differ by floating-point roundoff. It now permits only a relative
excess of at most eight machine epsilons and returns zero quadrature there;
it retains the real input. There is no absolute tolerance, a positive real
value against zero still fails, material excess still fails, and valid small
reactive components remain. An original 10 kVA / 0.7% / 70 W nameplate test
checks this without copying a native fixture. This numeric correction does
not resolve the 72 materially inconsistent input sets.

## Additional CSIRO source-voltage alignment

Representative 01's infeeder 2 connects to node 318 in variant 1. Its native
input has `Flag_Lf=6`, `Ug=33` kV, `delta=0` and `xi=0`. Across all 49 stored
states, each of `U12`, `U23` and `U31` agrees with 33,000 V; the maximum absolute
difference is `7.64e-10` V. This supports the absolute line-line voltage basis
and distinguishes it from the unrelated `Node.Un=1` initial guess.

Do not turn that observation into an exact grounded-phase source assertion.
Phase-to-earth magnitudes differ from `33000/sqrt(3)` by up to 0.00394 V, and
the reported `Ue` reaches 0.0123 V. These historical values do not establish
the source's complete zero-sequence circuit or independently reproduce its
solution. The current decoder preserves grounding inputs for later resolution.

The audited ULFNodeResult CSV has SHA-256
`3c7786552061339e6d416f3ee590100653bd26f1a7dbebd73dd7a46d45c229a9`.

The subsequent reproducible `check_source_sequences.py` check joins all 49
source branch/node states. Injected phase currents recovered from `conj(S/V)`
match recorded magnitudes within `2.842170943040401e-14` A. The positive
voltage matches the specified source phasor within `4.003e-10` V; negative
voltage is below `2.672e-10` V. However, `V0 + Z0*I0` with the current direct
input `Z0=0.7109+j13.7575` ohm has magnitude 29.2533–33.0974 V. Its implied
`-V0/I0` ratio is not constant across states (real part 0.001699–0.001757 ohm;
imaginary part -0.000456–0.000154 ohm). Do not fit those ratios into the
parser or infer a native impedance formula from them. The small phase-voltage
displacement may involve numerical behavior; this check cannot establish its
cause or historical input alignment. It demonstrates why matching line-line
voltage alone is insufficient evidence for complete source conversion.

Additional CSV digests from the same external database:

- `Infeeder`: `7de6acfb4fac257b27185da775ca0ab0ebb5613eec30d8486e3099ca60b074be`.
- `CalcParameter`: `0421b39c2f7db4be91e9b7cccfab7f163c90d37fd1850ddf2f75a1680b5b4941`.

The selected settings are `Flag_LFZ0=1` and `Flag_ScType=1`, and the source
is an in-service L123 port with no active profiles or controls covered by the
check. The report remains external as `csiro01-source-sequence-check.json`.
The April 2014 source manual and the Siemens-authored
[October 2021 Power Flow manual, p. 45](https://www.scribd.com/document/704623348/Power-Flow)
do not resolve this dataset's source discrepancy. The latter still describes
constant-voltage sources using a limiting internal impedance and distinguishes
phase and sequence calculations. No copied manual is added to the repository.

Using the existing external table exports, reproduce the line-line check:

```python
import csv
from pathlib import Path

path = Path("/path/to/exports/ULFNodeResult.csv")
with path.open() as stream:
    states = [r for r in csv.DictReader(stream)
              if r["Node_ID"] == "318" and r["Variant_ID"] == "1"]
assert len(states) == 49
error = max(abs(float(r[field]) * 1000 - 33000)
            for r in states for field in ("U12", "U23", "U31"))
assert error < 1e-6  # volts, historical consistency only
print(error)
```

## Nominal transformer result audit

The external Representative 01 `TwoWindingTransformer` export contains 252
rows; SHA-256 of the inspected CSV is
`5e8f0b390aaa11ac942b920dd2783e8ec076ebca5e0ab4b8607857b7f3757310`.
The new `check_transformer_sequences.py` evidence harness compares current
nominal row values with historical node phasors and both terminal powers,
requiring matching result state and recorded common tap. It does not resolve
the local equipment-type references or claim a historical input snapshot.

All 11 full-winding ordinary Dyn11 units have 49 complete states and nominal
common taps. Results:

| Elements | States | Positive/negative sequence comparison |
| --- | --- | --- |
| 2060, 2104, 2105, 2219 | 196 | Maximum port-current error below 3.60e-12 A; their current no-load VA equals real core watts |
| 1979, 1981, 2101, 2102, 2220, 2221, 2223 | 343 | Maximum positive error 3.1817 A; maximum negative error 0.014067 A; current input rows include reactive no-load admittance |

Reconstructed current magnitudes agree with all stored magnitudes within
`1.14e-13` A. Negative-sequence currents reach 1.61 A, providing actual unequal
phase excitation. Zero-sequence currents remain below `1.62e-11` A, so these
observations cannot corroborate the zero-sequence primitive. Under the pi
hypothesis the apparent nominal core VAR stays below `1.35e-9` var across
all 539 states, including the seven units whose current input rows imply
nonzero reactive core consumption. Do not substitute that observed quantity
into the model: historical changes, equipment-type resolution and native
calculation behavior have not been separated.

This adds an explicit transformer-state discrepancy to the earlier source
discrepancy. Before using the feeder as a whole-network oracle, obtain or
reconstruct its exact historical input state, then verify core behavior with
controlled no-load and unequal-load native cases. Neither arbitrary taps nor
extra rotation was exercised by these ordinary full-winding cases. The
LoadFlow connector at the pinned revision also explicitly rejects nonzero
`roh` and `AddRotate` in
[its transformer reader](https://github.com/benediktibk/loadflow/blob/2d95494327123de7eaa9c6fe0bc09990038ee364/source/Interfaces/SincalConnector/TwoWindingTransformer.cs),
so it cannot supply that missing validation. All new native exports and
upstream source reads remain in temporary research storage; no fixture or
third-party implementation was added to the repository.

## Other inspected databases and pinned provenance

| Case | Repository revision | Database bytes | Schema | Nodes / elements / terminals | Findings |
| --- | --- | --- | --- | --- | --- |
| IEEE18 | `b1c44bc5b68f581c3f18da3a54138d6c9810a8ce` | 2,703,360 | 15.5 | 18 / 44 / 62 | All terminal codes 7; 18 balanced node results; no populated coupling, neutral-point or unbalanced result table |
| IEEE33 | same | 2,666,496 | 15.5 | 33 / 67 / 99 | All terminal codes 7; 33 balanced node results; no populated coupling, neutral-point or unbalanced result table |
| MATLAB LPC European LV | `7da511c81a89509f2bdc3ca1f0ab56da15fbb096` | 4,374,528 | 12.8 | 168 / 262 / 468 | 55 single-phase loads; terminal counts 1=21, 2=19, 3=15, 7=413; no stored load-flow results |
| MATLAB LPC S1a | same | 3,801,088 | 12.8 | 29 / 137 / 165 | Single-phase connections and 54 loads; no stored load-flow results |
| Student `e230438_con_phase2` | `985d38a49060593b461fb27e353fc34853b71f85` | 3,080,192 | 16.0 | 40 / 84 / 126 | All terminal codes 7; no populated coupling, neutral-point or unbalanced result table |

The IEEE18 and IEEE33 enclosing RAR files are respectively 3,082,571 and
1,923,421 bytes. They include executable files belonging to other tools;
none were executed. Selected database members were streamed to temporary
files rather than extracting the whole archives.

Database SHA-256 values:

- IEEE18: `9978078e4c956cf6d2e740bf2411fd5076111c17bfbc564925b3f205e797e8e5`.
- IEEE33: `3b392c49f28770c77aa73b0a9b6091afab8e43f2b9af8b37df919e09f60f8e12`.
- MATLAB LPC European LV: `7565a1ab584d44ba2305a40ba9925bd043231bcd05668d6b8a700a69bf1baa15`.
- MATLAB LPC S1a: `015a39ce65ab9626ad22dc4fdf41071b2350883ff94e61c88f10680746a7d1df`.

The IEEE18 archive also contains a 1,006-byte `Leika/config1.cpl` XML file.
It declares a `Coupl` document, a system with R/S/T conductors, and indexed
R/X/G/C entries. Its lower triangle contains zeros while its upper triangle
contains nonzero mutual entries. It has no associated `CouplingData` row in
this database, so it is an orphaned syntax example, not proof of an active
line model or a rule for matrix symmetry. Do not copy it into fixtures without
permission; independently establish units, triangular-storage rules, indexing,
and neutral representation first.

## Manuals and newly resolved questions

[LoadFlow revision](https://github.com/benediktibk/loadflow/tree/2d95494327123de7eaa9c6fe0bc09990038ee364)
contains Siemens-authored April 2014 manuals under `documents/external/sincal`.
They were read outside the repository, with relevant phase/grounding tables
rendered for inspection. The repository's source-code license does not license
these manuals for redistribution. Page numbers below distinguish printed
pages from PDF positions.

| Reference | Evidence now available | Remaining constraint |
| --- | --- | --- |
| `Datenbankbeschreibung.pdf`, printed pp. 6 and 63 / PDF pp. 12 and 69 | `Node.Un` is an initial voltage guess; `Node.Flag_Phase` selects fault phases; nominal line-line voltage comes from `VoltageLevel.Un` | Do not infer nominal ratings or bus conductor availability from the node guess/fault fields; newer voltage-control fields still require their own versioned definitions |
| `Datenbankbeschreibung.pdf`, printed p. 8 / PDF p. 14 | Terminal codes 1–8 mean L1, L2, L3, L12, L23, L31, L123, N; terminal switch and element service flags are separate | Verify against each supported schema; this is not a bitmask |
| Same, printed pp. 20–21 / PDF pp. 26–27 | Load model and input-format enums distinguish Z/PQ/I, phase wye powers and phase-pair delta powers | Read active fields and voltage bases; never interpret all populated columns simultaneously |
| Same, printed p. 45 / PDF p. 51 | Transformer vector-group enum and independent `AddRotate` | Encoding is documented; a correct conductor-domain transformer primitive is still required |
| Same, printed p. 75 / PDF p. 81 | Neutral-point IDs, sharing, switch state, RE/XE and RG/XG fields | Preserve shared-neutral topology; do not fold every value into an independent grounding shunt |
| `Eingabedaten.pdf`, printed pp. 154–157 / PDF pp. 164–167 | Missing phases, N-only return lines, direct return-conductor mode, temperature/frequency scaling, coupling sidecars replacing ordinary line data | Sequence equivalents and explicit return paths are different models; never count both twice |
| Same, printed pp. 261–266 / PDF pp. 271–276 | Leika/matrix-file reference, R/S/T/E conductor labels, neutral-point connection diagrams | Need a linked authentic conductor case and a verified sidecar format |
| `Lastfluss.pdf`, unbalanced sections | Additional zero-sequence data and neutral treatment required | Historical component-method settings are version dependent |

The file-format manual found in the same tree documents diagrams, network
state/graphics and result XML. It does not establish a complete, fresh native
network archive writer. XML is not yet an escape from the writer-schema gate.

## Reproduce and advance the evidence

For an already downloaded Access file, inspect without editing it:

```sh
mdb-tables -1 /path/to/database.mdb
mdb-export /path/to/database.mdb Version
mdb-export /path/to/database.mdb Terminal
mdb-export /path/to/database.mdb ULFNodeResult
```

Keep MDB Tools an optional research dependency. A CSV or SQLite transcription
of Access tables is derived research data, not an authentic SQLite SINCAL
project, and must not be passed off as a writer-acceptance fixture.

Next evidence work is to align one CSIRO historical state, inventory missing
model semantics, and build original small cases from documented fields. Keep
full datasets external, require explicit redistribution terms for uncertain
sources, and apply AGENTS.md's exact-file approval rule above 100 KiB. No
maintainers or dataset authors were contacted during this search.

## Targeted follow-up: LoadFlow native integration cases

Five databases from the pinned LoadFlow revision above were downloaded to
temporary research storage and inspected with MDB Tools 1.0.1. Their
[upstream integration tests](https://github.com/benediktibk/loadflow/blob/2d95494327123de7eaa9c6fe0bc09990038ee364/source/Interfaces/SincalConnectorIntegrationTest/PowerNetDatabaseAdapterTest.cs)
identify disconnected-line, parallel-system and transformer cases. Upstream
test assertions are useful discovery evidence; those tests were not executed
here. No upstream code or database was copied into this repository.

Each path below is under
`source/Interfaces/SincalConnectorIntegrationTest/testdata/` in that revision,
ending in `<case>_files/database.mdb`.

| Case | Bytes | Schema | Observed nodes / elements / terminals | Research value and limits |
| --- | --- | --- | --- | --- |
| `calculation_transformer6` | 3,190,784 | 11 | 2 / 3 / 4 | One transformer and two balanced node results |
| `calculation_transmissionline10` | 3,375,104 | 11.2 | 2 / 3 / 4 | One line with `ParSys=2`; possible parallel-count regression evidence |
| `calculation_transmissionline9` | 3,375,104 | 11.2 | 3 / 6 / 9 | Line 5 remains in service while both its terminals are open; this does not demonstrate one-ended charging |
| `unsupported` | 3,411,968 | 11 | 2 / 4 / 5 | An upstream negative-test candidate; not an unbalanced case |
| `dorfnetz` | 7,077,888 | 11 | Unreliable export | MDB Tools reports invalid row locations for Node, Element and LFNodeResult; quarantine these decoded tables |

The four warning-free exports have only terminal code 7 and empty
`NeutralPointImp`, `CouplingData` and `ULFNodeResult` tables. They do not close
the explicit-neutral/coupling evidence gap. The village-network export reports
345 code-7 terminals but cannot establish complete topology or result coverage:
its apparent zero node/element/result counts are decoder failures, not empty
native tables. The warnings occur 116, 230 and 80 times for those respective
tables. A second reliable reader would be required before using that case.

SHA-256, in the same case order:

- `38508bbc23a08450ab3c738d8273cde2b842b1e7eccb28eff3332f5431555e33`
- `30a0bac6db0d62000497ec9577cf5cfd879179f929ffa988b89dc6af036f7079`
- `c28eaacaf9d96b66c88dc6615bf801b44d44cfee5fc729c9f7243f8f68f9c5bd`
- `53155c9b2f624466badcd9182d5382fbe564505bfa3991709153dd1c7072fe8b`
- `bdabf5fb722ab714698a580b7cf16fd328aa064be4c1747b97f3b8836d7eeaea`

Keep all five external. The repository's GPL statement covers its applications
and libraries; it has not established redistribution provenance for each native
model. All five also exceed PowerIO's per-file fixture limit. These candidates
can guide small original synthetic tests for open ports and parallel scaling,
but cannot be promoted to repository fixtures or unbalanced reference cases
on the strength of the upstream test names alone.


## Voltage-basis follow-up

A targeted source-impedance search also identified a mapping omission in the
Siemens *Database Interface and Automation* manual (April 2015), printed p. 90:
[`Flag_Volt` selects line-line or line-earth voltage](https://manualzz.com/doc/2231110/pss-sincal-database-interface-and-automation).
The older April 2014 table lacks that selector. Inspection of the existing
licensed SimBench archive confirms that referenced levels use 1, with NULL
on an unused level. The private reader now gates referenced node and equipment
levels to explicit mode 1. No manual pages or new native files were vendored.
Finite source zero-sequence load-flow behavior still lacks a reconciled oracle.


## Ohmic shunt follow-up

The April 2014 Input Data manual, printed pp. 121–122, defines the direct
R+jX shunt and its phase-earth, phase-pair and star connections. Database
Description pp. 30–31 distinguishes constant impedance from controlled-current
operation. This supports a small original circuit test suite with unequal
phase voltages and explicit floating/grounded star points.

The already licensed schema-14.8 SimBench archive has additional
`ShuntImpedance.Flag_Lf`, `Typ_ID` and `Flag_Typ_ID` columns. The inspected
older manuals do not establish the new input selector's values or NULL
default. The private mapper rejects any layout containing `Flag_Lf`, rather
than interpreting its absence from the old manual as permission to ignore it.
Type references are also guarded. No new native shunt case or newer selector
documentation was found in the targeted search. Native shunt validation and
capacitor/reactor profiles remain outstanding; the six new tests establish
circuit equations and PMD transport only. No manual or external data was added
to repository fixtures.

## Publication-focused search, 2026-10-07

This follow-up specifically sought native unbalanced models published with
papers, theses and research datasets. No new native model was downloaded or
added to fixtures in this pass. Public metadata and repository inventories
were saved in `/private/tmp/powerio-sincal-research/search-2026-10-07/`.
The absence of a file in the inspected release is not proof that the authors
have no shareable model.

Searches included SINCAL with `unbalanced`, `neutral`, `four wire`, `IEEE 13`,
`test feeder`, `supplementary material`, `data availability`, `.mdb`, `.sinx`
and `.zip`; targeted Zenodo, Figshare, Mendeley Data, IEEE DataPort, BetterGrids,
GitLab and university repository queries; GitHub repository/code queries;
and the DataCite public DOI catalog. GitHub text search is noisy and binary
files are poorly indexed: an extension search returned only a placeholder
`.SINX` in a file-extension demonstration repository. Recursive trees,
rather than code-search totals, were used to inspect candidate repositories.

The DataCite API query `sincal` returned 28 records, including unrelated
personal-name matches and theses. Its only record typed `Dataset` was the
already known 2015 CSIRO collection. This is a bounded metadata-search result,
not a census of all scientific data. The CQU Figshare article API for record
13438031 listed one thesis PDF and no model supplement.

### New leads and exclusions

| Publication or project | What was verified | Disposition for native end-to-end validation |
| --- | --- | --- |
| [CSIRO National Low-Voltage Feeder Taxonomy / RepresentativeLVNetworks](https://github.com/csiro-energy-systems/RepresentativeLVNetworks) | The official README identifies 23 representative networks released in OpenDSS. Recursive tree at `b715e88a9bf0c04892970a7adf6129b4d775adfc` has 193 files and no `.sin`, `.sinx`, `.mdb`, `.db` or enclosing ZIP/RAR/7z model archive. | Useful independent distribution examples, but no native SINCAL input in this release. Do not conflate these with the 2015 SINCAL collection. |
| [ESTCP EW19-5054, Comprehensive Microgrid Energy Storage Designs with Guaranteed Optimality](https://serdp-estcp.mil/projects/details/9e433e35-8dba-4d6b-a294-cd81e3dc1d62/ew19-5054-project-overview) | The project reports SINCAL validation; its final report includes an IEEE 123-bus example. The public products list contains a final report and executive summary, not model files. | Concrete acquisition lead for a benchmark export and matching results. No downloadable native model or model redistribution license established. Public release of the report does not license an unpublished database. |
| [University of Melbourne / AusNet, HV-LV Modelling of Selected HV Feeders](https://arena.gov.au/knowledge-bank/advanced-planning-of-pv-rich-distribution-networks-deliverable-1-hv-lv-modelling-of-selected-hv-feeders/) | The report describes extracting utility SINCAL MDB data and building detailed OpenDSS networks. The inspected ARENA deliverable page publishes the report. | Potential paired native/OpenDSS validation source if the owners release the inputs. No public native download or redistribution permission established. |
| [A Practical Approach to Optimising Distribution Transformer Tap Settings](https://doi.org/10.3390/en13184889) | The paper's supplementary-material statement makes substation data availability subject to TasNetworks approval. | Not an openly licensed fixture source. No author or utility was contacted. |
| [TU Wien network-management thesis (2022)](https://doi.org/10.34726/hss.2022.29907), [storage integration thesis (2023)](https://doi.org/10.34726/hss.2023.112254), [transformer ageing thesis (2024)](https://doi.org/10.34726/hss.2024.107062) | Repository pages describe SINCAL studies and list PDF fulltexts with an In Copyright statement; no native supplement appears in the inspected attachment lists. | Author-acquisition leads only; phase/neutral coverage and native data rights are unverified. |
| [CQU, Experimental investigation and assessment of renewable energy integration into the grid](https://doi.org/10.25946/13438031) | Figshare API lists only `Thesis_Shafiullah_GM_Redacted.pdf` (12,265,256 bytes), under CQUniversity Thesis 1.0. | A thesis reference, not a released native model corpus. |
| Tercan et al., *Daily Energy Use Planning With Distributed Energy Storage Systems*, IMSS 2019 | The author's [paper text](https://www.researchgate.net/publication/348310586_Daily_Energy_Use_Planning_With_Distributed_Energy_Storage_Systems), section 2.1, explicitly describes converting single/two-phase lines and loads into balanced three-phase equivalents and removing the regulator. A [university-hosted proceedings link](https://ksu.lt/wp-content/uploads/2020/12/2019-1-2_compressed.pdf) was located, but direct retrieval failed. | Exclude as evidence for native unbalanced IEEE 13-bus coverage. Benchmark names do not establish the actual simulated profile. |

Additional recursive GitHub tree checks found no candidate native database or
model archive in `mmj81/SINCAL`, `ivenguzel/EE474_PSS_Sincal_Simulation`,
`Mohamedkrs/CIM_Data_Manager_sincal_powerfactory`, `KIR007-glitch/PSS-Sincal`,
`joseph9916/PSS_Sincal_Automation`,
`ivanovdrenergorazvitie/scripts_for_sincal`, `bodems/bachelorarbeit`,
`ZhigaMason/euroteq-pre-distribution-2026`, `ZGEnergy/grc-tech-evaluation`,
or `energychain/cernion-energy-tools`. These contain reports, converters,
scripts or other-format data; mentioning SINCAL is insufficient evidence.
All inspected recursive tree responses reported `truncated=false`.

The paper-linked [MilosFTN IEEE18/33 repository](https://github.com/MilosFTN/Models-of-the-IEEE-18--and-33-bus-test-systems)
now cites DOI `10.1109/IcETRAN66854.2025.11114093`. This improves bibliographic
provenance, but does not change the previous database inspection or license
assessment: the inspected cases have balanced results, and citation
instructions do not supply a redistribution license.

### Consequences for the validation roadmap

1. **Prioritize CSIRO Representative 06 for a complete external unbalanced
   comparison**, with Representative 01 next for profiles and mixed winding
   connections. Both are existing CC BY 4.0 native evidence. Keep the Access
   transport explicit: a research table transcription is not a native SQLite
   export and cannot certify the runtime acquisition path. Record component
   coverage and unresolved source/transformer discrepancies before declaring
   either a golden whole-network case.
2. **Retain the MATLAB LPC European LV and S1a cases as secondary research
   candidates.** The previously inspected native MDBs contain mixed-phase
   connections and no stored power-flow results. The repository root MIT
   license remains present at the same pinned revision; inherited benchmark
   rights remain unresolved. A paired OpenDSS benchmark would require an
   explicit topology/parameter/profile correspondence check, not just a
   shared IEEE name. Do not vendor these databases yet.
3. **Keep explicit-neutral and active coupling evidence as a separate gap.**
   None of the additional releases inspected here supplies a confirmed linked
   native case with those inputs and matching results. The orphan coupling
   sidecar already cataloged does not close this gap.
4. **Define an acquisition packet for future author/vendor cooperation:**
   an explicit data license; original native project plus required type and
   coupling sidecars; product/schema versions; selected variant and snapshot;
   solver settings, tap/control states and profiles; per-phase complex bus
   voltages and terminal currents/powers; and preferably an independently
   constructed OpenDSS or PowerFactory counterpart. No messages were sent.

This search changes corpus priorities and records concrete acquisition leads;
it adds no parser validation result and does not satisfy native writer gate E2.

The subsequent [prioritized verification run](README.md#prioritized-authentic-unbalanced-verification-2026-10-07)
uses fresh CSIRO 06 and 01 table exports, with source hashes checked against
the inventory. Its [derived packet](unbalanced-verification.json) records
per-phase static-load comparisons, existing line/profile/source reruns, and
stored port-current balance with explicit coverage exclusions. It exposes
a phase-power discrepancy hidden by matching load totals and retains the
source/transformer gaps. The MATLAB LPC candidates were rechecked for stored
results and still have none. No model payload was added to the fixture corpus.
