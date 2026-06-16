# PowerFactory

powerio reads DIgSILENT PowerFactory data through the DGS plaintext interchange
format. The `.pfd` project export is not decoded; it is recognized and rejected
with a named error that points at the DGS path.

## Why `.pfd` is not decoded

`.pfd` is DIgSILENT's encrypted binary project export. The payload is
statistically a cipher stream: a uniform byte distribution (chi-square close to
the degrees of freedom), no compression or container structure, no file magic,
and no recoverable record anchors. Without the key there is nothing to parse, and
no public decoder or key exists. Every tool that ingests `.pfd` (pandapower's
converter, powfacpy, PowerMCP, the DIgSILENT-provided import objects) drives a
licensed PowerFactory runtime through its COM or Python API; none parses the
bytes. So powerio rejects `.pfd` rather than guessing, and the error tells the
user to export DGS instead.

## Reading PowerFactory data via DGS

DGS is the format PowerFactory writes for data exchange, and the path the open
tooling (powsybl, GridCal, roseau-load-flow) uses to read PowerFactory data
without the GUI. To get a case out of PowerFactory: File > Export > DGS, then
`parse_file("case.dgs")` (or `parse_file(path, Some("dgs"))`).

DGS is a flat table-per-class plaintext dump of the PowerFactory object model.
Each section is `$$<Class>;col(type:width);...` followed by semicolon-delimited
rows, with `a`/`i`/`r`/`p` column types for string, integer, real, and object
pointer. Connectivity is by ID reference through `StaCubic` cubicles (element
`obj_id` -> cubicle, cubicle `fold_id` -> `ElmTerm` bus, `obj_bus` orders the two
ends), so the reader resolves endpoints in a second pass. The reader keys columns
by descriptor name and pointers by raw string, so it handles both schema
generations: V5 (integer `ID`, dot decimals) and V7 (string `FID`, an extra `OP`
column, comma decimals). Line and transformer electrical parameters come from the
`TypLne`/`TypTr2` type objects; DGS carries no system MVA base, so `base_mva`
defaults to 100, as in the PowerWorld readers. `powsybl-core`'s `powerfactory-dgs`
module (Java) and roseau-load-flow's `from_dgs_file` (Python) are reference
implementations.

The reader maps `ElmTerm` -> bus, `ElmLne`+`TypLne` and `ElmTr2`+`TypTr2` ->
branch (a transformer carries a tap so it reads as one), `ElmSym` -> generator,
`ElmLod` -> load, and `ElmShnt` -> shunt. A slack is set when a machine declares
`ip_ctrl == 1` or `bustp == SL`; otherwise machine buses become PV and, as with a
PowerWorld `.pwb`, an export without a slack designation leaves no reference bus
for `to_normalized` to synthesize one.

## Validation

`powerio/tests/dgs_parity.rs` checks the reader against the MATPOWER companion of
the same standard case, using public MPL-2.0 DGS fixtures under `tests/data/dgs/`
(see that directory's README for provenance and license):

- IEEE 39: the VeraGrid `IEEE_39.dgs` (V5) export decodes to the exact `case39.m`
  topology (39 buses, 46 branches, 10 generators). Because this export carries
  the same per-unit data as case39, the reader's per-unit conversion
  (`rline*dline / zbase`) and transformer Z/tap math are also checked
  numerically: 45 of 46 branches match r, x, and effective tap within 1e-3 (the
  one exception is a transformer whose impedance differs by a single source value
  in the export).
- IEEE 118: the VeraGrid `IEEE118_v2_test.dgs` (V7) export decodes to the
  `case118.m` bus set and 54 generators. It is a different 118-bus dataset (179
  branches vs case118's 186), so its electrical values are not compared.

Topology is provenance-independent and asserted exactly; electrical values are
only compared where the two sources share parameter provenance.
