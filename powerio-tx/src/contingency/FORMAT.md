# PSS/E contingency analysis file notes

Maintainer notes for the readers in this module. This file is the format
reference the reader code cites. The user facing behavior summary lives in the
guide's format fidelity chapter. Fixture provenance is in
`tests/data/psse/contingency/README.md`.

## The three contingency analysis files

A PSS/E contingency analysis reads three text files next to a case:

- `.con`, the contingency description file: which outages to apply.
- `.sub`, the subsystem description file: which buses, areas, zones, owners,
  and voltage levels make up each named subsystem. An automatic specification
  in a `.con` expands over a subsystem named here.
- `.mon`, the monitored element file: which branches, interfaces, and bus
  voltages the analysis reports on.

This module reads `.con`. The `.sub` and `.mon` sections of these notes are
added with those readers.

## Why the grammar below is stated from files

Siemens documents these files in the PSS/E Program Operation Manual, which
is licensed and not public. Every statement this reader accepts was therefore
established from public contingency files, from the example set PSS/E ships,
and from public question threads, and is listed with its evidence in the
fixture README. A statement outside the list keeps its line, with the
surrounding whitespace dropped, rather than failing the file, so a file this
reader has never seen still reads, and a new spelling shows up as a
`READ.CON.STATEMENT_UNRECOGNIZED` note rather than as a silent loss.

## Tokenizer

`lexer.rs` holds the tokenizer the three contingency analysis files share.

- Whitespace runs separate tokens; leading and trailing whitespace is dropped.
- A token opening with `'` or `"` may hold spaces. The quotes are removed and
  the inner text is kept. A quote followed by whitespace or by the end of the
  line closes the token; a quote sitting inside a word is part of the name,
  and the search continues to the last quote of that character on the line.
  That is how `'L_000022O'~1'` is one token whose text is `L_000022O'~1`. A
  quote that never closes runs to the end of the line, without the line's
  trailing whitespace.
- A line whose first non-blank character is `/`, `!`, or `#`, or whose first
  token is `COM`, is a comment. `//` falls under the `/` case.
- An unquoted token whose first character is `/`, after at least one statement
  token, ends the statement: the rest of the line is a comment.
- Every line keeps its original text and its 1-based number.

A case name is the token after the `CONTINGENCY` keyword, trimmed. The quote
rule above is what keeps a name holding an apostrophe whole, so
`CONTINGENCY 'L_000022O'~1'` names `L_000022O'~1`, `CONTINGENCY F01` names
`F01`, and a trailing comment stays out of the name.

A `CONTINGENCY` line stating a further token, `CONTINGENCY A B`, opens the case
with the first token as its name and reports the rest as
`READ.CON.SOURCE_MALFORMED`; those tokens are not kept. The case opens rather
than the line being kept as text, because a line that opened no case would
leave that case's `END` to terminate the file and every statement after it to
read as text.

## File structure

Optional header comments, then any mix of `CONTINGENCY` blocks, automatic
specifications, `SKIP` blocks, and file level statements, then an optional file
level `END`. Header comments are kept as written, which preserves the
`/PSS(R)E 35` stamp and the `COM` banner PSS/E writes. Comment lines elsewhere
are dropped, after the file level `END` included. Statement lines after that
`END` are kept as file level statements marked `after_end`, and reported once.
The writer states them after the `END` it writes, so a `SKIP` or a
`CONTINGENCY` the file states past its terminator reads back as text rather
than as grammar.

Three conditions refuse the file, each naming its 1-based line: a
`CONTINGENCY` that starts before the previous case reached `END`, a case still
open at end of input, and a `SKIP` or dispatch block still open at end of
input.

## Statements inside a case

Keywords are case insensitive. `i`, `j`, and `k` are non-negative integers; a
bus token that is not one keeps the line as text, which is how a file written
in bus-name mode reads. `BUS` before a bus number is optional after `TO`. An
absent circuit tail means circuit `1`, and `CIRCUIT`, `CKT`, `CIRCUITS`, and
`CIRCUT` all introduce one.

| Statement | Synonyms | Reads as |
| --- | --- | --- |
| `OPEN LINE FROM BUS i TO BUS j [CIRCUIT c]` | `OPEN`, `TRIP`, `DISCONNECT`; `LINE`, `BRANCH` | `OpenBranch { from, to, circuit }` |
| `OPEN BRANCH FROM BUS i TO BUS j TO BUS k [CIRCUIT c]` | as above | `OpenThreeWinding { buses, circuit }` |
| `OPEN THREEWINDING AT BUS i TO BUS j TO BUS k [CIRCUIT c]` | `OPEN`, `TRIP`, `DISCONNECT` | `OpenThreeWinding { buses, circuit }` |
| `REMOVE MACHINE id FROM BUS i` | `REMOVE`, `TRIP`; `MACHINE`, `UNIT` | `RemoveMachine { bus, id }` |
| `ADD MACHINE id TO BUS i` | `MACHINE`, `UNIT` | `AddMachine { bus, id }` |
| `REMOVE SHUNT [id] FROM BUS i` | `REMOVE`, `TRIP` | `RemoveShunt { bus, id }` |
| `REMOVE SWSHUNT FROM BUS i` | `REMOVE`, `TRIP` | `RemoveSwitchedShunt { bus }` |
| `REMOVE LOAD [id] FROM BUS i` | `REMOVE`, `TRIP` | `RemoveLoad { bus, id }` |
| `DISCONNECT BUS i` | | `DisconnectBus { bus }` |
| `INCREASE BUS i LOAD BY x MW` | `INCREASE`, `RAISE`, `DECREASE`; `MW`, `PERCENT` | `ChangeLoad { bus, change }` |
| `SET BUS i GENERATION TO x PERCENT` | `LOAD`, `GENERATION` | `ChangeGeneration { bus, change }` |

An element id may be quoted and may carry padding; `CKT '1 '` is circuit `1`.
An amount written `100%` states 100 percent. A non-finite amount keeps the line
as text, because no written form of one reads back.

A line whose last token is `DISPATCH`, or whose first two tokens are
`DEFAULT DISPATCH`, opens a nested block that ends at the next `END`. The
second rule covers the direction and level forms PowerGEM writes,
`DEFAULT DISPATCH DOWN`, `DEFAULT DISPATCH UP`, and
`DEFAULT DISPATCH FIRSTLEVEL`, whose `END` would otherwise read as the file
`END`. Inside a case the whole block, its `END` included, is one
`Unrecognized { text }` whose lines are joined with newlines, so writing it and
reading it again gives the same block. Opening a block is reported once, at the
opening line, as `READ.CON.STATEMENT_UNRECOGNIZED`.

## File level statements

| Statement | Reads as |
| --- | --- |
| `SINGLE BRANCH IN SUBSYSTEM name [3WLOWVOLTAGE]` | `AutomaticSpec`; `SINGLE`/`DOUBLE`, `BRANCH`/`LINE`, `UNIT`/`MACHINE`, `TIE`, and `IN`/`FROM` |
| `SKIP` ... `END`, holding `i TO j [CIRCUIT c]` or `FROM BUS i TO BUS j [CIRCUIT c]` | `SkipRule` per line |
| `DEFAULT DISPATCH [DOWN\|UP\|FIRSTLEVEL]` ... `END` | one `RetainedStatement` holding every line |

Anything else at file level, `BUSNUMBERS`, `BUSNAMES`, `BRANCHNAMES`, and the
TARA-only statements among them, becomes a `RetainedStatement` with its line
number and is reported. A retained statement holds the line with its
surrounding whitespace dropped, as every statement kept as text does. A line
inside a `SKIP` block that states no branch is reported as
`READ.CON.SOURCE_MALFORMED`, a warning, and kept the same way.

The reader records at most 16 notes. A 17th finding records one
`READ.CON.NOTES_TRUNCATED` in place of its note and every finding after it
records nothing, so a file of unrecognized lines cannot grow the note list
without limit. A file of exactly 16 findings gets 16 notes and no marker. Every
line is still kept; only the notes stop.

## What the writer states

`to_con` writes the header lines as written, then the automatic
specifications, then one `SKIP` block holding every skip rule, then the cases
in order, then the file level statements kept from before the file `END`, then
a final `END`, then the statements read after that `END`. Every line ends with
a newline. Reading the result gives the same set, except that a statement kept
from the middle of a file is written after the cases and so reads back from a
different line.

| Value | Written as |
| --- | --- |
| case | `CONTINGENCY`, the quoted name, the actions, `END` |
| `OpenBranch` | `OPEN LINE FROM BUS {from:>6} TO BUS {to:>6} CIRCUIT {c}` |
| `OpenThreeWinding` | `OPEN THREEWINDING AT BUS {a:>6} TO BUS {b:>6} TO BUS {c:>6} CIRCUIT {ckt}` |
| `RemoveMachine` | `REMOVE MACHINE {id} FROM BUS {bus:>6}` |
| `AddMachine` | `ADD MACHINE {id} TO BUS {bus:>6}` |
| `RemoveShunt` | `REMOVE SHUNT [{id} ]FROM BUS {bus:>6}` |
| `RemoveSwitchedShunt` | `REMOVE SWSHUNT FROM BUS {bus:>6}` |
| `RemoveLoad` | `REMOVE LOAD [{id} ]FROM BUS {bus:>6}` |
| `DisconnectBus` | `DISCONNECT BUS {bus:>6}` |
| `ChangeLoad`, `ChangeGeneration` | `INCREASE\|DECREASE BUS {bus} LOAD\|GENERATION BY {x} MW\|PERCENT`, or `SET BUS {bus} LOAD\|GENERATION TO {x} MW\|PERCENT` |
| `Unrecognized` | its text |
| `AutomaticSpec` | `SINGLE BRANCH IN SUBSYSTEM` and the quoted name, `UNIT` with `IN`, `TIE` with `FROM`, and ` 3WLOWVOLTAGE` appended when set |
| `SkipRule` | `{from:>6} TO {to:>6} CIRCUIT {c}` inside one `SKIP` ... `END` block |

A case name and a subsystem name are written quoted, as PSS/E writes them. An
id and a circuit are written quoted when the value is empty, holds whitespace,
opens with `/`, or holds a quote character, and bare otherwise; an unquoted
token opening with `/` would end the statement. The delimiter is the quote
character the value does not hold: `"` around a value holding an apostrophe,
`'` around every other value. So `O' HARE` is written `"O' HARE"` and a circuit
`/1` is written `'/1'`, and each reads back unchanged.

A value holding both `'` and `"` has no delimiter that closes it, because a
quoted token ends at the first quote of its own character that whitespace or
the end of the line follows. The reader therefore keeps a `CONTINGENCY` line,
an automatic specification, a `SKIP` line, or an action naming such a value as
text, reported as `READ.CON.SOURCE_MALFORMED`, rather than stating a value no
written line reads back. A set read from a file names only values the writer
states.

A float is written with its `Display` form, which reads back as the same
`f64`.

## Kept as text, not yet established

These appear in public files and in question threads, and no public source
states their meaning well enough to read them into typed actions. They keep
their lines, with the surrounding whitespace dropped:

- `BUSDOUBLE` and the other `DOUBLE` spellings beyond
  `DOUBLE BRANCH|UNIT|TIE IN|FROM SUBSYSTEM`.
- `PARALLEL` branch statements.
- What a `DISPATCH` block's body means: the subsystem, the participating
  machines, and the dispatch method are kept as lines rather than as fields.
  Every such block is reported at its opening line.
- Bus-name mode, where a statement names `'02CHAMBR 345'` in place of a bus
  number. Reading these needs the case, which this module does not take.

## Resolution against a network

`ContingencySet::resolve` binds a set to a `BalancedNetwork`. It is separate
from reading, because a `.con` file names elements the way a RAW file does and
a network does not carry those names.

### Why the ids are recomputed

A network row's `uid` is either the identity its source stated or one PowerIO
generated from bus numbers: `bus-4` for a load, `3-1` for a branch. Neither
form carries a machine or a circuit id, so a `.con` statement cannot be matched
against it. The PSS/E ids are not stored either: the reader drops an id of `1`
because it is what the writer allocates positionally, and it keeps a machine id
only when the writer would have allocated a different one.

`PsseEquipmentIndex` therefore recomputes, for every element, the id a RAW file
written from this network would state, using the writer's own allocation: the
element's retained id when it has one and that id is still free on its key,
else the lowest positive integer still free there. An element thus answers to
the words PSS/E itself would address it by.

| Family | Preferred id | Allocation key |
| --- | --- | --- |
| machine | the `psse_eqid` property of the generator's `ComponentId` in `detailed_connectivity` | the bus |
| branch | the branch's `extras["id"]` | the stored terminal pair `(from, to)` |
| two winding transformer | `extras["id"]`, else the retained `psse_eqid` | the stored terminal pair, allocated apart from the lines |
| load, fixed shunt, switched shunt | `extras["id"]` | the bus, each family allocated apart |
| three winding transformer | `extras["id"]`, else the retained `psse_eqid` | the position among the transformers on the same ordered bus triple |

Every family is keyed on the trimmed id the writer allocates, because PSS/E
reads a quoted id by its trimmed text, as the `.con` reader and the RAW reader
both do. The writer's allocation keeps two rows on one key apart under that
reading: PSS/E forbids an apostrophe inside a quoted field and the writer
replaces one with a space, so an id whose sanitized form trims onto an id
already stated there takes a free positional id instead. A bus carrying `a'`
and `a` states `a ` and `1`, and a statement naming either binds to the row the
RAW file states it for.

A branch lookup reads both orientations, so a statement naming `1 TO 3` finds a
branch stored `3 1`. A self-loop is counted once. The lines are keyed apart
from the two winding transformers, because the writer allocates the two
families in separate namespaces and a line and a transformer on one terminal
pair therefore both carry circuit `1`. A lookup reads the lines first and the
transformers only when no line carries the circuit id, so that pair of rows
states one branch rather than an ambiguity.

Zero rows is not found. More than one is ambiguous and binds to nothing, which
happens when two parallel branches of one family are stored in opposite
terminal orders and take the same circuit id. A three winding transformer
matches on its three buses in any order, and two transformers on one bus triple
stored in different winding orders are ambiguous the same way.

### What each statement binds to

| Action | Binds to | Not found |
| --- | --- | --- |
| `OpenBranch` | the one `branch` row | `NoSuchBranch`, or `AmbiguousBranch` past one row |
| `OpenThreeWinding` | the one `transformer_3w` row | `NoSuchTransformer3w`, or `AmbiguousTransformer3w` past one row |
| `RemoveMachine`, `AddMachine` | the `generator` row | `NoSuchMachine` |
| `RemoveShunt` | the fixed `shunt` with that id, or every fixed shunt at the bus | `NoSuchShunt` |
| `RemoveSwitchedShunt` | every switched `shunt` at the bus | `NoSuchShunt` |
| `RemoveLoad` | the `load` with that id, or every load at the bus | `NoSuchLoad` |
| `DisconnectBus` | the `bus` row alone | `NoSuchBus` |
| `ChangeLoad`, `ChangeGeneration` | the `bus` row alone | `NoSuchBus` |
| `Unrecognized` | nothing | `Unrecognized` |

`DisconnectBus` binds to the bus and to nothing else: which elements at that
bus leave service depends on what the consumer models, so expanding the bus is
the consumer's work. `ChangeLoad` and `ChangeGeneration` bind to the bus for
the same reason, and the amount to move rides on the action rather than being
applied here.

A `ResolvedComponent` states the component type naming the table its `row`
indexes, the row's own identity, and the element's own in service flag as the
network states it now. A row carrying no `uid`, or one `ComponentId` does not
accept, states no identity, and still states its type and row: a caller that
needs persistent identities calls `assign_missing_component_ids` on the network
before building the index, which gives every row a `uid`. A bus is in service
when its type is anything other than isolated. An element already out of
service still binds, because outaging it changes nothing. A case with no
actions resolves to no components.

The `.con` grammar states `REMOVE SWSHUNT FROM BUS i` and carries no id, as the
RAW switched shunt record itself does not, so the statement addresses every
switched shunt at the bus.

Every reason states a snake_case `name`, for reports and bindings:
`no_such_bus`, `no_such_branch`, `ambiguous_branch`,
`ambiguous_transformer_3w`, `no_such_machine`, `no_such_shunt`, `no_such_load`,
`no_such_transformer_3w`, `unrecognized`.

Resolution reports rather than refuses. A case holding any unresolved action is
counted unresolved and earns one `BUILD.CON.CASE_UNRESOLVED` note naming the
case and its first unresolved action; the actions of that case that did bind
stay listed, so a caller can see how far the case got. The notes stop at the
reader's budget of 16: the first case past it records one
`BUILD.CON.NOTES_TRUNCATED` in place of its note and the cases after that
record nothing, so a set resolved against the wrong network cannot grow the
note list without limit. Every case is still counted.

## A later convergence point

PowerWorld states contingencies in its own `.aux` grammar, read by the
`Contingency` view in `format/powerworld/objects.rs`.
The two describe the same study object and are candidates to meet at one typed
model; they are separate today because neither reader resolves against a
network.
