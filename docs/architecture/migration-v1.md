# v1 naming migration

The v1 compiler-IR architecture names the two model families explicitly and adds
a compiler-package envelope. The historical names keep working; the new names are
introduced as aliases ahead of any breaking rename, so nothing breaks on adoption.

## The three names

| concept | v1 name | what it is |
|---|---|---|
| balanced model | `BalancedNetwork` | scalar positive-sequence transmission model (was, and still is, `Network`) |
| multiconductor model | `MulticonductorNetwork` | wire-coordinate distribution model (was, and still is, `DistNetwork`) |
| compiler package | `CompilerPackage` | the `.pio.json` envelope wrapping one payload; the eventual meaning of a top-level `Network`/package name |

The historical `Network` and `DistNetwork` are not removed. The v1 names are
forward aliases; the structs are unchanged. The top-level `Network` identifier is
not repurposed as the package envelope yet, because it still means the balanced
model and repurposing it would break callers; `CompilerPackage` is the
transitional name until that rename can be staged.

## Migration table

`introduced` names the PR that adds the alias; `removal target` is when a
deprecated name goes away (the forward aliases have none — they are permanent v1
names). The Rust aliases landed in PR #143; the `.pio.json` envelope landed in
the compiler-package PR this document accompanies ("this PR").

| old / current name | v1 name | surface | introduced | removal target | notes |
|---|---|---|---|---|---|
| `powerio::Network` | `powerio::BalancedNetwork` | Rust | PR #143 | none (both kept) | type alias; `Network` stays the struct |
| `powerio_dist::DistNetwork` | `powerio_dist::MulticonductorNetwork` | Rust | PR #143 | none (both kept) | type alias; `DistNetwork` stays the struct |
| — | `powerio_pkg::CompilerPackage` | Rust | this PR | n/a | new envelope crate `powerio-pkg` |
| `powerio.Network` | `powerio.BalancedNetwork` (planned) | Python | planned | none (both kept) | pure-Python alias; deferred to a naming PR |
| `powerio.dist.DistNetwork` | `powerio.dist.MulticonductorNetwork` (planned) | Python | planned | none (both kept) | pure-Python alias; deferred to a naming PR |
| `powerio.Case` | `powerio.Network` | Python | 0.3.3 | 0.4.0 | deprecated back-compat alias (pre-existing) |
| `powerio.dist.DistCase` | `powerio.dist.DistNetwork` | Python | 0.3.3 | 0.4.0 | deprecated back-compat alias (pre-existing) |
| `PioNetwork` | `PioBalancedNetwork` (planned) | C ABI | planned | none | keep `PioNetwork`; add an alias typedef and `pio_balanced_*` doc names |
| `PioDistNetwork` | `PioMulticonductorNetwork` (planned) | C ABI | planned | none | keep `PioDistNetwork`; ABI version unchanged until a real break |
| — | `pio_package_*` / `PioPackage` (planned) | C ABI | planned | n/a | a stable handle for `.pio.json` once the JSON schema settles |
| `PowerIO.Network` (Julia) | `PowerIO.BalancedNetwork` (planned) | Julia | planned | none | wrappers over the C ABI follow the C names |
| `PowerIO.DistNetwork` (Julia) | `PowerIO.MulticonductorNetwork` (planned) | Julia | planned | none | |
| MCP `parse` returns `powerio-json` / `bmopf-json` strings | `parse` returns a `.pio.json` package + summary + diagnostics (planned) | MCP | planned | n/a | add `capabilities` and `diagnostics` tools; package is the transport |
| — | `powerio package <case> -o case.pio.json` (planned) | CLI | planned | n/a | emit a compiler package; `summary`/`diagnostics` read one |

## Per-surface status

### Rust

- `powerio::BalancedNetwork` and `powerio_dist::MulticonductorNetwork` are public
  type aliases, re-exported from each crate root (added in PR #143).
- `powerio_pkg::CompilerPackage` and its supporting types are the new envelope
  (this PR).
- `DistNetwork` is now serde-serializable (the `source` and `defaulted` fields
  are skipped; see ADR 0001), so it can be a `.pio.json` payload (this PR). No
  struct renamed, no name removed.

### Python (deferred)

- The pure-Python aliases `powerio.BalancedNetwork` and
  `powerio.dist.MulticonductorNetwork` are deferred to a separate naming PR, to
  keep this envelope PR narrow. PR #143 added the Rust aliases but not the Python
  ones.

### C ABI, Julia, MCP, CLI (planned)

These adopt the package and the v1 names in later PRs. The C ABI keeps
`PioNetwork`/`PioDistNetwork` and its current ABI version; alias typedefs and a
`.pio.json` handle come without a breaking change. Julia mirrors the C names. The
MCP server moves to package transport (`parse -> {package, summary, diagnostics}`)
and gains `capabilities`/`diagnostics` tools. The CLI gains a `package` command
that emits `.pio.json`. None of these is in this change set; this table is the
plan of record.

## Compatibility promise

- No existing public name is removed by this change set.
- The forward aliases are permanent; code may move to them at any pace.
- The two deprecated back-compat aliases (`Case`, `DistCase`) remain on their
  pre-existing 0.4.0 removal schedule.
- `.pio.json` is versioned by `schema_version` (semver); additive fields bump the
  minor and a reader tolerates unknown future fields.
