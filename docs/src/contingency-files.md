# PSS/E contingency analysis files

A PSS/E contingency analysis reads three text files beside the case: a
contingency description file (`.con`) naming the outages to run, a subsystem
description file (`.sub`) naming the bus groups those outages draw on, and a
monitored element file (`.mon`) naming what the run reports on. Each is a
PowerIO value of its own, not a network: `ContingencySet`, `SubsystemSet`,
and `MonitoredSet`. Their PowerIO IR structural type names are in the
[PowerIO IR reference](ir-reference.md).

```text
CONTINGENCY 'L_000001ODES'
OPEN LINE FROM BUS   1001 TO BUS   1064 CIRCUIT 1
END
SINGLE BRANCH IN SUBSYSTEM 'WOA'
END
```

`parse` routes a file by its extension or by a declared token, `emit` writes
it back under that same token, and `serialize` carries it through PowerIO IR.
The tokens are `psse-con`, `psse-sub`, and `psse-mon`; `con`, `sub`, `mon`,
`contingency`, `subsystem`, and `monitored` are accepted aliases. No grid
exchange format states one of these files, so emitting a `.con` module as
`psse` or as `psse-sub` is refused by value type.

## Reading and binding

Reading holds what the file states and touches no network. Binding is the
separate step.

```rust,ignore
use powerio::{PioValue, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let case = parse("case.raw")?;
    let PioValue::BalancedNetwork(network) = case.value() else {
        panic!("a .raw parses to a balanced network");
    };

    let module = parse("cases.con")?;
    let PioValue::ContingencySet(set) = module.value() else {
        panic!("a .con parses to a contingency set");
    };

    let resolution = set.resolve(network);
    println!(
        "{} of {} cases bound",
        resolution.resolved,
        resolution.cases.len()
    );
    for note in resolution.diagnostics() {
        eprintln!("{}: {}", note.code(), note.message());
    }

    // An automatic specification expands into one case per element of a
    // subsystem, so the expanded set states every outage explicitly.
    let subsystems = parse("groups.sub")?;
    let PioValue::SubsystemSet(subsystems) = subsystems.value() else {
        panic!("a .sub parses to a subsystem set");
    };
    let expanded = set.expand(network, subsystems);
    print!("{}", expanded.set.to_con());
    Ok(())
}
```

`ContingencySet::resolve` returns one `ResolvedCase` per case: the elements
each action bound to, and the actions that bound to nothing with the reason
each one did not. `Subsystem::select_buses` names the buses of one subsystem,
and `MonitoredSet::resolve` binds monitored branches, interfaces, and voltage
scopes to table rows against a network and a subsystem set. The binding
recomputes the PSS/E machine and circuit identifier of every element with the
RAW writer's own allocation, because a network row's identity carries neither.

## What is kept as text

Each reader covers the statements its grammar states and keeps every other
line as the source wrote it, under `READ.CON.STATEMENT_UNRECOGNIZED`,
`READ.SUB.STATEMENT_UNRECOGNIZED`, or `READ.MON.STATEMENT_UNRECOGNIZED`. A
line kept inside a case becomes `ContingencyAction::Unrecognized`; a line kept
at file level becomes a `RetainedStatement` with the 1-based source line. A
tool that writes its own directives into a `.con` therefore reads completely
rather than failing on its first line, and `to_con` writes those lines back.

Only three shapes are errors: a case that never closes, a case that starts
inside another, and a block left open at end of input. The reader's note
budget is sixteen records per file; past it one further note reports the
suppression.

A case that does not apply to the network is a `BUILD.CON.CASE_UNRESOLVED`
note naming the case and the first action that bound to nothing, rather than a
silent omission. An automatic specification naming a subsystem the `.sub` file
does not state stays unexpanded and earns `BUILD.CON.SUBSYSTEM_UNKNOWN`.

## Emission

A parsed module emits its own file exactly: `emit` returns the retained source
bytes for a same format write. A module that carries no source, such as one
read back from PowerIO IR, emits canonical text through `to_con`, `to_sub`, or
`to_mon`. Reading canonical text back gives the same set, except that a
statement kept from the middle of a source file is written after the cases and
so reads back from a later line; writing that second set gives the same text.

## Command line

```console
$ powerio contingency resolve case.raw cases.con
$ powerio contingency resolve case.raw cases.con --sub groups.sub --mon watch.mon --json
$ powerio contingency expand case.raw cases.con --sub groups.sub -o expanded.con
```

`resolve` prints the case counts, one line per case that bound to nothing with
its reason, and, when `--mon` names a monitored element file, the monitored
row counts. `--sub` is read with `--mon`, whose statements name the subsystems
it states, and `resolve` refuses `--sub` without it. `--json` prints the same
counts as one object on stdout. `expand` writes the expanded `.con` text to
stdout, or to the file `-o` names.

## The grammar

The grammar is established from public contingency files and the example set
PSS/E ships, because the manual that defines it is licensed and not public.
`powerio-tx/src/contingency/FORMAT.md` lists each statement with its evidence
and gives the writer's spellings. The case names an expansion generates and
the reading of `3WLOWVOLTAGE` are PowerIO's convention, because PSS/E
documents neither publicly.
