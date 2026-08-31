# PowerIO 1.0 issue audit

`PioModule<T>` was the accepted universal top level parse and compiler type.
The audit marked issue titles and bodies that still proposed `NetworkPackage`,
`PioRecord`, or a separate parsed wrapper for revision.

Status: historical tracker snapshot from 2026-08-25. It records the scope used
for the 0.10 implementation and is not current API or issue authority.

The audit covered every issue then assigned to the `v1.0.0` milestone, issues
later added to the 1.0 scope, and closed 1.0 issues whose shipped API needed
another breaking change.

## Then current 1.0 milestone

| Issue | 1.0 decision | Required correction |
|---|---|---|
| [#400](https://github.com/eigenergy/powerio/issues/400) | keep | Expose the accepted incidence matrix, branch susceptance, phase shift injection, and stable branch mapping API. Use PowerModels signs at the public boundary. |
| [#407](https://github.com/eigenergy/powerio/issues/407) | keep | Add one sparse physical AC power flow Jacobian operation with polar and Cartesian coordinates, explicit mappings, and structure reuse. |
| [#399](https://github.com/eigenergy/powerio/issues/399) | keep, rewrite | Do not expose solver preparation tables. Matrix and instance results must carry stable element mappings through Rust, C, Python, and Julia without a second extraction. |
| [#398](https://github.com/eigenergy/powerio/issues/398) | keep | Expose the explicit `MulticonductorNetwork` to `BalancedNetwork` transformation through MCP. Use concrete transformation names and never apply it implicitly. |
| [#397](https://github.com/eigenergy/powerio/issues/397) | keep, rewrite | Implement efficient typed state selection and explicit static module export. The solver path must not copy a `PioModule` through generic JSON. |
| [#383](https://github.com/eigenergy/powerio/issues/383) | keep | Complete typed regulator round trips. Any BMOPF extension emitted by PowerIO must parse again without becoming an untyped object. |
| [#377](https://github.com/eigenergy/powerio/issues/377) | keep | Aggregate repeated findings and include element paths. This is part of the `Diagnostic` cleanup. |
| [#376](https://github.com/eigenergy/powerio/issues/376) | keep | Finish the PMD capacitor and DSS line rating conversions. |
| [#375](https://github.com/eigenergy/powerio/issues/375) | keep | Return the version diagnosis without embedding a consumer specific remedy. |
| [#360](https://github.com/eigenergy/powerio/issues/360) | keep | Preserve piecewise generator cost data and report any unavoidable constant objective offset. |
| [#339](https://github.com/eigenergy/powerio/issues/339) | keep | Exercise OpenDSS include containment, nesting, and byte budgets in fuzzing. |
| [#338](https://github.com/eigenergy/powerio/issues/338) | keep | Replace or bound the PowerWorld binary table search and restore useful fuzz throughput. |
| [#325](https://github.com/eigenergy/powerio/issues/325) | keep | Install and execute every release wheel before publication. |
| [#324](https://github.com/eigenergy/powerio/issues/324) | keep, rewrite | Apply numerical guards only to the quantity divided by the selected DC formula. Revisit the pivot test using local scale. |
| [#307](https://github.com/eigenergy/powerio/issues/307) | keep | Permute conductor maps and matrices together so DSS writing preserves terminal identity. |
| [#291](https://github.com/eigenergy/powerio/issues/291) | keep | Factor once, reuse scratch buffers, and remove per iteration allocation from sensitivity construction. |
| [#274](https://github.com/eigenergy/powerio/issues/274) | keep | Bound retained PowerWorld display identities independently of the input size and state the parse limit. |
| [#261](https://github.com/eigenergy/powerio/issues/261) | keep, rewrite | Never create an epsilon impedance. Merge only an unrated identity switch after checking controls and bounds, return mappings and diagnostics, and refuse every unsupported case. |
| [#196](https://github.com/eigenergy/powerio/issues/196) | keep, expand | Implement ordinary generic collections with private shared numerical data and small owning `OperatingPoint<N>` handles. Replace string keyed update maps, resolve identities once, and feed matrix and instance consumers without cloning or materializing networks. Do not expose traits for memory representation. |

## Promoted into 1.0

These issues were moved to the 1.0 milestone during this audit. Their bodies
now contain the exact 1.0 completion requirements.

| Issue | Reason |
|---|---|
| [#232](https://github.com/eigenergy/powerio/issues/232) | Direct multiconductor passive and augmented admittance data is an accepted 1.0 requirement. BMOPFTools supplies the compatibility tests. |
| [#293](https://github.com/eigenergy/powerio/issues/293) | The fixes include breaking representations for extras and conductor matrices. The 1.0 window is the time to remove the documented peak memory and allocation costs. |
| [#294](https://github.com/eigenergy/powerio/issues/294) | Dense PTDF and LODF construction retains several quadratic buffers and uses cache hostile assembly. Allocation review was a 1.0 release requirement. |

## Remain after 1.0

| Issue | Reason |
|---|---|
| [#14](https://github.com/eigenergy/powerio/issues/14) | Full scenario batch operator reuse can follow 1.0. The 1.0 typed operating state design must make it possible without another API break. |
| [#111](https://github.com/eigenergy/powerio/issues/111) | Dynamic model data remains a 1.1 design target. PowerIO 1.0 must not put dynamic records into either steady state network type. |

## Closed issues requiring new 1.0 work

- [#235](https://github.com/eigenergy/powerio/issues/235) shipped
  `ScopfInstance`. The public type now needs to become `AcScucInstance`, with
  DOE GO Challenge 3 JSON parsed as the source for that instance.
- [#173](https://github.com/eigenergy/powerio/issues/173) stabilized network
  values and generic operating point fields. The 1.0 `PioModule.value` enum
  now needs to include problem instances, and the runtime update path needs
  typed updates.
- [#236](https://github.com/eigenergy/powerio/issues/236) exposed construction
  of the current operating point wire data. The binding API must move to the
  typed update representation without losing bulk construction.
- [#49](https://github.com/eigenergy/powerio/issues/49) prepared the then public
  structures. This architecture removed several of them and added explicit
  problem instance types, so constructor coverage required another audit.
- [#194](https://github.com/eigenergy/powerio/issues/194) correctly separated
  PowerIO network JSON from `.pio.json`. PowerIO network JSON remains a structured
  network transport; `PioModule` gains one typed value that can also be a
  problem instance.

## Created tracker issues

| Issue | Scope |
|---|---|
| [#408](https://github.com/eigenergy/powerio/issues/408) | `powerio-core`, `powerio-tx`, the entry facade, and retirement of the two 0.x packages. |
| [#409](https://github.com/eigenergy/powerio/issues/409) | `PioModule<T>`, the 20 dynamic values, and consuming typed narrowing. |
| [#410](https://github.com/eigenergy/powerio/issues/410) | Stored module schema version 1 and explicit 0.9 migration. |
| [#411](https://github.com/eigenergy/powerio/issues/411) | Generic time and scenario containers, shared data, and borrowed access. |
| [#412](https://github.com/eigenergy/powerio/issues/412) | Operating points, seven calculation instances, seven solutions, and the `AcScuc` naming correction. |
| [#413](https://github.com/eigenergy/powerio/issues/413) | ABI v6 ownership, errors, arrays, allocator, and concurrency rules. |
| [#414](https://github.com/eigenergy/powerio/issues/414) | Exact BMOPF 0.1.0 electrical and calculation mappings. |
| [#415](https://github.com/eigenergy/powerio/issues/415) | File, directory, and memory destinations and writing ownership. |
| [#416](https://github.com/eigenergy/powerio/issues/416) | Egret `system.time_keys` as `TimeSeries<BalancedNetwork>`. |
| [#417](https://github.com/eigenergy/powerio/issues/417) | Source format profiles, opaque sources, and stable format identifiers. |
| [#418](https://github.com/eigenergy/powerio/issues/418) | The exact PyPSA CSV electrical profile, including supported snapshot-local series. |
| [#419](https://github.com/eigenergy/powerio/issues/419) | OpenDSS static circuits and complete sampled QSTS states. |
| [#420](https://github.com/eigenergy/powerio/issues/420) | DOE GO Challenge 3, DeepMind OPFData, and GridFM typed values. |
| [#421](https://github.com/eigenergy/powerio/issues/421) | Parser allocations, peak memory, wall time, and bounded malformed input. |
| [#422](https://github.com/eigenergy/powerio/issues/422) | Typed PowerMCP routing and state selection. |
| [#423](https://github.com/eigenergy/powerio/issues/423) | The independent format and calculation evaluation workspace. |
| [PowerIO.jl #120](https://github.com/eigenergy/PowerIO.jl/issues/120) | ABI v6 handles, originating library ownership, read-only native views, and typed modules. |

The final DC equations stay in PowerIO [#400](https://github.com/eigenergy/powerio/issues/400)
and PowerIO.jl [#114](https://github.com/eigenergy/PowerIO.jl/issues/114).
Both issue bodies now include
`p_branch = -Bf * va + b .* shift`. The four preexisting PowerIO.jl 1.0 issues
[#111](https://github.com/eigenergy/PowerIO.jl/issues/111),
[#112](https://github.com/eigenergy/PowerIO.jl/issues/112),
[#113](https://github.com/eigenergy/PowerIO.jl/issues/113), and
[#114](https://github.com/eigenergy/PowerIO.jl/issues/114) were also reconciled
with the Rust design.

## Verified implementation gaps at the time

The discarded instance draft contained these useful audit findings. They stay
here as implementation evidence, not proposed public API:

- `powerio-prob` has no solution types and its current OPF instances expose
  parallel solver preparation vectors.
- `NetworkPackage` can store balanced or multiconductor network data plus its
  current operating point updates, but not the accepted instance, solution,
  `TimeSeries<T>`, or `ScenarioSet<T>` values.
- Tellegen copies PowerIO instance columns into `DcNetwork` and `AcNetwork` and
  fits unsupported generator costs before solving.
- PowerIO has no direct multiconductor admittance implementation;
  BMOPFTools contains the independent reference implementation.
- The Egret parser drops supported generator capability and ramp fields that
  need typed coverage or diagnostics.
- `ElementRef.row` is already optional and `source_uid` exists, but table and
  row remain permitted as durable identity. Replace that semantic reference
  with module scoped `ElementId { kind, id }`; keep source rows in source
  maps and private caches.

## Recorded PR disposition

The existing stacks are evidence and salvage branches, not the 1.0 dependency
graph. On 2026-08-25 every open PR description in both repositories received
the matching disposition below so its GitHub page no longer implies that an
obsolete public API should merge.

| PR | Disposition |
|---|---|
| PowerIO #387 | Retain sparse factorization work. Do not ship it in 0.9.1 as written because the public `Iterative` to `Sparse` rename is not a patch change. |
| PowerIO #401 | Retain sparse automatic routing, dense fallback, and checked CSC handling on top of the factorization work. Fold in sensitivity allocation fixes from #405. |
| PowerIO #402 | Do not merge. Reuse mapping logic privately; do not expose solver rows or indexes. |
| PowerIO #403 | Superseded. Reuse fill guards, mapping code, and bulk extraction tests in ABI v6 only. |
| PowerIO #404 | Port preflight, apply, transformation, diagnostics, option validation, and stdio behavior to the later `PioModule` MCP surface. Replace generic lower terminology. |
| PowerIO #405 | Salvage PowerModels signs, token unification, allocation reductions, checked conversions, and MCP readiness fixes. Do not merge the branch as a unit. |
| PowerIO.jl #115 | Do not merge the public solver source row API. |
| PowerIO.jl #116 | Rebuild independently from `main`, adding the raw path finiteness fixes and tests from #118. |
| PowerIO.jl #117 | Superseded by the accepted physical PowerModels incidence semantics. |
| PowerIO.jl #118 | Salvage C ownership, finiteness, allocation, bulk extraction, and corrected sign work. Do not retain `DcPowerFlowData`. |

PowerIO #405 and PowerIO.jl #118 do not form a passing cross repository tip.
Both also omit the affine branch shift term in documentation or tests. The
required relation and regression are:

```text
p_branch = -Bf * va + b .* shift
```

### 0.9.1 maintenance release

A reduced maintenance release can run before the 1.0 restructure. PowerIO.jl
can take one patch rooted at `main` containing #116 plus the relevant #118
finiteness, allocation, and ownership corrections. PowerIO #387 and #401 are
not patch compatible. Cut PowerIO 0.9.1 only for an actual backward compatible
Rust fix. The PowerIO.jl release intent requires its version to match the Rust
tag and fixes the reviewed Julia source digest and changelog before artifact
updating begins. The updater may change only `Artifacts.toml`, so there is no
Julia only 0.9.1 path under the current release process. If no Rust correction
warrants 0.9.1, defer both. No current Rust stack PR merges before that tag.
