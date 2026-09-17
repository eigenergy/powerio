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

## A later convergence point

PowerWorld states contingencies in its own `.aux` grammar, read by the
`Contingency` view in `format/powerworld/objects.rs`.
The two describe the same study object and are candidates to meet at one typed
model; they are separate today because neither reader resolves against a
network.
