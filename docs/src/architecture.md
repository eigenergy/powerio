# PowerIO's intermediate representations

PowerIO borrows one idea from compiler infrastructure: source text and the data you compute with are different representations, and the way between them is a set of explicit, verifiable operations. This page defines what "intermediate representation" means here, since the term promises more than any one type delivers.

## The module is the unit

`PioModule<T>` is the top level unit. It pairs one typed value with everything needed to understand how that value was produced: the retained source, the reader's diagnostics, the source map, and the history of transformations applied. Operations take modules and produce modules, so the record survives a pipeline: parse, select a scenario, lower to balanced, write, and the result still knows where its bytes came from.

## The IR is a family, not a format

PowerIO's intermediate representation is the family of typed, source neutral in-memory values it can parse, inspect, transform, and write: the two network models, operating points, time series, scenario sets, the seven calculation instances, and the seven solutions. Source neutral means independent of any source file's syntax: a `BalancedNetwork` parsed from PSS/E and one parsed from MATPOWER are the same type with the same meanings. It does not mean every source can be represented without limits; each format has a documented profile, and data outside it is retained and reported rather than silently absorbed.

There is no one universal network format in this family. `BalancedNetwork` is the reusable typed value for the supported balanced electrical profile; it does not absorb multiconductor data, other energy carriers, or source specific calculation data. `MulticonductorNetwork` stands beside it, not below it.

## The dynamic boundary

`PioValue` is the closed sum of the built in value families, used where something must discover at run time which value is present: automatic parsing, the stored document, and the C, Python, Julia, and MCP boundaries. It is not the base class of all possible module values. Rust code can put any type in a module (`PioModule<MyApplicationType>`) and get the same source, diagnostic, and history behavior; the type stays out of the dynamic boundary until PowerIO adds a variant, a stored schema, and binding tests.

## Stored documents are serializations

`.pio.json` is one versioned serialization of the dynamic module. It is the way to persist any value kind with its records, and its schema version moves independently of the package version. It is not the IR itself and not a preferred exchange format; the case formats remain the exchange surface.

## Transformations name their input and output

Every transformation states its concrete input and output types and returns diagnostics. Multiconductor to balanced conversion is an explicit lossy lowering: it moves to a less detailed representation under stated assumptions. Constructing a calculation instance from a network is also a move to a more specific calculation representation. Ordinary format conversion is not called lowering; it is parse plus write at the same level.

## Derived data stays derived

Sparse matrices, dense solver rows, factorization state, and caches are analysis data computed from the IR, carrying element mappings back into it. They are not part of the electrical ontology, are not stored in modules, and can change representation without changing any public meaning.

The design decisions PowerIO genuinely shares with LLVM and MLIR, and the ones it deliberately does not, are recorded in the Developer Guides: [LLVM and MLIR lessons](compiler-ir.md). The component layout behind these types is drawn and checked in [Architecture map](architecture-map.md).
