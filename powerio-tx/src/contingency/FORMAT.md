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

This module reads all three, one reader per file, and expands a `.con` file's
automatic specifications against a network and a `.sub` file's subsystems. The
`.con` grammar comes first below, then `.sub`, then `.mon`, then the expansion
rules.

## Why the grammar below is stated from files

Siemens documents these files in the PSS/E Program Operation Manual, which
is licensed and not public. Every statement these readers accept was therefore
established from public contingency analysis files, from the example set PSS/E
ships, and from public question threads, and is listed with its evidence in the
fixture README. A statement outside the list keeps its line, with the
surrounding whitespace dropped, rather than failing the file, so a file a
reader has never seen still reads, and a new spelling shows up as a
`READ.CON.STATEMENT_UNRECOGNIZED`, `READ.SUB.STATEMENT_UNRECOGNIZED`, or
`READ.MON.STATEMENT_UNRECOGNIZED` note rather than as a silent loss.

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

## The subsystem description file

`sub.rs` reads `.sub` into `SubsystemSet` and writes it back with `to_sub`. A
subsystem names a set of buses; a `.con` automatic specification and a `.mon`
statement both name a subsystem stated here.

```text
/PSS(R)E 35
COM SUBSYSTEM description file entry created by PSS(R)E Config File Builder
SUBSYSTEM 'WOA'
   AREA 1
END
END
```

A file is optional header comments, then any number of subsystems, then one
file level `END`. `SUBSYSTEM name` opens one and `SYSTEM name` is its synonym;
the name is quoted or bare. Indentation is spaces or tabs and blank lines are
dropped. An `END` closes the innermost open block: the open `JOIN` group when
there is one, the subsystem otherwise, and the file after that. A missing file
`END` is tolerated and the writer adds one, a further bare `END` after it
states nothing, and any other statement after it is kept at file level, marked
`after_end`, and reported once as `READ.SUB.TEXT_AFTER_END`. The writer states
those statements after the `END` it writes, so a `SUBSYSTEM` line read after
the terminator is written after the terminator and reads back as text rather
than as a subsystem.

Two conditions refuse the file, each naming its 1-based line: a `SUBSYSTEM`
that starts before the previous one reached `END`, and a subsystem or a `JOIN`
group still open at end of input.

### Selectors

Each selector is one keyword and its values, and several may follow the
subsystem name on one line, in which case a trailing `END` there closes the
subsystem: `SUBSYSTEM CON AREA 5 ZONE 1 END` is one subsystem. A single valued
spelling reads as a range whose ends are equal, and both ends are inclusive.

| Statement | Reads as |
| --- | --- |
| `AREA n`, `AREAS a b` | `Area { from, to }` |
| `ZONE n`, `ZONES a b` | `Zone { from, to }` |
| `OWNER n`, `OWNERS a b` | `Owner { from, to }` |
| `BUS n`, `BUSES a b` | `Bus { from, to }` |
| `KVRANGE lo hi` | `KvRange { lo, hi }`, floats, inclusive on base kV |
| `JOIN [name]` ... `END` | one `SelectorGroup` whose `join` states the name |

`JOIN` opens a group closed by its own `END`, and selectors may follow the
keyword on that line as they may follow a subsystem name. The token after the
keyword is the group's name unless it opens a selector, so `JOIN AREA 1` opens
a group with no name over area 1. The rest of the line reads as selectors, and
a tail outside the grammar is reported and kept as text in the group the line
opened rather than dropped.

A group's `join` states how the file stated it: absent for the subsystem's
implicit group, which holds the selectors stated outside any `JOIN`,
`anonymous` for a `JOIN` with no name, and `named` for one with a name. Two
`JOIN` blocks with no name are therefore two groups whose bus sets union,
rather than one group whose selectors intersect.

A `KVRANGE` states its band low end first. A line whose first token is a
selector keyword but whose values are not the numbers it needs, a `KVRANGE`
whose ends run the other way round included, is reported as
`READ.SUB.SOURCE_MALFORMED`, a warning; any other line inside a subsystem is
reported as `READ.SUB.STATEMENT_UNRECOGNIZED`. Both keep their trimmed line
where the file stated it: on the `SelectorGroup` when a `JOIN` is open, and on
the subsystem otherwise. That is where the TARA-only statements land: `SCALE
ALL FOR EXPORT INCLUDE OFFLINE`, `PARTICIPATE`, `ADD ...`, `BASELOAD n`,
`TURBINETYPE n`, and `EXCEPT`. A `JOIN` line read while a `JOIN` is already
open opens no group and is kept in the open one the same way. A line at file
level outside any subsystem is kept on the set and reported the same way. The reader records at
most 16 notes and then one `READ.SUB.NOTES_TRUNCATED`.

### Which buses a subsystem names

`Subsystem::select_buses` takes a `BalancedNetwork` and returns the bus ids.
Within one group, the buses matching each selector family present are unioned
within the family and intersected across the families; the groups of a
subsystem are unioned. A group with no selector, and a subsystem with no group,
names no bus.

`OWNER` reads the bus's `extras["psse_owner"]`, and an absent property means
owner 1, because the RAW reader keeps that field only when it differs from the
default. `KVRANGE` reads `Bus::base_kv`.

The bare body rule is the same as a single `JOIN` group. PSS/E's own rule for a
body that mixes bare selectors with `JOIN` groups is not publicly documented;
this reader unions the implicit group with the named ones, which is what
`pssetools` and the 3phaseee notes describe for named groups.

### What the subsystem writer states

`to_sub` writes the header lines as written, then each subsystem, then the file
level statements kept as text, then a final `END`.

| Value | Written as |
| --- | --- |
| subsystem | `SUBSYSTEM 'name'`, its groups, its kept lines, `END` |
| implicit group | its selectors, one per line, indented three spaces |
| named `JOIN` group | `   JOIN 'name'`, its selectors, its kept lines, `   END` |
| `JOIN` group with no name | `   JOIN`, its selectors, its kept lines, `   END` |
| `Area` | `   AREA {n}`, or `   AREAS {a} {b}` when the ends differ |
| `Zone`, `Owner` | `ZONE`/`ZONES`, `OWNER`/`OWNERS`, the same way |
| `Bus` | `   BUS {n}`, or `   BUSES {a} {b}` |
| `KvRange` | `   KVRANGE {lo} {hi}` |

A float is written in its `Display` form, with `.0` added when that form states
no decimal point, so `KVRANGE 69.0 999.0` reads back as the same two `f64`
values. The implicit group is written first and the `JOIN` groups after it, in
the order they were read, so reading the written file gives the same set. Each
kept line is written where it was read, inside its group or its subsystem, so
it reads back into the same place; the file level statements read after the
file `END` are written after the `END` the writer states.

## The monitored element file

`mon.rs` reads `.mon` into `MonitoredSet` and writes it back with `to_mon`.

```text
/PSS(R)E 34
COM MONITORED element file entry created by PSS(R)E Config File Builder
MONITOR VOLTAGE RANGE SUBSYSTEM 'ILLINOIS200' 0.950 1.050
MONITOR BRANCHES IN SUBSYSTEM 'ILLINOIS200'
MONITOR TIES FROM SUBSYSTEM 'ILLINOIS200'
END
```

| Statement | Synonyms | Reads as |
| --- | --- | --- |
| `MONITOR BRANCHES IN SUBSYSTEM name [3WLOWVOLTAGE]` | `BRANCHES`, `LINES`; `IN`, `FROM` | `BranchesInSubsystem { subsystem, low_voltage_3w }` |
| `MONITOR TIES FROM SUBSYSTEM name` | `IN`, `FROM` | `TiesFromSubsystem { subsystem }` |
| `MONITOR BRANCHES` ... `END`, holding `i j [ckt]` | `BRANCHES`, `LINES` | `Branches(Vec<BranchRef>)` |
| `MONITOR INTERFACE name [RATING x [MW]]` ... `END` | | `Interface { name, rating_mw, branches }` |
| `MONITOR VOLTAGE RANGE scope lo hi` | | `VoltageRange { scope, vmin, vmax }` |
| `MONITOR VOLTAGE DEVIATION scope down [up]` | | `VoltageDeviation { scope, down, up }` |

A `MONITOR VOLTAGE RANGE` states its band low end first; one whose ends run
the other way round keeps its line as text. Every limit a statement states is
finite, so a value that is not keeps its line the same way.

A `MONITOR BRANCHES` line with nothing after it opens a block of branch lines
that runs to the next `END`, and `MONITOR INTERFACE` always opens one. Inside a
block, `i j` names circuit `1` and `i j ckt` names that circuit. A scope is one
of `ALL BUSES`, `SUBSYSTEM name`, `BUS n`, `AREA n`, `ZONE n`, `OWNER n`, and
`KV x`.

Files carry one or two file level `END`s and both read the same: the first ends
the file, a further bare `END` states nothing, and any other statement after it
is kept at file level, marked `after_end`, and reported once as
`READ.MON.TEXT_AFTER_END`. The writer states those statements after the `END`
it writes, so a `MONITOR` line read after the terminator is written after the
terminator and reads back as text rather than as a statement. A line inside a
block that states no branch is reported as `READ.MON.SOURCE_MALFORMED`, a
warning, any other line outside the grammar as
`READ.MON.STATEMENT_UNRECOGNIZED`, and both keep their trimmed line where the
file stated it: on the `Branches` or `Interface` statement when a block is
open, and on the set otherwise. A `MONITOR` line inside a block opens no
statement, because the block runs to its own `END`, so it is kept in the block
the same way. The reader records at most 16 notes and then one
`READ.MON.NOTES_TRUNCATED`. A block still open at end of input refuses the
file, naming the line that opened it.

`to_mon` writes the header lines as written, the statements in order, the lines
kept from before the file `END`, a final `END`, and then the lines read after
that `END`. The spellings are the ones in the table above, with
`SUBSYSTEM 'name'` and `INTERFACE 'name'` quoted, a rating written
`RATING {x} MW`, floats written as in `to_sub`, and a block's branches written
one per line as `{i:>6} {j:>6} {ckt}`, then the block's kept lines, before its
`END`.

### Binding a monitored set to a network

`MonitoredSet::resolve` takes the network and a `SubsystemSet` and returns
`MonitoredResolution`, whose rows are positions in the network's tables. A
caller binding several files to one network builds one `PsseEquipmentIndex` and
calls `MonitoredSet::resolve_with` and `ContingencySet::expand_with`, which read
the network the index borrows.

| Statement | Binds to |
| --- | --- |
| `BranchesInSubsystem` | `branch_rows`: every branch with both terminals in the subsystem. With `3WLOWVOLTAGE`, `transformer_3w_rows` gains every three winding transformer whose lowest voltage winding sits there |
| `TiesFromSubsystem` | `tie_rows`: every branch with exactly one terminal in the subsystem |
| `Branches` | `branch_rows`, one per listed branch |
| `Interface` | one `ResolvedInterface`, its members in statement order |
| `VoltageRange`, `VoltageDeviation` | one `ResolvedVoltageScope`, the scope's bus rows with the limits |

An interface member is a row and the orientation the statement stated it in. A
branch's flow runs from its stored `from` terminal to its stored `to` terminal,
so a member the statement named the other way round carries `reversed` and
enters the interface sum with the opposite sign.

A listed branch binds through `PsseEquipmentIndex::branch_rows`, so it matches
in either terminal order; zero rows is `NoSuchBranch` and more than one is
`AmbiguousBranch`, and either keeps the statement in `unresolved`. A statement
naming a subsystem the set does not state is `NoSuchSubsystem`. Each entry of
`unresolved` earns one `BUILD.MON.STATEMENT_UNRESOLVED` note.

A scope naming an area, zone, owner, bus, or kV level the network does not hold
names no row and is not counted unresolved, because the statement is
well formed and the network simply holds nothing there. `Kv x` matches a base
kV within 1e-6. Service state does not enter: a monitored element is reported
on whether or not the network states it in service, because monitoring reads a
result rather than changing the case.

## Automatic expansion

`ContingencySet::expand` takes a network and a `SubsystemSet` and turns each
`AutomaticSpec` into explicit cases. The expanded set keeps the header and the
statements kept as text, states the explicit cases first and the generated
cases after them, and holds no specification that expanded.

| Specification | Expands to one case per |
| --- | --- |
| `SINGLE BRANCH IN SUBSYSTEM s` | in service branch with both terminals in `s` |
| `SINGLE BRANCH IN SUBSYSTEM s 3WLOWVOLTAGE` | the above, plus each in service three winding transformer whose lowest voltage winding bus is in `s` |
| `SINGLE UNIT IN SUBSYSTEM s` | in service generator at a bus in `s` |
| `SINGLE TIE FROM SUBSYSTEM s` | in service branch with exactly one terminal in `s` |
| `DOUBLE ...` | unordered pair of the corresponding single cases, actions concatenated |

Cases keep table order. A branch a `SkipRule` names, in either terminal order
and on the same circuit, produces no case. Only elements the network states in
service expand, because outaging one already out of service changes nothing.
Circuit and machine ids come from `PsseEquipmentIndex`, so a generated case
names its element the way a RAW file written from this network would.

A specification naming a subsystem the set does not state stays in `automatic`
and earns one `BUILD.CON.SUBSYSTEM_UNKNOWN` note. A specification whose
subsystem is stated but holds fewer in service elements of its target family
than its order needs, one for `SINGLE` and two for `DOUBLE`, expands into no
case and earns one `BUILD.CON.SPECIFICATION_EMPTY` note whose message states
how many elements were eligible and how many the order needs. A `DOUBLE`
specification needs two, because its cases are the unordered pairs of the
eligible elements. Those two findings are the whole of what an expansion notes.

The `SKIP` rules are the expansion's own input, so they are dropped only once
every specification that could read them has expanded: a set that states rules
and no specification at all keeps them.

### Names the expansion gives its cases

PSS/E's own generated case names are not publicly documented. These are
PowerIO's convention, following the spelling the GO Competition's `case14.con`
uses:

| Case | Named |
| --- | --- |
| branch or tie | `L_{from}_{to}_{ckt}` |
| machine | `G_{bus}_{id}` |
| three winding transformer | `T_{a}_{b}_{c}_{ckt}`, the buses in winding order |
| double | `{first}+{second}` |

What `3WLOWVOLTAGE` selects is likewise not publicly documented. This reading
is PowerIO's: the transformer joins the expansion when the bus of its lowest
voltage winding is in the subsystem. A winding stating no nominal kV defers to
its terminal bus base kV, the same rule the RAW reader states, and a tie
between two windings takes the earlier one. The flag has no effect on a `TIE`
specification.

## A later convergence point

PowerWorld states contingencies in its own `.aux` grammar, read by the
`Contingency` view in `format/powerworld/objects.rs`.
The two describe the same study object and are candidates to meet at one typed
model; they are separate today because neither reader resolves against a
network.
