# Integrated transmission/distribution modeling

`powerio-tnd` owns the relationship between `BalancedNetwork` and
`MulticonductorNetwork`. Its Rust API is also available through `powerio::tnd`.
The component crates remain independently usable. Cross-model transformations
live in tnd; facade transformations add the common module record handling.

`IntegratedNetwork::new` takes a transmission `PioModule`, a map of named feeder
modules, and explicit interfaces. Each interface identifies a transmission bus,
a feeder voltage source, and its A/B/C terminals. Names stay scoped to each
component, and the original modules retain their source bytes and records.
Construction checks references, shared frequency, competing source constraints,
phase maps, and the load replacement policy. Closed ideal switches join terminals
by their terminal maps, so a source, ground, or other interface reached through
them counts as competing at the terminal it reaches. Its validated contents are
immutable.

An interface states a positive voltage ratio, which `Interface::new` requires,
and an angle displacement in radians. Construction checks the selected
source's stated phase voltages against it: each magnitude must lie within 0.5
to 2 times the bus phase voltage times the ratio, and the stated angles must
rotate A, B, C in the phase map's order. A wrong ratio or a swapped phase map
therefore fails before any coupled equation uses it.
`boundary_voltages` converts transmission positive-sequence per-unit voltage using
the bus line-to-line kV base into feeder phase-to-earth volts, in A/B/C order.
`boundary_power` sums V times conjugate(I) and returns MW + j MVAr, positive into
the feeder. Each current is measured in amperes with the same phase order.
The interface is lossless. A consumer supplies any substation transformer losses
inside a component model; different component MVA bases do not alter these SI
boundary quantities.

The consumer replaces the selected source's three phase voltage equations with
coupled equations. Additional zero-volt source terminals remain earth clamps.
Floating-reference sources are refused by this initial interface rules.
`TransmissionDemand::Additional` adds the feeder demand at the transmission bus;
`ReplaceLoads` names the active load UIDs to substitute when assembling coupled
equations. Keeping this choice explicit prevents double-counting feeder demand.
The retained standalone component records are unchanged, so a consumer must apply
these substitutions when building an integrated calculation.

## Formats and migration

SINCAL is the first adapter under `powerio_tnd::formats::sincal`. Enable the
`sincal` feature to acquire and retain native projects, parse explicit balanced
or multiconductor profiles, and author the supported experimental outputs.
Storage internals remain hidden from normal rustdoc. The existing `powerio`
format names, CLI behavior, Python API, and C ABI 7 remain available.
Direct Rust users of the unreleased SINCAL adapters move from the tx/dist crates
to tnd. `powerio::dist::parse_sincal` remains available as a facade convenience.
The published `powerio-sincal-tables` transport identity remains unchanged.

SINCAL profile parsing produces a component module. An integrated case requires
explicit interfaces; voltage levels alone cannot establish a network partition
or determine which transmission load a feeder replaces. Existing MATPOWER and
OpenDSS modules can be composed through the same generic API.

## Extension points

A future native integrated format should store component references and an
interface manifest, with a versioned boundary schema and retained source records.
A [PowerModelsITD.jl adapter](https://lanl-ansi.github.io/PowerModelsITD.jl/stable/)
can map its component files and boundary definitions into this composition.
Both adapters should reuse the component parsers and checked interface constructor.

The initial integration API is Rust-only. PowerIO IR version 2 and the facade's
`PioValue` variants are unchanged. A future integrated IR variant and binding
accessors need a separate compatibility design. Coupled calculation instances
belong in `powerio-prob`, while sparse coupled operators belong in
`powerio-matrix`. Solver algorithms remain downstream.
