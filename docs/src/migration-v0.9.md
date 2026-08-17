# Migrating to 0.9

0.9.0 is the API that 1.0.0 ships, so it takes the breaks that were being deferred. A C or Julia consumer also needs [the ABI 5 guide](abi-v5.md); the two sets barely overlap.

Most of what follows is a compile error. Three items are not, and they are the ones to check by hand: the DC susceptance default, `.pio.json` documents written before 0.9, and the Python name lookups.

## The deprecated names are gone

They were scheduled for deletion at 1.0 anyway, and `powerio/src/lib.rs` re-exported only `BalancedNetwork`, so the aliases were reachable at `powerio::network::Network` and nowhere else. Every existing `use powerio::Network;` got a compile error rather than the promised warning, so 0.9.0 takes the break instead of carrying it.

```rust,ignore
// 0.8
use powerio::Network;
use powerio_dist::DistNetwork;
use powerio_prob::build_scopf_instance_from_str;
branch.legacy_total_charging_b()

// 0.9
use powerio::BalancedNetwork;
use powerio_dist::MulticonductorNetwork;
use powerio_prob::parse_scopf_str;
branch.total_charging_b()
```

`Network` named one of two models by the word for both, and `DistNetwork` named a crate rather than a model. Nothing about `total_charging_b` is legacy; it is the projection every MATPOWER shaped writer needs.

**Python has no shim.** `powerio.Network` and `powerio.dist.DistNetwork` raise `AttributeError` from the module's ordinary attribute lookup. Use `powerio.BalancedNetwork` and `powerio.dist.MulticonductorNetwork`.

## The DC susceptance default changed value

This one is silent. A caller that passed no convention gets different numbers.

`DcConvention` offered `b = 1/x` and MATPOWER's `b = 1/(x·τ)`, and neither reads the branch resistance, so a case with a real r/x ratio had no convention describing it and every consumer computed one by hand. The new default is `SeriesImpedance`, `b = x/(r² + x²)`, with phase shift injections and no tap scaling.

| 0.8 | 0.9 | formula |
|---|---|---|
| `PaperPure` (default) | `ReactanceOnly` | `1/x` |
| `Matpower` | `Matpower` | `1/(x·τ)` |
| — | `SeriesImpedance` (default) | `x/(r² + x²)` |

The gap grows with r/x: small on transmission cases, large on distribution ones. At `r = x = 0.1` the old default returned 10 and the new one returns 5. A resistanceless branch is unaffected, because `x/(r² + x²)` reduces to `1/x` exactly.

`ReactanceOnly` is not deprecated. `b = 1/x` is the textbook DC linearization, so reproducing a published result needs it exactly as written.

The method takes the resistance now:

```rust,ignore
conv.branch_susceptance(x, tap)       // 0.8
conv.branch_susceptance(r, x, tap)    // 0.9
```

It returns a **positive** Laplacian edge weight, in every variant. PowerModels and tellegen write the negative one; that is their convention, and a caller that negates once cannot get a sign flipped matrix from the choice of variant.

It also returns `NaN` for a denominator that is not finite, which `SeriesImpedance` already did. `1/±inf` is `0.0`, so `ReactanceOnly` and `Matpower` used to read an infinite reactance as a zero weight edge and drop the branch from the Laplacian without saying so; `Matpower` divides by `x * tap`, so two finite factors whose product overflows read the same way. The matrix and instance builders check the result and raise `NonFiniteSusceptance`. Every finite denominator is unchanged.

On the other surfaces:

```sh
powerio matrices --convention series          # was --convention paper-pure
```

```python
net.ptdf()                        # convention="series" by default
net.ptdf(convention="reactance-only")
net.ptdf(convention="paper")      # ValueError, naming the successor
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

`powerio-pkg` gets a real error type. `from_json` returned one opaque `serde_json::Error` for malformed JSON, an unreadable lineage, and a `model_kind` contradicting its payload; those are `Malformed`, `UnsupportedVersion` and `ModelKindMismatch`.

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
