# PowerIO's intermediate representations

PowerIO borrows one idea from compiler infrastructure: source text and the data you compute with are different representations, and the way between them is a set of explicit, verifiable operations. This page defines what "intermediate representation" means here, since the term promises more than any one type delivers.

## The module is the unit

`PioModule<T>` is the top level unit. It pairs one typed value with everything needed to understand how that value was produced: the retained source, the reader's diagnostics, the source map, and the history of transformations applied. Collection access returns typed entries; transformations and solvers derive new modules while preserving the relevant records. `emit` borrows a module and returns external artifacts plus emission diagnostics; matrix calculations borrow the typed value and return derived data with element mappings.

## The IR is a family, not a format

PowerIO's intermediate representation is the family of typed, source neutral in-memory values it can parse, inspect, transform, and emit: the two network models, operating points, time series, scenario sets, seven calculation instances, and eight solution types. The extra solution type is `SocwrOpfSolution`, which records an SOCWR relaxation of an `AcOpfInstance`. Source neutral means independent of any source file's syntax: a `BalancedNetwork` parsed from PSS/E and one parsed from MATPOWER are the same type with the same meanings. It does not mean every source can be represented without limits; each format has a documented profile, and data outside it is retained and reported rather than silently absorbed.

There is no one universal network format in this family. `BalancedNetwork` is the reusable typed value for the supported balanced electrical profile; it does not absorb multiconductor data, other energy carriers, or source specific calculation data. `MulticonductorNetwork` stands beside it, not below it.

## The dynamic boundary

`PioValue` is the closed sum of the built in value families, used where something must discover at run time which value is present: automatic parsing, the stored document, and the C, Python, Julia, and MCP boundaries. It is not the base class of all possible module values. Rust code can put any type in a module (`PioModule<MyApplicationType>`) and get the same source, diagnostic, and history behavior; the type stays out of the dynamic boundary until PowerIO adds an enum case, a stored schema, and binding tests.

## Stored documents are serializations

`.pio.json` is the PowerIO IR serialization of the dynamic module. It persists any registered value type with its records using `"schema": "powerio.module"` and the `powerio` crate version, currently `"version": "0.11.0"`. The IR is the in-memory module; `.pio.json` is its stored form, separate from grid exchange formats.

## Transformations name their input and output

Every transformation states its concrete input and output types and returns diagnostics. Multiconductor to balanced conversion moves to a less detailed representation under stated assumptions and reports what it cannot carry. Constructing a calculation instance from a network moves to a more specific calculation representation. Ordinary format conversion is parse plus emit at the same level.

## Derived data stays derived

Sparse matrices, dense solver rows, numerical factors, and caches are analysis data computed from the IR, carrying element mappings back into it. They are not part of the electrical ontology, are not stored in modules, and can change representation without changing any public meaning.

The design decisions PowerIO genuinely shares with LLVM and MLIR, and the ones it deliberately does not, are recorded in the Developer Guides: [LLVM and MLIR lessons](compiler-ir.md). The component layout behind these types is drawn and checked in [Architecture map](architecture-map.md).
