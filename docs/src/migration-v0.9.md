# Migrating to 0.9

0.9.0 is the API that 1.0.0 ships, so it takes the breaks that were being deferred. A C or Julia consumer also needs [the ABI 5 guide](abi-v5.md); the two sets barely overlap.

Most of what follows is a compile error. Two items are not, and they are the ones to check by hand: the DC susceptance default and `.pio.json` documents written before 0.9.

## The deprecated names remain for one release

The 0.8 compatibility names were scheduled for deletion at 1.0, but several
were not reachable where the deprecation notes promised. 0.9.0 repairs that
bridge. Each old name resolves with a warning that names its successor and its
1.0.0 removal.

```rust,ignore
// 0.8
use powerio::Network;
use powerio_dist::DistNetwork;
use powerio_prob::build_scopf_instance_from_str;
branch.legacy_total_charging_b()

// 0.9, preferred names
use powerio::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;
use powerio_prob::parse_scopf_str;
branch.total_charging_b()
```

`DcConvention::PaperPure` also remains as a deprecated associated constant for
`DcConvention::ReactanceOnly`. `scripts/deprecated-inventory.sh` lists the
bridge and its `--assert-empty` mode is the 1.0.0 removal gate.

Python follows the same rule: `powerio.Network` and
`powerio.dist.DistNetwork` resolve with `DeprecationWarning`. Prefer
`powerio.BalancedNetwork` and `powerio.dist.MulticonductorNetwork` now so the
1.0.0 removal is a no-op for your code.

The retired `powerio-json` case format token is not part of this bridge. Model
JSON moves through `to_json` and `from_json`, and classification reports
`model-json`.

## The 0.9 DC solver weight default changed value

This one is silent. A caller that passed no convention gets different numbers.

This section records the 0.8 to 0.9 behavior, when
`DcConvention::branch_susceptance` returned the positive solver edge weight
`w`. In 0.10 the public `BranchSusceptanceFormula::branch_susceptance` follows
PowerModels and returns `b = -w`; `solver_edge_weight` returns the positive
factor described below.

`DcConvention` offered `w = 1/x` and MATPOWER's `w = 1/(x·τ)`, and neither reads the branch resistance, so a case with a real r/x ratio had no convention describing it and every consumer computed one by hand. The 0.9 default became `SeriesImpedance`, `w = x/(r² + x²)`, with phase shift injections and no tap scaling.

| 0.8 | 0.9 | formula |
|---|---|---|
| `PaperPure` (default) | `ReactanceOnly` | `w = 1/x` |
| `Matpower` | `Matpower` | `w = 1/(x·τ)` |
| — | `SeriesImpedance` (default) | `w = x/(r² + x²)` |

The gap grows with r/x: small on transmission cases, large on distribution ones. At `r = x = 0.1` the old default returned 10 and the new one returns 5. A resistanceless branch is unaffected, because `x/(r² + x²)` reduces to `1/x` exactly.

`ReactanceOnly` is not deprecated. The positive factor `w = 1/x` is the
textbook DC linearization; the current public coefficient is `b = -1/x`.

The method takes the resistance now:

```rust,ignore
conv.branch_susceptance(x, tap)       // 0.8
conv.branch_susceptance(r, x, tap)    // 0.9
```

The 0.9 method returned a **positive** Laplacian edge weight in every variant.
Current code names that value `solver_edge_weight`; public matrices use the
negative PowerModels coefficient from `branch_susceptance`.

It also returns `NaN` for a denominator that is not finite, which `SeriesImpedance` already did. `1/±inf` is `0.0`, so `ReactanceOnly` and `Matpower` used to read an infinite reactance as a zero weight edge and drop the branch from the Laplacian without saying so; `Matpower` divides by `x * tap`, so two finite factors whose product overflows read the same way. The matrix and instance builders check the result and raise `NonFiniteSusceptance`. Every finite denominator is unchanged.

On the other surfaces:

```sh
powerio matrices --convention series-susceptance   # was --convention paper-pure
```

```python
net.calc_ptdf()                        # convention="series" by default
net.calc_ptdf(convention="reactance-only")
net.calc_ptdf(convention="paper")      # ValueError, naming the successor
```

The old spellings raise rather than resolving, because the nearest-looking option is a different formula and a caller who guesses would get numbers instead of an error.

## `ReferenceBuses` replaces `Vec<usize>`

A consumer that grounded one slack bus wrote `.first()`, which grounds one island of a network of several and leaves the rest singular. The new type has no `first()`.

```rust,ignore
let slack = instance.reference_buses[0];        // 0.8
for bus in instance.reference_buses.iter() { }  // 0.9, several islands
let slack = instance.reference_buses.single()?; // 0.9, errors unless exactly one
```

The serde form is unchanged: it still reads and writes a bare array.

## `nodal_generator_data` no longer fails

It aggregates several generators at one bus instead of refusing, combining cost curves by the parallel rule `q = 1/Σ(1/qᵢ)`. Drop the `?`; `Error::MultipleGeneratorsAtBus` is gone.

```rust,ignore
let nodal = instance.nodal_generator_data()?;   // 0.8
let nodal = instance.nodal_generator_data();    // 0.9
```

`build_dc_opf_instance` also reads bus shunt conductance now, which it never did although the AC builder always had, and carries it as its own vector beside `p_d` rather than folded into it.

## Each crate raises its own errors

`powerio_matrix::Error` and `powerio_prob::Error` were literally `powerio::Error`. They are distinct types now, carrying what each crate raises, and reaching the hub's variants through a wrapper.

```rust,ignore
matches!(e, powerio_prob::Error::UnknownBus { .. })                    // 0.8
matches!(e, powerio_prob::Error::Core(powerio::Error::UnknownBus { .. })) // 0.9
```

Each wraps the layer below with `#[error(transparent)]`, so a hub failure crossing the boundary keeps its `Display` text byte for byte. The C ABI reports errors as text and nothing else, so a wrapper that restated the message would change what every binding prints.

The `.pio.json` document layer (`powerio::package`, `powerio-pkg` in 0.9) gets a real error type. `from_json` returned one opaque `serde_json::Error` for malformed JSON, an unreadable lineage, and a `model_kind` contradicting its payload; those are `Malformed`, `UnsupportedVersion` and `ModelKindMismatch`.

In Python this fixes a hole rather than breaking anything: `package_pyerr` mapped every `.pio.json` failure to a bare `ValueError`, so `except powerio.PowerIOError` did not catch a package failure although it caught every other parse failure. It does now.

## Regenerate stored `.pio.json` documents

Every document powerio authors states `powerio_version`, the release that wrote it. `.pio.json`, the SCOPF document, the DC OPF bundle manifest, the geo layer sidecar, the Arrow catalog and table metadata, and every summary document changed their version field.

A document from 0.8.x or earlier states `schema_version` and no `powerio_version` at all. It deserializes to the empty string rather than defaulting to the current version, so the gate stays closed and the reader names the release that wrote it. That is deliberate: a document written under a schema the reader no longer implements should fail loudly, not load as though it were current.

Regenerate them. There is no upgrade path for a stored document.

## Python `to_dense()` reports one bus space

`to_dense()` returned case-mirror tables beside a lowered-view `reference_bus`, `n_components` and `is_radial`, and a `bprime()` whose shape did not match the tables next to it. On a case with an in-service 3-winding transformer they disagreed. `_BalancedNetwork.lowered()` is new and `to_dense` builds every field from it, so the whole result is the lowered space.

If you were joining `to_dense().buses` against a case file table by position, that join now needs the raw table instead.

## `Branch::synthesize_rate_a` takes voltage bands

```rust,ignore
br.synthesize_rate_a(window, fr_vmax, to_vmax)                  // 0.8
br.synthesize_rate_a(window, (fr_vmin, fr_vmax), (to_vmin, to_vmax))  // 0.9
```

The phasor difference is convex in the two voltages, so its largest value over the band box sits at a corner. Below roughly 10° that corner is one terminal at its ceiling and the other at its floor. Reading only the ceilings returned a bound several times tighter than the branch physically has, and an OPF enforces it.

## Bus ids have a ceiling

`BusId::MAX` is `i64::MAX`, and `validate` refuses a network carrying an id above it, with the headroom a 3-winding star expansion needs reserved under the same ceiling. The readers parse an id through `as usize`, which saturates rather than failing, so two distinct out of range ids in a source file used to land on the same value and both surface as `-1` from `pio_bus_ids` — a branch endpoint then matched two bus rows. Real cases sit far below the ceiling; PSS/E stops at 999997.
