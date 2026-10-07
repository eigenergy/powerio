# PowerIO SINCAL transport and schema support

Internal support for PowerIO's balanced and multiconductor SINCAL adapters.
This crate acquires bounded SQLite payloads from memory or native archives,
validates archive paths and retains source companions. It also validates the
supported SQLite schema's version, base variant, node/element identities and
ordered terminal references on a bounded query-only snapshot. It does not
interpret electrical fields, select a model family, or produce a network.
Its interfaces are implementation details, not a supported parsing
API. Public SINCAL parsing and emission remain under development.

Direct SQLite structural admission is pinned to observed electrical schema
14.8. Explicit MDB Tools acquisition records admit schema 11.5. Both select an
explicit base variant (or the sole variant); derived-variant inheritance and
other schema versions remain unsupported. The database connection retains
the existing query budget, attachment ban, size limits and query-only mode.

It depends on `powerio-core` for source ownership, and on ZIP and SQLite libraries
for transport and database access, never on `powerio-tx`, `powerio-dist`, the
facade, matrices or solvers. Model-specific
validation and diagnostics stay in their owning adapters. Successful structural
validation does not establish that either electrical profile can be mapped.
The native archive
and database are read without opening paths stored inside `database.ini`.

Published with the workspace so component crates can depend on this support
without depending on one another. No third-party model fixtures are packaged.

`TableRecords` decodes the internal `powerio-sincal-tables` document produced by
the explicit Python/MDB Tools adapter under `evals/sincal/`. The library never
spawns a process. Column declarations, absent/excluded tables, NULL cells and
typed integer/real/text values remain distinct. Materialization uses bound
values and no column affinity or native SQL declarations. The record document
is not PowerIO IR or an authentic native SQLite export, and its claimed MDB
digest is provenance metadata rather than an authenticity guarantee. Keep the
original source separately; the document cannot reproduce original MDB bytes.
