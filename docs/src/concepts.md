# Core Concepts

Five concepts carry the whole API: the module, the value families, diagnostics, sources, and format profiles. Every language exposes them under the same names.

## The module

Every successful parse returns a module: `PioModule<T>` in Rust, `PioModule{T}` in Julia, `PioModule` in Python and C. A module holds exactly one typed value beside the records that explain it: the source it was parsed from, the reader's diagnostics, and the history of operations that produced the current value. Because the module keeps the source, writing it back to the same format reproduces the input byte for byte, and diagnostics can point at byte ranges of the actual file.

The typed value is the data you work with. In Rust and Julia the module's type parameter names it, so ordinary dispatch and type assertions work; in Python and C the module reports its kind as a stable string.

## The value families

The kind strings are permanent identifiers, shared by every language, the stored document, and the MCP tools.

| Kind | What the source declared |
|---|---|
| `balanced_network` | a balanced positive sequence transmission network |
| `multiconductor_network` | a conductor level distribution network |
| `balanced_network_time_series` | a network whose inputs vary over declared time points |
| `balanced_operating_point_time_series` | a fixed network with a complete electrical state per time point |
| `multiconductor_operating_point_time_series` | complete sampled multiconductor states over time |
| `balanced_network_scenario_set` | named alternative networks over shared element identities |
| `dc_pf_instance` … `ac_scuc_instance` | the input of one named calculation (seven families) |
| `dc_pf_solution` … `ac_scuc_solution` | the result of one named calculation, sharing its instance (seven families) |

The two network types stay distinct. `BalancedNetwork` is the balanced positive sequence transmission model that MATPOWER, PSS/E, PowerModels JSON, and the other balanced formats meet at. `MulticonductorNetwork` is the conductor level distribution model that OpenDSS and PMD engineering JSON meet at, with per conductor impedance matrices, terminal maps, and grounding. Neither is a subtype of the other. Converting a multiconductor network to a balanced positive sequence equivalent is an explicit transformation that reports its assumptions and losses.

An instance is the complete input for one named calculation and contains or shares its network. A solution is the result of one calculation and contains or shares the instance it solves. A source parses to an instance or solution only when it declares that calculation: a MATPOWER case carries ratings and costs several calculations can use, so it stays a network; a BMOPF file defines a multiconductor AC OPF, so it parses to `mc_ac_opf_instance`.

`TimeSeries<T>` is an ordered sequence of complete values of one type; the element type states what varies. `ScenarioSet<T>` is a set of named alternatives with no implied order. Selecting one entry shares the base network's data; nothing reparses or copies numerical tables.

## Diagnostics

Every operation reports findings as structured diagnostics, never as bare strings. A diagnostic has a stable dotted code (`READ.DSS.INCLUDE_BUDGET`), one of four severities (`error`, `warning`, `remark`, `note`), a message, and, where they apply, a target naming the element or construct, byte spans into the retained source, related records, and a suggested action. Branch on the code; the message is explanation, under no stability promise.

A successful parse keeps its findings on the module. A failed operation raises the language's error carrying the same records. A module can hold an error severity finding and still be usable: the reader represented the value, and the finding says what is wrong with it.

## Sources and formats

A parse reads a source: one or more named immutable byte buffers, acquired from a file, a directory, or memory. The format is detected from the name and content; passing a format name overrides detection for ambiguous or mislabeled input without changing anything else about the parse. Canonical format names are stable lowercase strings (`matpower`, `psse`, `dss`, `pypsa-csv`), the same in every language, the CLI, and MCP. Common names such as `opendss` remain accepted aliases.

`resolve_format` maps a common alias such as `raw34` or `opendss` to the
canonical token and reports the conventional filename suffix, whether a
destination is a directory, and whether a fresh universal emitter exists for
the format. The suffix has no leading dot and can be compound. The capability
is not a promise for every module value kind or a build feature probe.
Applications use that answer for file pickers and downloads instead of copying
a format table or depending on a component enum.

## Format profiles

For each format, PowerIO documents the portion it supports: the profile. Every field inside the profile becomes typed data or produces a diagnostic saying why it cannot be represented. Data outside the profile stays in the retained source, so writing the same format back loses nothing, and converting to another format reports what the target cannot carry. PowerIO does not claim complete support for a format when it supports one profile of it; [Formats and Fidelity](format-fidelity.md) states each profile.
