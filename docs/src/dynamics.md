# Dynamics and DYR

## Representation decision

PSS/E `.dyr` is a dynamic-model file, not a power-flow network. PowerIO keeps
that distinction explicit: `.raw` is parsed by `powerio::parse` to a
`PioValue::BalancedNetwork`, while `.dyr` is parsed by `powerio::parse_dyr`
to a standalone `DynamicCase`. Pair the two with `DynamicStudy`; do not insert
dynamic records into `BalancedNetwork` or treat the raw model key as a typed
generator reference.

This separate Rust API is intentional. It establishes a stable boundary for
loss-preserving DYR records and generic bus association without prematurely
committing the cross-language `PioValue`, PowerIO IR, C ABI, Julia, or Python
representations to an incomplete dynamic-model schema.

## Parse and pair

The example below is compiled as the `dynamic_study` Cargo example. Run it in
a directory containing `case.raw` and `case.dyr` with
`cargo run -p powerio --example dynamic_study`.

```rust,ignore
{{#include ../../powerio/examples/dynamic_study.rs}}
```

`DynamicCase::records()` returns records in source order. Each record exposes
its first-field `key`, raw model name, optional device key, ordered parameter
tokens, source line, and record spelling. An unquoted key is a bus number,
returned by `bus_id()`; a quoted key names a device, as dc line models do
(`'DC1' 'CDC4T' ... /`), and `bus_id()` returns `None` for it. `DynamicCase::to_dyr_string()` returns the
original text exactly, including comments and formatting. `parse_dyr_bytes`
rejects invalid UTF-8 rather than replacing bytes.

## Linking guarantees and limitations

`DynamicStudy::unresolved_buses()` verifies only whether each generic DYR
bus ID occurs in the paired network, and skips records keyed by a device
name. Load models keyed by area, zone, or owner (for example `CLODAR`) state
that number where a bus would be, so the check can report them as unresolved
buses. It does **not** assert that the optional
`device_id` refers to a generator, load, or other particular equipment: the
meaning of that field and the association rule depend on the model family.
Unknown and user-defined models are preserved rather than rejected or
silently dropped. Parameters remain strings until a schema-backed model view
exists.

## Typed model views and generator linking

`powerio::typed_model` reads a record through its PSS/E Model Library
parameter schema for `GENCLS`, `GENROU`, `GENROE`, `GENSAL`, `GENSAE`,
`SEXS`, `ESAC1A`, `EXST1`, `TGOV1`, `GAST`, `HYGOV` and `IEEEST`
(`powerio::SUPPORTED_MODELS`). Parameters keep PSS/E names, order and the
machine base; a record of another model, with the wrong parameter count or a
non-numeric token, returns a `ModelError` and stays available raw. The typed
view never changes the retained records, so `to_dyr_string()` still writes the
source back byte for byte.

`DynamicStudy::machines()` (or `powerio::link_machines`) attaches the typed
records to the generators they name by `(bus, machine id)`. The PSS/E reader
keeps a machine id only when it differs from the positional default, so
`powerio::psse_machine_ids` rebuilds every generator's id with the writer's
allocation rule, and `powerio::generator_source_impedance` returns `ZSORCE`. Each
generator gets at most one machine, exciter, governor and stabilizer; every
record that does not attach is reported as a `LinkIssue` (untyped, no such
generator, or a duplicate role).

Not implemented yet: typed views for further models, load and dc-line
models, DYR-to-DYD/SWI/PowerWorld AUX conversion, transient simulation
(tellegen's `rms` module consumes these views), and dynamics exposure through
PowerIO IR, C ABI, Julia, or Python. This is the initial public API boundary,
not a claim that those features are complete.
