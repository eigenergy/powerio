# Reference buses and balancing sources

PowerIO currently has one bus designation, `BusType::Ref`. Treat it as the
network's **voltage-angle reference**, not proof that an in-service generator
can balance active power. The typed generator table remains the source of truth
for generator-backed supply. This distinction matters because exchange formats
do not all encode the two roles the same way.

## Current format mappings

| Format | Source representation | PowerIO mapping / caveat |
| --- | --- | --- |
| MATPOWER | Bus type 3 denotes the reference/slack bus; generators are separate rows. | Bus type 3 maps to `Ref`; generator rows map independently. A type-3 bus without a generator can be represented, but may not be solvable by a downstream power-flow solver. |
| PSS/E RAW | Bus type 3 identifies the swing bus; generator data is carried in separate machine records, and the RAW convention requires a machine record for type-2/type-3 buses. | The reader maps the swing designation to `Ref` and reads machine records separately. A type-3 bus without a machine is not a valid PSS/E RAW case; this is a source-format constraint, not a universal PowerIO invariant. |
| pandapower | `gen.slack` and `ext_grid` represent balancing/slack equipment. | A slack `gen` or `ext_grid` sets the bus to `Ref`; `ext_grid` rows also map to PowerIO generators. On write, a `Ref` bus with no generator gets a synthesized zero-output `ext_grid`, because pandapower needs a slack element. The writer emits a diagnostic: on read-back, the synthesized element becomes a generator. |
| CIM CGMES | Reference priority is carried on synchronous-machine/reference-related equipment rather than solely as a MATPOWER bus code. | The balanced model currently has no separate typed reference-priority field. Any mapping to `BusType::Ref` is therefore a projection; round-trip fidelity depends on retained source data and the specific CIM profile. |
| PowerSystems.jl | Distinguishes reference-bus (`REF`) and slack-source (`SLACK`) concepts. | PowerIO's current `BusType` does not encode both independently. Do not map `SLACK` to an assumption that a `Ref` bus must contain a generator without checking the source representation. |

## Island behavior

- `IslandReferencePolicy::Stated` preserves the reference designation in the
  supplied model. It does not certify that the island has an active-power
  balancing source.
- `IslandReferencePolicy::PerIsland` is a solver-readiness policy. It chooses
  a reference on supplied islands and de-energizes islands with no in-service
  generator. A reference-only island is therefore retained under `Stated`,
  but de-energized under `PerIsland`.
- `Island::is_supplied()` reports whether the island has an in-service
  generator; it is not a test for whether a reference bus is designated.

## Projection rule

Readers should preserve source reference designation and source equipment
records independently whenever the source format allows it. Writers should
project the typed model into the target format's required convention and emit a
diagnostic when that projection synthesizes or loses a balancing-source
distinction. In particular, a target that requires a slack element may need a
synthetic element; that synthetic record must not be mistaken for a source
generator in the original model.

A future additive model change may represent angle-reference designation and
balancing-source role as separate typed properties. This change documents the
current contract and locks down island behavior without introducing a breaking
public struct-field change.
