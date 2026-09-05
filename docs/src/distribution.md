# Distribution networks

A multiconductor network is the conductor level distribution model, and
OpenDSS, PowerModelsDistribution engineering JSON, and BMOPF JSON all parse to
it. When you need a calculation, construct an `McAcPfInstance` or
`McAcOpfInstance` from it explicitly (see
[Calculation instances and solutions](instances.md)).

```julia
using PowerIO
feeder = parse("IEEE13Nodeckt.dss")        # PioModule{MulticonductorNetwork}
net = feeder.value
net.lines[1]                               # terminal maps and the line code reference
net.linecodes[1]                           # per length impedance matrices, SI units
feeder.diagnostics                         # what the reader kept, assumed, or refused
```

The model identifies individual conductors throughout. Buses have ordered
terminals and explicit grounding, lines and switches map conductors between
terminal sets, transformers list their windings with connection kinds, and
loads and generators attach per terminal. Impedance and shunt matrices are in
SI units, per unit length, on line codes. An element the reader has no typed
slot for stays verbatim in the `untyped` table and is reported.

The OpenDSS profile is the static circuit, meaning the element definitions
and their electrical data. Load shapes, solve commands, monitors, and other
calculation instructions are outside it; they stay in the retained source,
are reported as uninterpreted, and survive a same format write byte for byte.

```julia
emit(feeder, "dss", "copy.dss")            # the source bytes, sidecars included
result = emit(feeder, "pmd")               # fresh PMD JSON
result.text
result.diagnostics
```

Nothing converts between the multiconductor and balanced models implicitly.
The balanced positive sequence equivalent is an explicit transformation that
reports its assumptions, such as voltage bases per zone, phase aggregation,
and switch merging, and refuses what it cannot represent:

```rust,ignore
use powerio::transform::{to_balanced, to_balanced_report};

let report = to_balanced_report(&feeder)?;   // readiness, assumptions, losses, diagnostics
let balanced = to_balanced(&feeder)?;        // PioModule<BalancedNetwork>
```

Python exposes the same pair as `module.to_balanced_report()` and
`module.to_balanced()`. The transformation has no C entry point in 0.11, so
PowerIO.jl does not bind it.

Multiconductor admittance matrices build directly from the multiconductor
network through `powerio_matrix::calc_multiconductor_admittance_matrix`,
which is Rust only in 0.11.

## BMOPF proposal profiles

PowerIO v0.11.0 supports a pinned BMOPF 0.2.0 **proposal**, subject to Task Force
review. The schema version is separate from the PowerIO release and from
PowerIO IR generation 2. Producer provenance records the proposal revision and
schema digest. Previously emitted schema identifiers remain readable aliases.

An unqualified `bmopf-json` emission preserves an unchanged source byte for
byte. An explicit profile always re-encodes the selected version:

```sh
powerio convert feeder.json --to bmopf-json@0.1.0 -o legacy.json
powerio convert feeder.json --to bmopf-json@0.2.0 -o proposal.json
```

The same format strings work with Rust `powerio::emit`, Python `powerio.emit`,
C `pio_emit`, Julia `emit` and the PowerIO MCP emission tool. Rust also exposes
`BmopfProfile` and `BmopfEmitOptions` for the distribution writer.

Legacy output relocates proposed-only equipment and transformer fields into
`extras`, with diagnostics. A consumer that ignores those extensions cannot be
assumed to calculate the same network. Proposal output uses the declared tables
and preserves winding ratings, taps, neutral impedances and current-limit data.

Unequal bus phase bounds remain individual values through parsing, generation-2
IR and explicit BMOPF output. Uniform values use `v_min`/`v_max`; unequal values
use `v_min_phase`/`v_max_phase` in the typed Rust model. A present scalar takes
precedence over the corresponding vector. PMD voltage arrays instead follow
terminal order and use engineering voltage units.

The C and Julia bus views expose unequal limits as
`phase_to_ground_voltage_min_v` and `phase_to_ground_voltage_max_v`, while
`voltage_min_v` and `voltage_max_v` represent uniform bounds. C spans borrow
from the network's owner and remain valid for the lifetime of that handle.
The unreleased ABI 7 layout and Julia definitions are updated together.

Parsing, structural schema validity, semantic consistency and computational
support are separate checks. Contradictory versions, invalid dimensions,
unknown electrical references and inconsistent bounds produce error diagnostics.
The native multiconductor admittance builder supports ideal grounded-WYE
transformer coupling and rejects leakage, other winding connections, floating
neutrals, core shunts or tap decisions that need a different formulation.

The BMOPF proposal schema is pinned to
[`2560b3b`](https://github.com/distribution-system-opt/dsopt-schema/commit/2560b3bb9cdfee89321e02a12c320c9fb6519af0),
with SHA-256 `7f8ab08476b610d7bc9ecf3539d209b3bc212e8472bc38987afedc1163d71bce`.
Fresh 0.2.0 output places the immutable retrieval URL in `meta.$schema` and
records the canonical identity, proposal status, digest and revision under
`meta.provenance.powerio_bmopf`. Existing provenance is preserved; a name
collision uses a numbered producer entry. Reading the former canonical or
version-directory identifiers remains supported without fetching remote data.
