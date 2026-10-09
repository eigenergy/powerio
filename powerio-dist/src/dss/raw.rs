//! Script execution and the raw object layer.
//!
//! A `.dss` file is a command script. This layer splits it into command
//! lines (handling block comments), resolves command verbs with the same
//! exact-then-prefix rule OpenDSS uses, follows `Redirect`/`Compile`
//! includes, and accumulates `New`/`Edit`/`~` property assignments into raw
//! objects with property names resolved against the class tables. Values
//! stay untyped [`Value`] tokens; interpretation happens in the readers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::lex::{Scanner, Value, VarMap};
use super::prop::{self, DssClass};
use crate::diagnostics::codes as C;
use crate::error::{Error, Result};

/// The OpenDSS executive command list, in definition order
/// (Executive/ExecCommands.cpp). Order fixes abbreviation resolution: a verb
/// matches exactly first, then the first command here with the verb as a
/// prefix. Only a handful execute in this layer; the rest are preserved as
/// [`RawCommand`]s.
static COMMANDS: &[&str] = &[
    "new",
    "edit",
    "more",
    "m",
    "~",
    "select",
    "save",
    "show",
    "solve",
    "enable",
    "disable",
    "plot",
    "reset",
    "compile",
    "set",
    "dump",
    "open",
    "close",
    "//",
    "redirect",
    "help",
    "quit",
    "?",
    "next",
    "panel",
    "sample",
    "clear",
    "about",
    "calcvoltagebases",
    "setkvbase",
    "buildy",
    "get",
    "init",
    "export",
    "fileedit",
    "voltages",
    "currents",
    "powers",
    "seqvoltages",
    "seqcurrents",
    "seqpowers",
    "losses",
    "phaselosses",
    "cktlosses",
    "allocateloads",
    "formedit",
    "totals",
    "capacity",
    "classes",
    "userclasses",
    "zsc",
    "zsc10",
    "zscrefresh",
    "ysc",
    "puvoltages",
    "varvalues",
    "varnames",
    "buscoords",
    "makebuslist",
    "makeposseq",
    "reduce",
    "interpolate",
    "alignfile",
    "top",
    "rotate",
    "vdiff",
    "summary",
    "distribute",
    "di_plot",
    "comparecases",
    "yearlycurves",
    "cd",
    "visualize",
    "closedi",
    "doscmd",
    "estimate",
    "reconductor",
    "_initsnap",
    "_solvenocontrol",
    "_samplecontrols",
    "_docontrolactions",
    "_showcontrolqueue",
    "_solvedirect",
    "_solvepflow",
    "addbusmarker",
    "uuids",
    "setloadandgenkv",
    "cvrtloadshapes",
    "nodediff",
    "rephase",
    "setbusxy",
    "updatestorage",
    "obfuscate",
    "latlongcoords",
    "batchedit",
    "pstcalc",
    "variable",
    "reprocessbuses",
    "clearbusmarkers",
    "relcalc",
    "var",
    "cleanup",
    "finishtimestep",
    "nodelist",
    "newactor",
    "clearall",
    "wait",
    "solveall",
    "calcincmatrix",
    "calcincmatrix_o",
    "tear_circuit",
    "connect",
    "disconnect",
    "refine_buslevels",
    "remove",
    "abort",
    "calclaplacian",
    "clone",
    "fncspublish",
    "exportoverloads",
    "exportvviolations",
    "zsc012",
    "aggregateprofiles",
    "allpceatbus",
    "allpdeatbus",
    "totalpowers",
    "comhelp",
    "gis",
    "giscoords",
    "readefieldhdf",
];

fn command_index(verb: &str) -> Option<usize> {
    let v = verb.to_ascii_lowercase();
    COMMANDS
        .iter()
        .position(|c| *c == v)
        .or_else(|| COMMANDS.iter().position(|c| c.starts_with(&v)))
}

/// One property assignment as applied to an object, in application order.
#[derive(Clone, Debug, PartialEq)]
pub struct RawProp {
    /// Canonical property name when resolved against the class table;
    /// the name as written when the class or property is unknown; `None`
    /// for a positional value on an unknown class.
    pub name: Option<String>,
    pub value: Value,
}

/// An accumulated object: every `New`/`Edit`/`~`/`like` assignment that
/// touched it, in order. Values are raw tokens.
#[derive(Clone, Debug)]
pub struct RawObject {
    /// Canonical lowercase class name (`line`, `load`, ...), known or not.
    pub class: String,
    /// Object name as written; lookup is case insensitive.
    pub name: String,
    pub props: Vec<RawProp>,
    /// Prop-count checkpoints at edit boundaries. Every object command line
    /// (`New`/`Edit`/`~`/`More`/property reference) is one engine Edit, and
    /// the class Edit ends in RecalcElementData; readers with end-of-edit
    /// side effects (Load) segment `props` on these. `like=` splices the
    /// source's checkpoints too: MakeLike copies the source's recalced
    /// state, so its boundaries must replay.
    pub edits: Vec<usize>,
}

impl RawObject {
    /// The last assignment to a canonical property name, if any.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.props
            .iter()
            .rev()
            .find(|p| p.name.as_deref() == Some(name))
            .map(|p| &p.value)
    }

    /// Edit boundary checkpoints, closed over the full prop list: a
    /// trailing segment without a recorded boundary counts as one more
    /// edit, so callers always see `props.len()` last.
    pub fn edit_bounds(&self) -> impl Iterator<Item = usize> + '_ {
        let tail =
            (self.edits.last().copied() != Some(self.props.len())).then_some(self.props.len());
        self.edits.iter().copied().chain(tail)
    }
}

/// A command this layer does not execute, preserved verbatim.
#[derive(Clone, Debug, PartialEq)]
pub struct RawCommand {
    /// Canonical verb when recognized, the first token as written otherwise.
    pub verb: String,
    /// Everything after the verb, trimmed.
    pub args: String,
}

/// Bus coordinates from a `BusCoords` file.
#[derive(Clone, Debug, PartialEq)]
pub struct BusCoord {
    pub bus: String,
    pub x: f64,
    pub y: f64,
}

/// The executed script: objects, options, and preserved commands.
#[derive(Debug, Default)]
pub struct RawDss {
    pub circuit_name: Option<String>,
    pub objects: Vec<RawObject>,
    /// `Set option=value` assignments in order.
    pub options: Vec<(String, Value)>,
    /// Commands preserved without execution (solve, calcvoltagebases, ...).
    pub commands: Vec<RawCommand>,
    pub buscoords: Vec<BusCoord>,
    pub vars: VarMap,
    /// Bumped by `clear`, so `run_command` can tell whether a `Clear` ran
    /// during the command it just dispatched and, if so, skip restoring the
    /// pre-command var snapshot over the empty table `clear` installed.
    vars_generation: u64,
    /// The reader's findings as `CODE: message` lines, rendered from
    /// `diagnostics` so the text and the structure cannot disagree.
    pub warnings: Vec<String>,
    /// Structured findings beside `warnings`; an `Error` entry marks an
    /// incomplete parse the CLI must not exit 0 on.
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
    index: BTreeMap<(String, String), usize>,
    active: Option<usize>,
    /// Properties, source resets, line copy/edit/calculation history and wildcard work,
    /// charged against `MAX_TOTAL_PROPS`.
    total_props: usize,
    // Source MakeLike copies resolved state, including flags/defaults that
    // an appended property history alone cannot restore. Reset markers refer
    // to offsets in the existing flattened history; no extra history tree.
    source_replay: BTreeMap<usize, SourceReplay>,
    pub(super) line_history: BTreeMap<usize, super::line_history::LineHistory>,
    pub(super) geometry_timeline: Vec<super::geometry::timeline::Action>,
    earth_model: Option<String>,
    pub(super) geometry_earth: Option<std::result::Result<super::geometry::EarthModel, String>>,
    frequency: super::frequency::FrequencyState,
    solution_mode: super::solution_mode::SolutionMode,
    /// Diagnostic records retained, against `MAX_TOTAL_DIAGNOSTICS`. Survives
    /// `clear` with the channel it counts.
    total_diagnostics: usize,
    /// Preserved records retained (`commands`, `options`, `buscoords`),
    /// against `MAX_TOTAL_PRESERVED`. Reset by `clear` with the collections
    /// it counts.
    total_preserved: usize,
    /// One of the whole parse ceilings was crossed: the terminal refusal is
    /// recorded and no further script text executes.
    halted: bool,
}

#[derive(Clone, Debug)]
pub(super) struct SourceReplay {
    pub constructor_frequency: f64,
    pub resets: Vec<(usize, f64)>,
    // MakeLike re-enables the target independently of the copied property
    // history. Wildcard Enable/Disable changes this without an engine Edit.
    pub enabled: bool,
    // Native object references bind during assignment, not after all objects
    // have been declared. A later overwrite cannot erase an earlier failure.
    pub invalid_reference: Option<String>,
}

impl SourceReplay {
    fn record_property(&mut self, p: &RawProp) {
        if p.name.as_deref() == Some("enabled") && !p.value.text.is_empty() {
            // Native InterpretYesNo recognizes leading y/t only; numeric 1
            // is false, unlike the legacy general Boolean helper.
            self.enabled = matches!(p.value.text.as_bytes()[0].to_ascii_lowercase(), b'y' | b't');
        }
    }
}

impl RawDss {
    pub(super) fn line_snapshot(&self, object: usize) -> super::line_history::Snapshot {
        super::line_history::Snapshot {
            object,
            properties: self.objects[object].props.len(),
            edits: self.objects[object].edits.len(),
            copies: self.line_history[&object].copies.len(),
            events: self.line_history[&object].events.len(),
        }
    }

    fn option_assignment(&mut self, position: &mut usize, name: &str, value: &Value) {
        self.frequency.assignment(position, name, value, &self.vars);
        if *position == 81 {
            self.earth_model = Some(value.text.clone());
        }
        if *position == 8 {
            self.solution_mode.assign(&value.text);
        }
    }

    pub(super) fn solution_mode(&self) -> &'static str {
        self.solution_mode.name()
    }

    pub(super) fn unresolved_mode_transition(&self) -> Option<&'static str> {
        self.solution_mode.unresolved_transition()
    }

    pub(super) fn snapshot_frequency(&self) -> f64 {
        self.frequency.snapshot()
    }
    pub(super) fn source_replay(&self, object: &RawObject) -> &SourceReplay {
        let index = self.index[&(object.class.clone(), object.name.to_ascii_lowercase())];
        &self.source_replay[&index]
    }
    pub fn find(&self, class: &str, name: &str) -> Option<&RawObject> {
        self.index
            .get(&(class.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .map(|&i| &self.objects[i])
    }

    pub fn of_class<'a>(&'a self, class: &'a str) -> impl Iterator<Item = &'a RawObject> {
        self.objects.iter().filter(move |o| o.class == class)
    }

    fn warn(&mut self, info: &'static crate::diagnostics::DiagnosticInfo, msg: impl Into<String>) {
        self.record(crate::diagnostics::Diagnostic::of(info, msg));
    }

    /// Record one finding. Both channels move together: the line is rendered
    /// from the record it is added with. At the whole parse ceiling one
    /// terminal refusal is recorded and the parse halts.
    fn record(&mut self, diagnostic: crate::diagnostics::Diagnostic) {
        if self.total_diagnostics >= MAX_TOTAL_DIAGNOSTICS {
            self.halt(format!(
                "the script's retained findings reached the whole parse ceiling of \
                 {MAX_TOTAL_DIAGNOSTICS}; execution stopped"
            ));
            return;
        }
        self.total_diagnostics += 1;
        self.warnings
            .push(crate::diagnostics::render_diagnostic(&diagnostic));
        self.diagnostics.push(diagnostic);
    }

    /// The one halt step: set the flag and emit the single terminal refusal
    /// into both channels directly, indivisibly, whichever ceiling fired
    /// first and whatever the other counter holds.
    fn halt(&mut self, message: String) {
        if self.halted {
            return;
        }
        self.halted = true;
        let refusal = crate::diagnostics::Diagnostic::of(
            &crate::diagnostics::codes::READ_DSS_INCLUDE_BUDGET,
            message,
        );
        self.warnings
            .push(crate::diagnostics::render_diagnostic(&refusal));
        self.diagnostics.push(refusal);
    }

    /// Charge one preserved record (`commands`, `options`, `buscoords`);
    /// false once the ceiling is crossed, after which the terminal refusal
    /// is recorded and the parse halts.
    fn charge_preserved(&mut self) -> bool {
        if self.total_preserved >= MAX_TOTAL_PRESERVED {
            self.halt(format!(
                "the script's preserved records reached the whole parse ceiling of \
                 {MAX_TOTAL_PRESERVED}; execution stopped"
            ));
            return false;
        }
        self.total_preserved += 1;
        true
    }

    fn clear(&mut self) {
        let geometry_earth = Some(super::geometry::timeline::earth_before_clear(self));
        // `Clear` resets the circuit, not the record of how the script was
        // read. A warning or an `Error` finding already emitted describes input
        // the caller wrote, and no later command un-writes it — dropping them
        // here returned a network that had refused an include, or skipped one
        // that escaped the case directory, with an empty `warnings` and nothing
        // for `powerio module` to lift into a finding.
        let (warnings, diagnostics) = (
            std::mem::take(&mut self.warnings),
            std::mem::take(&mut self.diagnostics),
        );
        let (total_diagnostics, halted) = (self.total_diagnostics, self.halted);
        let vars_generation = self.vars_generation.wrapping_add(1);
        let mut frequency = std::mem::take(&mut self.frequency);
        frequency.clear();
        let earth_model = self.earth_model.take();
        *self = RawDss {
            vars_generation,
            warnings,
            diagnostics,
            earth_model,
            geometry_earth,
            frequency,
            total_diagnostics,
            halted,
            ..RawDss::default()
        };
    }
}

/// Supplies included file text, so tests can run without a filesystem.
pub trait Loader {
    fn load(&mut self, path: &Path) -> std::io::Result<String>;
}

impl<F> Loader for F
where
    F: FnMut(&Path) -> std::io::Result<String>,
{
    fn load(&mut self, path: &Path) -> std::io::Result<String> {
        self(path)
    }
}

/// Redirect nesting limit; OpenDSS recurses unbounded, this bounds cycles.
const MAX_REDIRECT_DEPTH: usize = 64;

/// Includes a single parse may follow. Depth bounds one branch, not the work:
/// a file that redirects to itself twice expands into a binary tree of depth
/// [`MAX_REDIRECT_DEPTH`], so 34 bytes never finish parsing. The largest
/// fixture here (IEEE123Master.dss) follows 3.
const MAX_TOTAL_INCLUDES: usize = 4096;

/// Diagnostic records one parse retains, `warnings` and `diagnostics`
/// together (they move in step). A re-executed include repeats its findings;
/// the ceiling bounds what the whole parse may keep, independent of how many
/// times a file re-executes. The largest legitimate feeders stay far under
/// it.
const MAX_TOTAL_DIAGNOSTICS: usize = 1 << 18;

/// Preserved records one parse retains across `commands`, `options`, and
/// `buscoords` together, on the same rule as the diagnostic ceiling.
const MAX_TOTAL_PRESERVED: usize = 1 << 20;

/// Script text a single parse may pull in through includes. The include count
/// alone still admits amplification: one large file that redirects to itself is
/// re-executed once per load, so a few megabytes of input buy thousands of
/// times that in scanning. Together the two budgets keep an include tree
/// costing about what a single case file of this size costs. The root file is
/// not charged against it, so a case without includes is never truncated.
const MAX_TOTAL_INCLUDE_BYTES: usize = 64 << 20;

/// Parser variables (`var @name=value`) a single parse may define, on the
/// same amplification concern as includes: a script that redirects into
/// itself while defining one each time would otherwise grow the table
/// without bound. The largest legitimate feeders define a handful.
const MAX_TOTAL_VARS: usize = 4096;

/// Bytes stored across every var name and value together, on the same rule
/// as the include byte budget.
const MAX_TOTAL_VAR_BYTES: usize = 1 << 20;

/// Cap on the accumulated property assignments a single object may hold.
/// `like=` splices the source object's whole prop list, so a self-referencing
/// or mutually-referencing chain (`Edit Load.a like=a` repeated) doubles the
/// count each edit — a few hundred bytes could otherwise reach memory
/// exhaustion. No real object comes near this bound: the largest DSS class has
/// on the order of 100 properties and legitimate scripts edit an object a
/// handful of times.
const MAX_OBJECT_PROPS: usize = 1 << 16;

/// Properties retained across every object of one parse. A short script can
/// fill one object to its cap and then `like=` splice it into many new
/// objects; the whole-parse budget bounds what all of them together may
/// retain, refusing further splices and assignments through the clamp
/// diagnostic. The largest legitimate feeders stay far under it.
const MAX_TOTAL_PROPS: usize = 1 << 22;

/// Which directory the include containment confines to, for diagnostic
/// wording: the refusal names the boundary actually in force.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum IncludeBoundary {
    CaseDirectory,
    IncludeRoot,
}

impl IncludeBoundary {
    fn noun(self) -> &'static str {
        match self {
            Self::CaseDirectory => "the case directory",
            Self::IncludeRoot => "the include root",
        }
    }
}

struct Executor<'l, L: Loader> {
    raw: RawDss,
    loader: &'l mut L,
    /// Directory stack for relative include resolution; starts with the
    /// root file's directory, so its depth is the redirect nesting level.
    dirs: Vec<PathBuf>,
    /// When set (file parsing), `Redirect`/`Compile`/`Buscoords` includes are
    /// confined to this directory subtree, so an untrusted case file cannot
    /// read outside its own directory. `None` leaves includes unconfined, for
    /// the in-memory loaders used by tests and string parsing (which installs
    /// a loader that reads nothing).
    root: Option<PathBuf>,
    /// Names the confinement boundary in refusal messages: the case directory
    /// by default, the caller's include root when one was widened onto `root`.
    boundary: IncludeBoundary,
    /// Includes followed and script bytes they pulled in, against
    /// [`MAX_TOTAL_INCLUDES`] and [`MAX_TOTAL_INCLUDE_BYTES`]. `budget_spent`
    /// keeps the refusal to one message however many includes follow it.
    includes: usize,
    include_bytes: usize,
    budget_spent: bool,
    /// Vars defined and bytes of their names and values stored, against
    /// [`MAX_TOTAL_VARS`] and [`MAX_TOTAL_VAR_BYTES`]. Lives here rather than
    /// on `RawDss`, which `Clear` resets, so a budget already spent stays
    /// spent across a `Clear`, on the same rule as the include budget.
    var_count: usize,
    var_bytes: usize,
    var_budget_spent: bool,
}

/// Collapses `.` and `..` lexically, without touching the filesystem. A
/// leading `..` is preserved so a path that climbs above its base fails the
/// containment check rather than silently resolving somewhere inside it.
fn lexical_normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out: Vec<Component<'_>> = Vec::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir if matches!(out.last(), Some(Component::Normal(_))) => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out.into_iter().collect()
}

/// Splits script text into command lines, dropping block comments. A block
/// comment starts when the first nonspace characters are `/*` and ends on the
/// first line containing `*/`; both boundary lines are consumed whole,
/// matching the OpenDSS executive.
fn command_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut in_block = false;
    text.lines().enumerate().filter_map(move |(i, line)| {
        if in_block {
            if line.contains("*/") {
                in_block = false;
            }
            return None;
        }
        if line.trim_start().starts_with("/*") {
            in_block = true;
            if line.contains("*/") {
                in_block = false;
            }
            return None;
        }
        Some((i + 1, line))
    })
}

impl<L: Loader> Executor<'_, L> {
    fn run_script(&mut self, text: &str, file: &str) {
        for (line_no, line) in command_lines(text) {
            if self.raw.halted {
                return;
            }
            self.run_command(line, file, line_no);
        }
    }

    fn run_command(&mut self, line: &str, file: &str, line_no: usize) {
        // The scanner substitutes against a snapshot of the var table so the
        // live table stays free for mutation: `var` inserts into it directly
        // and redirected files both see and extend it. The snapshot only
        // diverges for a self referencing `var` line, which OpenDSS scripts
        // do not write. Taking the table rather than cloning it avoids
        // copying it on every command; `do_var` inserts land in the now
        // empty live table and are merged back onto the snapshot afterward.
        // A `Clear` during dispatch replaces the table outright, so the
        // restore is skipped when `vars_generation` shows one ran.
        let vars = std::mem::take(&mut self.raw.vars);
        let generation = self.raw.vars_generation;
        let mut scan = Scanner::new(line, Some(&vars));
        let ctx = |msg: String| format!("{file}:{line_no}: {msg}");
        match scan.next_param() {
            None => {}
            Some(first) if first.value.text.is_empty() && first.name.is_none() => {}
            Some(first) => {
                if let Some(name) = first.name {
                    // First parameter is name=value: a property reference
                    // like `Transformer.Reg1.Taps=[...]`.
                    self.edit_property_reference(&name, first.value, &mut scan, &ctx);
                } else {
                    self.dispatch(first.value.text, &mut scan, &ctx);
                }
            }
        }
        if self.raw.vars_generation == generation {
            let added = std::mem::replace(&mut self.raw.vars, vars);
            self.raw.vars.extend(added);
        }
    }

    fn dispatch(&mut self, verb: String, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        match command_index(&verb).map(|i| COMMANDS[i]) {
            Some("new") => self.do_new(scan, ctx),
            Some("edit") => self.do_edit(scan, ctx),
            Some("batchedit") => self.do_batch_edit(scan, ctx),
            Some("more" | "m" | "~") => self.do_more(scan, ctx),
            Some("select") => self.do_select(scan, ctx),
            Some(canonical @ ("open" | "close")) => self.do_terminal_state(canonical, scan, ctx),
            Some("set") => self.do_set(scan),
            Some("redirect") => self.do_redirect(scan, false, ctx),
            Some("compile") => self.do_redirect(scan, true, ctx),
            Some("buscoords") => self.do_buscoords(scan, ctx),
            Some("setbusxy") => self.do_setbusxy(scan, ctx),
            Some("var") => self.do_var(scan, ctx),
            Some("clear" | "clearall") => self.raw.clear(),
            Some("enable" | "disable") => self.do_enabled(&verb, scan, ctx),
            Some("//") => {}
            Some(canonical) => {
                if self.raw.charge_preserved() {
                    let args = scan.remainder().to_string();
                    if canonical == "solve" {
                        let mut position = 0;
                        while let Some(p) = scan.next_param() {
                            if p.value.text.is_empty() {
                                break;
                            }
                            self.raw.option_assignment(
                                &mut position,
                                &p.name.unwrap_or_default().to_ascii_lowercase(),
                                &p.value,
                            );
                        }
                    }
                    if matches!(
                        canonical,
                        "solve"
                            | "_solvenocontrol"
                            | "_solvedirect"
                            | "_solvepflow"
                            | "calcvoltagebases"
                            | "buildy"
                    ) {
                        self.record_line_calculation(canonical == "buildy", ctx);
                    }
                    self.raw.commands.push(RawCommand {
                        verb: canonical.to_string(),
                        args,
                    });
                }
            }
            None => {
                self.raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!("unknown command `{verb}`; line preserved verbatim")),
                );
                if self.raw.charge_preserved() {
                    self.raw.commands.push(RawCommand {
                        verb,
                        args: scan.remainder().to_string(),
                    });
                }
            }
        }
    }

    /// `var @name=value ...` defines parser variables. TParserVar::Add
    /// stores every value brace wrapped unless it begins with `@`;
    /// CheckforVar unwraps the braces into a quoted token, so a definition
    /// like `var @z=(8 1000 /)` still evaluates as RPN where it is used.
    fn do_var(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        while let Some(p) = scan.next_param() {
            if p.value.text.is_empty() && p.name.is_none() {
                break;
            }
            if let Some(name) = p.name {
                let stored = if p.value.text.starts_with('@') {
                    p.value.text
                } else {
                    format!("{{{}}}", p.value.text)
                };
                let key = name.to_ascii_lowercase();
                if self.charge_var(&key, &stored, ctx) {
                    self.raw.vars.insert(key, stored);
                }
            }
        }
    }

    /// Charges one var definition against the budgets, returning whether to
    /// store it. Charged at the attempt, so a definition right at either cap
    /// is refused before insertion, on the same rule as [`Self::charge_include`].
    fn charge_var(&mut self, name: &str, stored: &str, ctx: &dyn Fn(String) -> String) -> bool {
        let bytes = name.len() + stored.len();
        let over_budget = self.var_budget_spent
            || self.var_count >= MAX_TOTAL_VARS
            || self.var_bytes.saturating_add(bytes) > MAX_TOTAL_VAR_BYTES;
        if !over_budget {
            self.var_count += 1;
            self.var_bytes += bytes;
            return true;
        }
        if !self.var_budget_spent {
            self.var_budget_spent = true;
            let message = ctx(format!(
                "var {name}: refused; the case exceeded the variable budget of {MAX_TOTAL_VARS} \
                 entries and {} KiB, so further variables were not stored",
                MAX_TOTAL_VAR_BYTES >> 10,
            ));
            self.raw
                .warn(&crate::diagnostics::codes::READ_DSS_VALUE_CLAMPED, message);
        }
        false
    }

    /// A leading `name=value` parameter is a property reference
    /// (ExecCommands ProcessCommand): `Class.Name.Prop=value`,
    /// `Name.Prop=value` with the class omitted, or `Prop=value` on the
    /// active object. ParseObjName cuts the object part at the second dot;
    /// SetObject resolves an omitted class to the last referenced one,
    /// which here is the active object's class.
    fn edit_property_reference(
        &mut self,
        spec: &str,
        value: Value,
        scan: &mut Scanner,
        ctx: &dyn Fn(String) -> String,
    ) {
        let (object, prop) = match spec.split_once('.') {
            None => (None, spec),
            Some((first, rest)) => match rest.split_once('.') {
                None => (Some((None, first)), rest),
                Some((name, prop)) => (Some((Some(first), name)), prop),
            },
        };
        let active_or = |raw: &mut RawDss| {
            let active = raw.active;
            if active.is_none() {
                raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!("`{spec}=` with no active object")),
                );
            }
            active
        };
        let idx = match object {
            None => match active_or(&mut self.raw) {
                Some(idx) => idx,
                None => return,
            },
            Some((class, name)) => {
                let class = match class {
                    Some(c) => c.to_ascii_lowercase(),
                    None => match active_or(&mut self.raw) {
                        Some(idx) => self.raw.objects[idx].class.clone(),
                        None => return,
                    },
                };
                if let Some(idx) = self
                    .raw
                    .index
                    .get(&(class.clone(), name.to_ascii_lowercase()))
                    .copied()
                {
                    idx
                } else {
                    self.raw.warn(
                        &C::READ_DSS_REFERENCE_DROPPED,
                        ctx(format!(
                            "property reference to unknown object `{class}.{name}`"
                        )),
                    );
                    return;
                }
            }
        };
        self.raw.active = Some(idx);
        let table = prop_table(&self.raw.objects[idx].class);
        let name = match table {
            Some(c) => {
                if let Some(i) = c.prop_index(prop) {
                    c.props[i].to_string()
                } else {
                    self.raw.warn(
                        &C::READ_DSS_PROPERTY_UNKNOWN,
                        ctx(format!(
                            "unknown property `{prop}` on {}; kept as written",
                            c.name
                        )),
                    );
                    prop.to_ascii_lowercase()
                }
            }
            None => prop.to_ascii_lowercase(),
        };
        let mut props = vec![RawProp {
            name: Some(name),
            value,
        }];
        props.extend(collect_props_for(
            table,
            scan,
            Some(prop),
            &mut self.raw,
            ctx,
        ));
        self.apply_props(idx, props, ctx);
    }

    fn do_new(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some((class, name)) = self.object_spec(scan, ctx) else {
            return;
        };
        if class.eq_ignore_ascii_case("circuit") {
            // A new circuit brings its Vsource named "source"; the line's
            // remaining properties edit that source. Its defaults (bus1 =
            // sourcebus etc.) stay implicit here so the reader can tell
            // written values from materialized defaults.
            self.raw.circuit_name = Some(name);
            self.raw.frequency.new_circuit();
            self.raw.solution_mode = super::solution_mode::SolutionMode::default();
            let idx = self.make_object("vsource", "source".into());
            self.consume_and_apply(idx, scan, ctx);
            return;
        }
        let key = (class.to_ascii_lowercase(), name.to_ascii_lowercase());
        let idx = match self.raw.index.get(&key) {
            Some(&existing) => {
                self.raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!(
                        "duplicate `New {class}.{name}`; editing the existing object"
                    )),
                );
                existing
            }
            None => self.make_object(&class, name),
        };
        self.consume_and_apply(idx, scan, ctx);
    }

    fn do_edit(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some((class, name)) = self.object_spec(scan, ctx) else {
            return;
        };
        let key = (class.to_ascii_lowercase(), name.to_ascii_lowercase());
        let Some(&idx) = self.raw.index.get(&key) else {
            self.raw.warn(
                &C::READ_DSS_REFERENCE_DROPPED,
                ctx(format!("`Edit {class}.{name}` on an unknown object")),
            );
            return;
        };
        self.consume_and_apply(idx, scan, ctx);
    }

    fn batch_targets(&mut self, class: &str, ctx: &dyn Fn(String) -> String) -> Option<Vec<usize>> {
        // Charge the selection scan, including objects in other classes, so
        // repeated empty matches cannot evade the whole-script work bound.
        let cost = self.raw.objects.len();
        if self.raw.total_props.saturating_add(cost) > MAX_TOTAL_PROPS {
            self.raw.halt(ctx(
                "BatchEdit selection exceeds the whole-script work budget".into(),
            ));
            return None;
        }
        self.raw.total_props += cost;
        Some(
            self.raw
                .objects
                .iter()
                .enumerate()
                .filter(|(_, object)| object.class == class)
                .map(|(i, _)| i)
                .collect(),
        )
    }

    fn do_batch_edit(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some((class, pattern)) = self.object_spec(scan, ctx) else {
            self.raw.warn(
                &C::READ_DSS_BATCH_UNRESOLVED,
                ctx("BatchEdit requires Class.Pattern".into()),
            );
            return;
        };
        if class.eq_ignore_ascii_case("circuit") {
            return; // Native BatchEdit deliberately ignores Circuit.
        }
        let class = class.to_ascii_lowercase();
        let Some(table) = prop_table(&class) else {
            self.raw.warn(
                &C::READ_DSS_BATCH_UNRESOLVED,
                ctx(format!("BatchEdit class {class} is not supported")),
            );
            return;
        };
        // The native regular expression searches names case-insensitively;
        // it is not a shell glob and is not implicitly anchored. Unsupported
        // backreferences/lookarounds fail explicitly rather than being skipped.
        let Some(expression) = batch_expression(&pattern) else {
            self.raw.warn(
                &C::READ_DSS_BATCH_UNRESOLVED,
                ctx("BatchEdit needs a supported ASCII regular expression".into()),
            );
            return;
        };
        let props = collect_props_for(Some(table), scan, None, &mut self.raw, ctx);
        let Some(targets) = self.batch_targets(&class, ctx) else {
            return;
        };
        let mut cursor = 0;
        while let Some(&idx) = targets.get(cursor) {
            if self.raw.halted {
                return;
            }
            if self.raw.total_props >= MAX_TOTAL_PROPS {
                self.raw.halt(ctx(
                    "BatchEdit iterator exceeds the whole-script work budget".into(),
                ));
                return;
            }
            self.raw.total_props += 1;
            cursor += 1;
            if !self.raw.objects[idx].name.is_ascii() {
                self.raw.warn(
                    &C::READ_DSS_BATCH_UNRESOLVED,
                    ctx("BatchEdit matching of non-ASCII object names is not qualified".into()),
                );
                return;
            }
            // Native First/Next selects every visited object, including the
            // last unmatched name. A following continuation edits that object.
            self.raw.active = Some(idx);
            if expression.is_match(self.raw.objects[idx].name.as_bytes()) {
                if self
                    .raw
                    .total_props
                    .saturating_add(props.len())
                    .saturating_add(1)
                    > MAX_TOTAL_PROPS
                    || self
                        .retained_property_count(idx)
                        .saturating_add(props.len())
                        .saturating_add(1)
                        > MAX_OBJECT_PROPS
                {
                    self.raw.halt(ctx(
                        "BatchEdit assignments exceed the property/work budget".into()
                    ));
                    return;
                }
                self.apply_props(idx, props.clone(), ctx);
                // Native MakeLike locates the template through the same
                // class iterator. Next resumes after that template, which
                // can skip names or revisit earlier ones; visits stay bounded.
                for property in props.iter().filter(|p| p.name.as_deref() == Some("like")) {
                    if let Some(position) = self
                        .raw
                        .index
                        .get(&(class.clone(), property.value.text.to_ascii_lowercase()))
                        .and_then(|target| targets.binary_search(target).ok())
                    {
                        cursor = position + 1;
                    } else {
                        self.raw.warn(
                            &C::READ_DSS_BATCH_UNRESOLVED,
                            ctx("BatchEdit copy template does not exist".into()),
                        );
                        return;
                    }
                }
            }
        }
    }

    fn do_more(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some(idx) = self.raw.active else {
            self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx("`~` with no active object".into()),
            );
            return;
        };
        self.consume_and_apply(idx, scan, ctx);
    }

    fn do_terminal_state(
        &mut self,
        verb: &str,
        scan: &mut Scanner,
        ctx: &dyn Fn(String) -> String,
    ) {
        if !self.raw.charge_preserved() {
            return;
        }
        self.raw.commands.push(RawCommand {
            verb: verb.into(),
            args: scan.remainder().into(),
        });
        let Some((class, name)) = self.object_spec(scan, ctx) else {
            self.raw.warn(
                &C::READ_DSS_LINE_UNRESOLVED,
                ctx("terminal command requires an explicit element".into()),
            );
            return;
        };
        let key = (class.to_ascii_lowercase(), name.to_ascii_lowercase());
        let Some(&idx) = self
            .raw
            .index
            .get(&key)
            .filter(|_| class.eq_ignore_ascii_case("line"))
        else {
            self.raw.warn(&C::READ_DSS_LINE_UNRESOLVED, ctx(format!("{verb} {class}.{name}: terminal state requires an existing ordinary Line; other classes remain unresolved")));
            return;
        };
        // The native command consumes positional values even when they have
        // parameter names. Bind the object now, not after later declarations.
        let mut integer = || {
            scan.next_param()
                .map(|p| p.value.to_f64(Some(&self.raw.vars)))
        };
        let terminal = integer();
        let conductor = integer();
        let number = |v: Option<std::result::Result<f64, _>>, default: usize| -> Option<usize> {
            match v {
                None => Some(default),
                Some(Ok(n)) if n.is_finite() && (0.0..=32.0).contains(&n) && n.fract() == 0.0 => {
                    Some(n as usize)
                }
                _ => None,
            }
        };
        let (Some(terminal @ 1..=2), Some(conductor)) = (number(terminal, 0), number(conductor, 0))
        else {
            self.raw.warn(&C::READ_DSS_LINE_UNRESOLVED, ctx(format!("{verb} Line.{name}: explicit terminal 1 or 2 and a nonnegative conductor index are required")));
            return;
        };
        if self.raw.total_props >= MAX_TOTAL_PROPS
            || self.retained_property_count(idx) >= MAX_OBJECT_PROPS
        {
            self.raw.halt(ctx(
                "line terminal history exceeds the supported work budget".into(),
            ));
            return;
        }
        let event = super::line_history::Event {
            position: self.raw.line_snapshot(idx),
            frequency: self.raw.snapshot_frequency(),
            rebuild_only: false,
            terminal_state: Some((terminal, conductor, verb == "close")),
        };
        self.raw.total_props += 1;
        self.raw
            .line_history
            .get_mut(&idx)
            .unwrap()
            .events
            .push(event);
        self.raw
            .geometry_timeline
            .push(super::geometry::timeline::Action::Terminal {
                object: idx,
                terminal,
                conductor,
                closed: verb == "close",
            });
        self.raw.active = Some(idx);
    }

    fn do_select(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some((class, name)) = self.object_spec(scan, ctx) else {
            return;
        };
        let key = (class.to_ascii_lowercase(), name.to_ascii_lowercase());
        match self.raw.index.get(&key) {
            Some(&idx) => self.raw.active = Some(idx),
            None => self.raw.warn(
                &C::READ_DSS_REFERENCE_DROPPED,
                ctx(format!("`Select {class}.{name}` on an unknown object")),
            ),
        }
    }

    fn do_set(&mut self, scan: &mut Scanner) {
        let mut position = 0;
        while let Some(p) = scan.next_param() {
            if p.value.text.is_empty() && p.name.is_none() {
                break;
            }
            let name = p.name.unwrap_or_default().to_ascii_lowercase();
            if self.raw.charge_preserved() {
                self.raw.option_assignment(&mut position, &name, &p.value);
                self.raw.options.push((name, p.value));
            }
        }
    }

    fn do_enabled(&mut self, verb: &str, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let canonical = COMMANDS[command_index(verb).unwrap()];
        if !self.raw.charge_preserved() {
            return;
        }
        self.raw.commands.push(RawCommand {
            verb: canonical.into(),
            args: scan.remainder().into(),
        });
        let Some(parameter) = scan.next_param() else {
            return;
        };
        if parameter
            .name
            .as_ref()
            .is_some_and(|name| !"object".starts_with(&name.to_ascii_lowercase()))
        {
            self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx("expected object=Class.Name".into()),
            );
            return;
        }
        let Some((class, name)) = parameter.value.text.split_once('.') else {
            return; // Native Enable/Disable requires an explicit class.
        };
        if class.eq_ignore_ascii_case("line") {
            self.do_line_enabled(name, canonical == "enable", ctx);
            return;
        }
        if !class.eq_ignore_ascii_case("vsource") {
            return; // Other equipment enable/disable histories remain unqualified.
        }
        let enabled = canonical == "enable";
        if name == "*" {
            // Native wildcard assignment does not select an object or invoke
            // RecalcElementData. Charge each visited state to bound repeated
            // wildcard work even though no property history is appended.
            let count = self.raw.source_replay.len();
            if self.raw.total_props.saturating_add(count) > MAX_TOTAL_PROPS {
                self.raw
                    .halt("source state assignments exceeded the whole parse budget".into());
                return;
            }
            self.raw.total_props += count;
            for replay in self.raw.source_replay.values_mut() {
                replay.enabled = enabled;
            }
        } else if let Some(&idx) = self
            .raw
            .index
            .get(&("vsource".into(), name.to_ascii_lowercase()))
        {
            self.apply_props(
                idx,
                vec![RawProp {
                    name: Some("enabled".into()),
                    value: Value::new(if enabled { "true" } else { "false" }),
                }],
                ctx,
            );
        } else {
            self.raw.warn(
                &C::READ_DSS_REFERENCE_DROPPED,
                ctx(format!("{canonical} names an unknown Vsource.{name}")),
            );
        }
    }

    fn do_line_enabled(&mut self, name: &str, enabled: bool, ctx: &dyn Fn(String) -> String) {
        let targets: Vec<_> = if name == "*" {
            self.raw
                .line_history
                .keys()
                .copied()
                .filter(|&idx| self.raw.objects[idx].class == "line")
                .collect()
        } else {
            self.raw
                .index
                .get(&("line".into(), name.to_ascii_lowercase()))
                .copied()
                .into_iter()
                .collect()
        };
        if targets.is_empty() && name != "*" {
            self.raw.warn(
                &C::READ_DSS_REFERENCE_DROPPED,
                ctx(format!("enabled assignment names an unknown Line.{name}")),
            );
            return;
        }
        if self
            .raw
            .total_props
            .saturating_add(targets.len().saturating_mul(2))
            > MAX_TOTAL_PROPS
            || targets
                .iter()
                .any(|&idx| self.retained_property_count(idx).saturating_add(2) > MAX_OBJECT_PROPS)
        {
            self.raw.halt(ctx(
                "Line enabled assignments exceed the supported work budget".into(),
            ));
            return;
        }
        let active = self.raw.active;
        for idx in targets {
            self.apply_props(
                idx,
                vec![RawProp {
                    name: Some("enabled".into()),
                    value: Value::new(if enabled { "true" } else { "false" }),
                }],
                ctx,
            );
        }
        if name == "*" {
            self.raw.active = active;
        }
    }

    /// Resolves a file argument relative to the current file's directory.
    /// Backslash separators (the format's DOS heritage) become `/`. Returns
    /// `None` when a confinement root is set (file parsing) and the resolved
    /// path does not lexically sit under the root — whether it climbs out with
    /// `..` or is an absolute path outside the root — so an untrusted case file
    /// cannot pull in arbitrary paths. An absolute include is admitted only
    /// when it strips the root as a prefix, which requires the root itself to
    /// be absolute; the file entry points normalize the case file's parent, so
    /// a case given by an absolute path admits absolute includes inside its
    /// own directory, while a relative case path admits only relative ones.
    fn resolve(&self, file_arg: &str) -> Option<PathBuf> {
        use std::path::Component;
        let rel = file_arg.replace('\\', "/");
        let base = self.dirs.last().cloned().unwrap_or_default();
        let joined = base.join(&rel);
        match &self.root {
            None => Some(joined),
            Some(root) => {
                // Containment: after stripping the root prefix, only plain
                // name components may remain. A leftover `..`, root, or drive
                // prefix means the path escapes — this also covers an empty
                // root (case file in the working directory), where
                // `starts_with` alone would accept absolute paths, and a root
                // that itself begins with `..`, where counting leading `..`
                // components would misjudge the climb.
                let normalized = lexical_normalize(&joined);
                normalized
                    .strip_prefix(root)
                    .is_ok_and(|rest| rest.components().all(|c| matches!(c, Component::Normal(_))))
                    .then_some(normalized)
            }
        }
    }

    /// Resolves an include argument, warning when it is refused for escaping
    /// the confinement boundary. `None` tells the caller to skip the include.
    fn resolve_or_warn(
        &mut self,
        verb: &str,
        file_arg: &str,
        ctx: &dyn Fn(String) -> String,
    ) -> Option<PathBuf> {
        let resolved = self.resolve(file_arg);
        if resolved.is_none() {
            let message = ctx(format!(
                "{verb} {file_arg}: refused; include escapes {}",
                self.boundary.noun()
            ));
            self.refuse_escape(message);
        }
        resolved
    }

    /// Records an include refused for leaving the confinement boundary: the
    /// warning line and the `Error` finding that keeps the run from exiting 0.
    fn refuse_escape(&mut self, message: String) {
        let suggested_action = match self.boundary {
            IncludeBoundary::CaseDirectory => {
                "place included files inside the case directory, or merge them into the case"
            }
            IncludeBoundary::IncludeRoot => {
                "place included files inside the include root, or merge them into the case"
            }
        };
        self.refuse(
            message,
            &crate::diagnostics::codes::READ_DSS_INCLUDE_REFUSED,
            suggested_action,
        );
    }

    /// Records a refused include: the warning line and the `Error` finding
    /// that keeps the run from exiting 0.
    fn refuse(
        &mut self,
        message: String,
        code: &'static crate::diagnostics::DiagnosticInfo,
        suggested_action: &'static str,
    ) {
        self.raw.record(
            crate::diagnostics::Diagnostic::of(code, message)
                .with_suggested_action(suggested_action),
        );
    }

    /// Charges one include against the budgets, returning whether to follow it.
    /// Charged at the attempt, so the loader is never called past the budget
    /// and the syscalls are bounded with the work. The counters live on the
    /// executor rather than in `RawDss`, which `Clear` resets. The refusal it
    /// emits stays put too: `RawDss::clear` keeps the parse record.
    fn charge_include(&mut self, verb: &str, path: &Path, ctx: &dyn Fn(String) -> String) -> bool {
        let over_budget = self.budget_spent
            || self.includes >= MAX_TOTAL_INCLUDES
            || self.include_bytes >= MAX_TOTAL_INCLUDE_BYTES;
        if !over_budget {
            self.includes += 1;
            return true;
        }
        if !self.budget_spent {
            self.budget_spent = true;
            let message = ctx(format!(
                "{verb} {}: refused; the case exceeded the include budget of {MAX_TOTAL_INCLUDES} \
                 files and {} MiB, so the rest of the includes were not followed",
                path.display(),
                MAX_TOTAL_INCLUDE_BYTES >> 20,
            ));
            self.refuse(
                message,
                &crate::diagnostics::codes::READ_DSS_INCLUDE_BUDGET,
                "check the case for an include cycle; a file that redirects to itself expands \
                 without bound",
            );
        }
        false
    }

    /// Records a failed include load. A containment refusal is the loader's
    /// own, covering what the lexical check cannot see: the path is inside the
    /// case directory but resolves out of it through a symbolic link. It
    /// carries the same code and severity as a lexical refusal. Every other
    /// load failure — including a `PermissionDenied` the filesystem raised on
    /// an include that is where it claims to be — stays a warning.
    fn warn_load_error(
        &mut self,
        verb: &str,
        path: &Path,
        e: &std::io::Error,
        ctx: &dyn Fn(String) -> String,
    ) {
        let message = ctx(format!("{verb} {}: {e}", path.display()));
        if Containment::refused_by_us(e) {
            self.refuse_escape(message);
        } else {
            self.raw.warn(&C::READ_DSS_INCLUDE_LOAD_FAILED, message);
        }
    }

    fn do_redirect(&mut self, scan: &mut Scanner, compile: bool, ctx: &dyn Fn(String) -> String) {
        let Some(p) = scan.next_param() else {
            self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx("redirect with no file".into()),
            );
            return;
        };
        let verb = if compile { "compile" } else { "redirect" };
        let Some(path) = self.resolve_or_warn(verb, &p.value.text, ctx) else {
            return;
        };
        if self.dirs.len() > MAX_REDIRECT_DEPTH {
            self.raw.warn(
                &C::READ_DSS_INCLUDE_DEPTH_LIMIT,
                ctx(format!("redirect depth limit at {}", path.display())),
            );
            return;
        }
        if !self.charge_include(verb, &path, ctx) {
            return;
        }
        match self.loader.load(&path) {
            Ok(text) => {
                self.include_bytes += text.len();
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                self.dirs.push(dir.clone());
                // `run_command` took the live var table out for `scan` to
                // borrow, so it reads empty here; reinstall the ambient
                // table (one copy, bounded by the include budget rather than
                // paid per command) so the include's own commands see and
                // can extend it, exactly like a directly nested command
                // would. A `Clear` inside the include bumps the generation
                // and installs its own empty table, which the check below
                // leaves alone instead of overwriting with the pre-redirect
                // snapshot.
                let ambient = scan.vars().cloned().unwrap_or_default();
                let outer = std::mem::replace(&mut self.raw.vars, ambient);
                let generation = self.raw.vars_generation;
                self.run_script(&text, &path.display().to_string());
                if self.raw.vars_generation == generation {
                    let added = std::mem::replace(&mut self.raw.vars, outer);
                    self.raw.vars.extend(added);
                }
                self.dirs.pop();
                // The engine keeps one current directory: Redirect restores
                // the caller's on return (SetCurrentDir(SaveDir)), Compile
                // pins it to the compiled file's OWN directory — ExecHelper
                // DoRedirect sets CurrDir once from the file path (~:300)
                // and compile exit reapplies it via SetDataPath (~:361) —
                // even when the compiled script itself compiled deeper. The
                // caller's later relative paths follow the compiled file.
                if compile && let Some(top) = self.dirs.last_mut() {
                    *top = dir;
                }
            }
            Err(e) => self.warn_load_error(verb, &path, &e, ctx),
        }
    }

    fn do_buscoords(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some(p) = scan.next_param() else {
            self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx("buscoords with no file".into()),
            );
            return;
        };
        let Some(path) = self.resolve_or_warn("buscoords", &p.value.text, ctx) else {
            return;
        };
        if !self.charge_include("buscoords", &path, ctx) {
            return;
        }
        match self.loader.load(&path) {
            Ok(text) => {
                self.include_bytes += text.len();
                for (line_no, line) in text.lines().enumerate() {
                    if self.raw.halted {
                        break;
                    }
                    let mut s = Scanner::new(line, None);
                    let Some(bus) = s.next_param() else { continue };
                    if bus.value.text.is_empty() {
                        continue;
                    }
                    let x = s.next_param().map(|p| p.value).unwrap_or_default();
                    let y = s.next_param().map(|p| p.value).unwrap_or_default();
                    match (x.to_f64(None), y.to_f64(None)) {
                        (Ok(x), Ok(y)) => {
                            if self.raw.charge_preserved() {
                                self.raw.buscoords.push(BusCoord {
                                    bus: bus.value.text,
                                    x,
                                    y,
                                });
                            }
                        }
                        _ => self.raw.warn(
                            &C::PARSE_DSS_SOURCE_MALFORMED,
                            ctx(format!(
                                "buscoords {}:{}: unparseable coordinates",
                                path.display(),
                                line_no + 1
                            )),
                        ),
                    }
                }
            }
            Err(e) => self.warn_load_error("buscoords", &path, &e, ctx),
        }
    }

    /// `SetBusXY bus=name x=.. y=..`, named or positional. The inline twin of
    /// `Buscoords`: it states one bus's coordinates in the deck itself, so it
    /// is the form that survives a single-document round trip.
    fn do_setbusxy(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let mut bus: Option<String> = None;
        let mut x: Option<f64> = None;
        let mut y: Option<f64> = None;
        let mut position = 0usize;
        while let Some(p) = scan.next_param() {
            let slot = match p.name.as_deref().map(str::to_ascii_lowercase).as_deref() {
                Some("bus") => 0,
                Some("x") => 1,
                Some("y") => 2,
                Some(_) => continue,
                None => {
                    let slot = position;
                    position += 1;
                    slot
                }
            };
            match slot {
                0 => bus = Some(p.value.text),
                1 => x = p.value.to_f64(None).ok(),
                2 => y = p.value.to_f64(None).ok(),
                _ => {}
            }
        }
        match (bus, x, y) {
            (Some(bus), Some(x), Some(y)) if !bus.is_empty() => {
                if self.raw.charge_preserved() {
                    self.raw.buscoords.push(BusCoord { bus, x, y });
                }
            }
            _ => self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx("setbusxy needs a bus and numeric x and y".into()),
            ),
        }
    }

    /// Reads `Class.Name` (or `object=Class.Name`) from the next parameter.
    fn object_spec(
        &mut self,
        scan: &mut Scanner,
        ctx: &dyn Fn(String) -> String,
    ) -> Option<(String, String)> {
        let p = scan.next_param()?;
        if let Some(name) = &p.name
            && !name.eq_ignore_ascii_case("object")
        {
            self.raw.warn(
                &C::PARSE_DSS_SOURCE_MALFORMED,
                ctx(format!("expected Class.Name, got `{name}=`")),
            );
            return None;
        }
        let spec = p.value.text;
        match spec.split_once('.') {
            Some((class, name)) if !class.is_empty() && !name.is_empty() => {
                Some((class.to_string(), name.to_string()))
            }
            _ => {
                self.raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!("malformed object spec `{spec}`")),
                );
                None
            }
        }
    }

    fn make_object(&mut self, class: &str, name: String) -> usize {
        let class_lc = class.to_ascii_lowercase();
        let idx = self.raw.objects.len();
        self.raw
            .index
            .insert((class_lc.clone(), name.to_ascii_lowercase()), idx);
        self.raw.objects.push(RawObject {
            class: class_lc,
            name,
            props: Vec::new(),
            edits: Vec::new(),
        });
        if matches!(
            self.raw.objects[idx].class.as_str(),
            "line"
                | "linecode"
                | "wiredata"
                | "cndata"
                | "linegeometry"
                | "linespacing"
                | "xfmrcode"
                | "transformer"
        ) {
            self.raw.line_history.insert(
                idx,
                super::line_history::LineHistory::new(self.raw.frequency.constructor()),
            );
            if matches!(
                self.raw.objects[idx].class.as_str(),
                "line" | "wiredata" | "cndata" | "linegeometry" | "linespacing"
            ) {
                self.raw
                    .geometry_timeline
                    .push(super::geometry::timeline::Action::New {
                        point: self.raw.line_snapshot(idx),
                        earth: self.raw.earth_model.clone(),
                    });
            }
        }
        if self.raw.objects[idx].class == "vsource" {
            let frequency = self.raw.frequency.constructor();
            self.raw.source_replay.insert(
                idx,
                SourceReplay {
                    constructor_frequency: frequency,
                    resets: Vec::new(),
                    enabled: true,
                    invalid_reference: None,
                },
            );
        }
        idx
    }

    fn consume_and_apply(
        &mut self,
        idx: usize,
        scan: &mut Scanner,
        ctx: &dyn Fn(String) -> String,
    ) {
        let props = collect_props_for(
            prop_table(&self.raw.objects[idx].class),
            scan,
            None,
            &mut self.raw,
            ctx,
        );
        self.apply_props(idx, props, ctx);
    }

    fn retained_property_count(&self, idx: usize) -> usize {
        self.raw.objects[idx].props.len()
            + self.raw.line_history.get(&idx).map_or(0, |h| {
                // Include the current command's pending closing checkpoint.
                h.copies.len()
                    + h.events.len()
                    + h.wires.values().map(Vec::len).sum::<usize>()
                    + self.raw.objects[idx].edits.len()
                    + 1
            })
            + self
                .raw
                .source_replay
                .get(&idx)
                .map_or(0, |r| r.resets.len())
    }

    fn record_line_calculation(&mut self, rebuild_only: bool, ctx: &dyn Fn(String) -> String) {
        let indices: Vec<_> = self
            .raw
            .line_history
            .keys()
            .copied()
            .filter(|&idx| self.raw.objects[idx].class == "line")
            .collect();
        if self.raw.total_props.saturating_add(indices.len()) > MAX_TOTAL_PROPS
            || indices
                .iter()
                .any(|&idx| self.retained_property_count(idx) >= MAX_OBJECT_PROPS)
        {
            self.raw.halt(ctx(
                "line calculation history exceeds the supported work budget".into(),
            ));
            return;
        }
        self.raw.total_props += indices.len();
        if !rebuild_only {
            self.raw
                .geometry_timeline
                .push(super::geometry::timeline::Action::Solve {
                    frequency: self.raw.snapshot_frequency(),
                });
        }
        for idx in indices {
            let event = super::line_history::Event {
                position: self.raw.line_snapshot(idx),
                frequency: self.raw.snapshot_frequency(),
                rebuild_only,
                terminal_state: None,
            };
            self.raw
                .line_history
                .get_mut(&idx)
                .unwrap()
                .events
                .push(event);
        }
    }

    fn append_source_resets(&mut self, target: usize, source: usize, base: usize) {
        if let Some(replay) = self.raw.source_replay.get(&source).cloned() {
            let target = self
                .raw
                .source_replay
                .get_mut(&target)
                .expect("source replay initialized");
            target.enabled = true;
            if target.invalid_reference.is_none() {
                target.invalid_reference = replay.invalid_reference;
            }
            target.resets.push((base, replay.constructor_frequency));
            target.resets.extend(
                replay
                    .resets
                    .into_iter()
                    .map(|(offset, frequency)| (base + offset, frequency)),
            );
        }
    }

    fn source_reference(&mut self, idx: usize, property: &RawProp) {
        if !self.raw.source_replay.contains_key(&idx) || property.value.text.is_empty() {
            return;
        }
        let (class, defaults): (&str, &[&str]) = match property.name.as_deref() {
            Some("daily" | "yearly" | "duty") => ("loadshape", &["default"]),
            Some("spectrum") => (
                "spectrum",
                &[
                    "default",
                    "defaultload",
                    "defaultgen",
                    "defaultvsource",
                    "linear",
                    "pwm6",
                    "dc6",
                ],
            ),
            _ => return,
        };
        let name = property.value.text.to_ascii_lowercase();
        if (class == "loadshape" && name == "none")
            || (!defaults.contains(&name.as_str()) && self.raw.find(class, &name).is_none())
        {
            let replay = self.raw.source_replay.get_mut(&idx).unwrap();
            if replay.invalid_reference.is_none() {
                replay.invalid_reference = Some(format!(
                    "{} references missing {class} object {:?} at assignment (native error 401)",
                    property.name.as_deref().unwrap(),
                    property.value.text,
                ));
            }
        }
    }

    fn copy_transformer_bindings(&mut self, idx: usize, src: usize, base: usize) {
        if self.raw.objects[idx].class == "transformer" {
            let bindings = self.raw.line_history[&src].xfmrcodes.clone();
            self.raw
                .line_history
                .get_mut(&idx)
                .unwrap()
                .xfmrcodes
                .extend(
                    bindings
                        .into_iter()
                        .map(|(offset, point)| (base + offset, point)),
                );
        }
    }

    fn copied_properties(&self, target: usize, source: usize) -> Vec<RawProp> {
        let mut cloned = self.raw.objects[source].props.clone();
        if self.raw.objects[target].class == "load" {
            // TLoadObj.MakeLike keeps the destination's physical connection.
            let bus = self.raw.objects[target]
                .get("bus1")
                .cloned()
                .unwrap_or_default();
            for property in &mut cloned {
                if property.name.as_deref() == Some("bus1") {
                    property.value.clone_from(&bus);
                }
            }
        }
        cloned
    }

    fn apply_like(&mut self, idx: usize, value: &Value, ctx: &dyn Fn(String) -> String) {
        let class = self.raw.objects[idx].class.clone();
        let key = (class.clone(), value.text.to_ascii_lowercase());
        match self.raw.index.get(&key).copied() {
            Some(src) => {
                if class == "vsource" && src == idx {
                    // Native source MakeLike(self) does not recalculate
                    // the current edit or reset its in-flight setters.
                    self.raw.source_replay.get_mut(&idx).unwrap().enabled = true;
                    return;
                }
                let base = self.raw.objects[idx].props.len();
                let src_len = self.raw.objects[src].props.len();
                let replay_charge = self
                    .raw
                    .source_replay
                    .get(&src)
                    .map_or(0, |r| r.resets.len() + 1)
                    + if self.raw.line_history.contains_key(&idx) {
                        1 + self.raw.objects[src].edit_bounds().count()
                    } else {
                        0
                    };
                // Refuse a splice that would push the object past the
                // cap. A self reference (`Edit X like=X`) or a mutual
                // chain otherwise doubles the prop count per edit; the
                // guard turns that into a warning instead of unbounded
                // growth.
                if self
                    .retained_property_count(idx)
                    .saturating_add(src_len)
                    .saturating_add(replay_charge)
                    > MAX_OBJECT_PROPS
                {
                    self.raw.warn(
                        &C::READ_DSS_VALUE_CLAMPED,
                        ctx(format!(
                            "like={}: {class} property count would exceed the supported \
                         maximum of {MAX_OBJECT_PROPS}; splice refused",
                            value.text
                        )),
                    );
                    return;
                }
                if self
                    .raw
                    .total_props
                    .saturating_add(src_len)
                    .saturating_add(replay_charge)
                    > MAX_TOTAL_PROPS
                {
                    self.raw.warn(
                        &C::READ_DSS_VALUE_CLAMPED,
                        ctx(format!(
                            "like={}: the script's retained property count would exceed \
                         the supported maximum of {MAX_TOTAL_PROPS}; splice refused",
                            value.text
                        )),
                    );
                    return;
                }
                self.raw.total_props += src_len + replay_charge;
                if self.raw.line_history.contains_key(&idx) {
                    let source = self.raw.line_snapshot(src);
                    let edit_start = self.raw.objects[idx].edits.len();
                    let edit_end = edit_start + self.raw.objects[src].edit_bounds().count();
                    self.raw.line_history.get_mut(&idx).unwrap().copies.push(
                        super::line_history::CopyPoint {
                            start: base,
                            end: base + src_len,
                            edit_start,
                            edit_end,
                            source,
                        },
                    );
                }
                self.copy_transformer_bindings(idx, src, base);
                self.append_source_resets(idx, src, base);
                let cloned = self.copied_properties(idx, src);
                let bounds: Vec<usize> = self.raw.objects[src]
                    .edit_bounds()
                    .map(|e| base + e)
                    .collect();
                self.raw.objects[idx].props.extend(cloned);
                self.raw.objects[idx].edits.extend(bounds);
                if matches!(
                    class.as_str(),
                    "line" | "wiredata" | "cndata" | "linegeometry" | "linespacing"
                ) {
                    self.raw
                        .geometry_timeline
                        .push(super::geometry::timeline::Action::Copy {
                            point: self.raw.line_snapshot(idx),
                            frequency: self.raw.snapshot_frequency(),
                        });
                }
            }
            None => self.raw.warn(
                &C::READ_DSS_REFERENCE_DROPPED,
                ctx(format!("like={} names an unknown {class}", value.text)),
            ),
        }
    }

    fn bind_geometry_references(
        &mut self,
        idx: usize,
        p: &RawProp,
        ctx: &dyn Fn(String) -> String,
    ) -> bool {
        if matches!(
            self.raw.objects[idx].class.as_str(),
            "linegeometry" | "line"
        ) && matches!(
            p.name.as_deref(),
            Some("wire" | "wires" | "cncable" | "cncables")
        ) {
            let names = if matches!(p.name.as_deref(), Some("wire" | "cncable")) {
                vec![p.value.text.clone()]
            } else {
                p.value.to_string_list(Some(&self.raw.vars))
            };
            if names.len() > 32
                || self
                    .retained_property_count(idx)
                    .saturating_add(names.len())
                    > MAX_OBJECT_PROPS
                || self.raw.total_props.saturating_add(names.len()) > MAX_TOTAL_PROPS
            {
                self.raw.halt(ctx(
                    "geometry conductor bindings exceed the supported count/work budget".into(),
                ));
                return false;
            }
            self.raw.total_props += names.len();
            let class = if p.name.as_deref().unwrap().starts_with("cn") {
                "cndata"
            } else {
                "wiredata"
            };
            let references = names
                .iter()
                .map(|name| {
                    self.raw
                        .index
                        .get(&(class.into(), name.to_ascii_lowercase()))
                        .map(|&object| self.raw.line_snapshot(object))
                })
                .collect();
            let offset = self.raw.objects[idx].props.len();
            self.raw
                .line_history
                .get_mut(&idx)
                .unwrap()
                .wires
                .insert(offset, references);
        }
        if matches!(
            self.raw.objects[idx].class.as_str(),
            "linegeometry" | "line"
        ) && p.name.as_deref() == Some("spacing")
        {
            let referenced = self
                .raw
                .index
                .get(&("linespacing".into(), p.value.text.to_ascii_lowercase()))
                .map(|&object| self.raw.line_snapshot(object));
            let offset = self.raw.objects[idx].props.len();
            self.raw
                .line_history
                .get_mut(&idx)
                .unwrap()
                .spacings
                .insert(offset, referenced);
        }
        if self.raw.objects[idx].class == "line" && p.name.as_deref() == Some("geometry") {
            let referenced = self
                .raw
                .index
                .get(&("linegeometry".into(), p.value.text.to_ascii_lowercase()))
                .map(|&object| self.raw.line_snapshot(object));
            let offset = self.raw.objects[idx].props.len();
            self.raw
                .line_history
                .get_mut(&idx)
                .unwrap()
                .geometries
                .insert(offset, referenced);
        }
        true
    }

    fn apply_props(&mut self, idx: usize, props: Vec<RawProp>, ctx: &dyn Fn(String) -> String) {
        self.raw.active = Some(idx);
        if self.raw.line_history.contains_key(&idx) {
            if self.retained_property_count(idx) > MAX_OBJECT_PROPS
                || self.raw.total_props >= MAX_TOTAL_PROPS
            {
                self.raw.warn(&C::READ_DSS_VALUE_CLAMPED, ctx("line history edit count exceeds the supported property/work budget; edit refused".into()));
                return;
            }
            // Reserve the closing checkpoint without exposing it as a
            // completed edit to references captured inside this command.
            self.raw.total_props += 1;
        }
        for p in props {
            // Keep the legacy flattened assignments and edit boundaries.
            // Stateful source/line readers also use reset/copy records to
            // recover hidden defaults and the state visible during MakeLike.
            if p.name.as_deref() == Some("like") {
                self.apply_like(idx, &p.value, ctx);
                continue;
            }
            if self.retained_property_count(idx) >= MAX_OBJECT_PROPS {
                self.raw.warn(
                    &C::READ_DSS_VALUE_CLAMPED,
                    ctx(format!(
                        "{}: property count exceeds the supported maximum of {MAX_OBJECT_PROPS}; \
                     further assignments dropped",
                        self.raw.objects[idx].class
                    )),
                );
                continue;
            }
            if self.raw.total_props >= MAX_TOTAL_PROPS {
                self.raw.warn(
                    &C::READ_DSS_VALUE_CLAMPED,
                    ctx(format!(
                        "{}: the script's retained property count exceeds the supported maximum \
                     of {MAX_TOTAL_PROPS}; further assignments dropped",
                        self.raw.objects[idx].class
                    )),
                );
                continue;
            }
            self.raw.total_props += 1;
            if self.raw.objects[idx].class == "line" && p.name.as_deref() == Some("linecode") {
                let referenced = self
                    .raw
                    .index
                    .get(&("linecode".into(), p.value.text.to_ascii_lowercase()))
                    .map(|&index| self.raw.line_snapshot(index));
                let offset = self.raw.objects[idx].props.len();
                self.raw
                    .line_history
                    .get_mut(&idx)
                    .unwrap()
                    .linecodes
                    .insert(offset, referenced);
            }
            if self.raw.objects[idx].class == "transformer" && p.name.as_deref() == Some("xfmrcode")
            {
                let referenced = self
                    .raw
                    .index
                    .get(&("xfmrcode".into(), p.value.text.to_ascii_lowercase()))
                    .map(|&index| self.raw.line_snapshot(index));
                let offset = self.raw.objects[idx].props.len();
                self.raw
                    .line_history
                    .get_mut(&idx)
                    .unwrap()
                    .xfmrcodes
                    .insert(offset, referenced);
            }
            if !self.bind_geometry_references(idx, &p, ctx) {
                return;
            }
            self.source_reference(idx, &p);
            if let Some(replay) = self.raw.source_replay.get_mut(&idx) {
                replay.record_property(&p);
            }
            self.raw.objects[idx].props.push(p);
            if matches!(
                self.raw.objects[idx].class.as_str(),
                "line" | "wiredata" | "cndata" | "linegeometry" | "linespacing"
            ) {
                self.raw
                    .geometry_timeline
                    .push(super::geometry::timeline::Action::Property {
                        point: self.raw.line_snapshot(idx),
                        offset: self.raw.objects[idx].props.len() - 1,
                    });
            }
        }
        // This command line was one engine Edit; it ends in
        // RecalcElementData, so record the boundary.
        let end = self.raw.objects[idx].props.len();
        self.raw.objects[idx].edits.push(end);
    }
}

fn batch_expression(pattern: &str) -> Option<regex::bytes::Regex> {
    if !pattern.is_ascii() {
        return None;
    }
    regex::bytes::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .unicode(false)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .ok()
}

fn prop_table(class: &str) -> Option<&'static DssClass> {
    prop::class_by_name(class)
}

/// Reads the remaining parameters of an object command, resolving names
/// (with abbreviation) and positional order against the class table. The
/// positional pointer continues from the last named property, as in the
/// reference. `after` seeds the pointer for property reference lines.
fn collect_props_for(
    class: Option<&'static DssClass>,
    scan: &mut Scanner,
    after: Option<&str>,
    raw: &mut RawDss,
    ctx: &dyn Fn(String) -> String,
) -> Vec<RawProp> {
    let mut out = Vec::new();
    let mut pointer: Option<usize> = class.zip(after).and_then(|(c, name)| c.prop_index(name));
    while let Some(p) = scan.next_param() {
        if p.value.text.is_empty() && p.name.is_none() {
            break;
        }
        let name = match (&p.name, class) {
            (Some(written), Some(c)) => {
                if let Some(i) = c.prop_index(written) {
                    pointer = Some(i);
                    Some(c.props[i].to_string())
                } else {
                    // Getcommand yields 0 for an unknown name, so the next
                    // positional lands on property 1 (the class Edit loops:
                    // `ParamPointer = CommandList.Getcommand(ParamName)`).
                    pointer = None;
                    raw.warn(
                        &C::READ_DSS_PROPERTY_UNKNOWN,
                        ctx(format!(
                            "unknown property `{written}` on {}; kept as written",
                            c.name
                        )),
                    );
                    Some(written.to_ascii_lowercase())
                }
            }
            (Some(written), None) => Some(written.to_ascii_lowercase()),
            (None, Some(c)) => {
                let next = pointer.map_or(0, |i| i + 1);
                pointer = Some(next);
                if let Some(canon) = c.props.get(next) {
                    Some((*canon).to_string())
                } else {
                    raw.warn(
                        &C::PARSE_DSS_SOURCE_MALFORMED,
                        ctx(format!(
                            "positional value `{}` beyond the last {} property",
                            p.value.text, c.name
                        )),
                    );
                    None
                }
            }
            (None, None) => None,
        };
        out.push(RawProp {
            name,
            value: p.value,
        });
    }
    out
}

/// Parses `.dss` text. `path` anchors relative includes; pass the file's
/// path when the text came from a file, anything descriptive otherwise.
///
/// Includes are resolved through `loader` without confinement: a caller that
/// passes a filesystem-backed loader lets `Redirect`/`Compile`/`Buscoords`
/// read any path the loader accepts. For untrusted input use
/// [`crate::convert::parse`] on a [`powerio_core::Source`] (acquisition
/// confined to the source's root) or [`__parse_raw_file`] (includes confined
/// to the case directory), or enforce your own containment inside the
/// loader.
#[doc(hidden)]
pub fn __parse_raw_with(text: &str, path: &str, loader: &mut impl Loader) -> RawDss {
    run_executor(text, path, None, IncludeBoundary::CaseDirectory, loader)
}

/// Projects a source acquisition failure onto the loader's `io::Error`,
/// keeping the executor's split: a refusal the acquisition stack raised (a
/// name escaping the root, a symbolic link) reports as a refused include,
/// and an ordinary filesystem failure as a load failure.
fn include_acquisition_error(error: &powerio_core::Error) -> std::io::Error {
    // Matched by registered code spelling: the code register is the stable
    // surface a consumer keys on.
    let refusal = error.diagnostics().first().is_some_and(|d| {
        d.code() == "REQUEST.SOURCE.ESCAPES_ROOT"
            || d.code() == "REQUEST.SOURCE.SYMLINK_REFUSED"
            || d.code() == "REQUEST.SOURCE.NOT_A_FILE"
    });
    if refusal {
        Containment::refused(error.to_string())
    } else {
        std::io::Error::other(error.to_string())
    }
}

/// Reads an include the executor already resolved and confined lexically,
/// through the source's own acquisition walk, so a path component replaced
/// during the parse cannot redirect the read outside the root. The resolved
/// path is rebased onto `lexical_root` — the executor's confinement root —
/// and acquired by that root relative name; an in-memory source consults its
/// named buffers under the same names. A leading byte order mark on an
/// included file is not decoded (the retained bytes keep it).
fn source_include_loader(
    source: &powerio_core::Source,
    lexical_root: PathBuf,
) -> impl FnMut(&Path) -> std::io::Result<String> {
    move |p: &Path| {
        let relative = p.strip_prefix(&lexical_root).map_err(|_| {
            Containment::refused(format!(
                "{} does not sit under the confinement root",
                p.display()
            ))
        })?;
        let mut name = String::new();
        for component in relative.components() {
            if !name.is_empty() {
                name.push('/');
            }
            name.push_str(&component.as_os_str().to_string_lossy());
        }
        let buffer = source
            .root_buffer(&name)
            .map_err(|error| include_acquisition_error(&error))?;
        let text = std::str::from_utf8(buffer.content_bytes()).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("stream did not contain valid UTF-8: {e}"),
            )
        })?;
        Ok(text.to_owned())
    }
}

/// Executes one `.dss` source, following `Redirect`/`Compile`/`Buscoords`
/// includes through the source's own acquisition policy: a file source
/// admits includes beneath its acquisition root — the case directory, unless
/// a wider root was selected with
/// [`powerio_core::Source::with_acquisition_root`] — and an in-memory source
/// admits only the named buffers supplied to it. An include that escapes is
/// refused with an `Error` finding, whether it climbs out with `..`, is an
/// absolute path outside the root, or escapes through a symbolic link; the
/// acquisition walk refuses symbolic links at read time, so a component
/// replaced during the parse cannot redirect a read outside the root. The
/// include budget in files and bytes and the nesting limit are enforced by
/// the executor unchanged.
pub(crate) fn parse_raw_from_source(source: &powerio_core::Source) -> Result<RawDss> {
    let primary = source.primary_buffer().map_err(|error| Error::FormatRead {
        format: "dss",
        message: error.to_string(),
    })?;
    let text = std::str::from_utf8(primary.content_bytes()).map_err(|e| Error::FormatRead {
        format: "case text",
        message: format!("not valid UTF-8: {e}"),
    })?;
    if let Some(root) = source.selected_acquisition_root() {
        // A selected root is canonical, so resolution runs in the canonical
        // root's space, the same space the acquisition walk confines to. The
        // primary's own directory seeds relative includes.
        let mut file = root.to_path_buf();
        file.extend(primary.directory_segments());
        match Path::new(source.name()).file_name() {
            Some(base) => file.push(base),
            None => file.push(source.name()),
        }
        let mut loader = source_include_loader(source, root.to_path_buf());
        Ok(parse_raw_confined_under(
            text,
            &file.display().to_string(),
            root,
            &mut loader,
        ))
    } else {
        // The default root of a file source is its containing directory, and
        // an in-memory source resolves the same way against its (possibly
        // empty) name prefix; either way the executor's lexical root is the
        // parent of the source name, and the loader rebases resolved paths
        // onto it.
        let lexical_root = lexical_normalize(
            Path::new(source.name())
                .parent()
                .unwrap_or_else(|| Path::new("")),
        );
        let mut loader = source_include_loader(source, lexical_root);
        Ok(parse_raw_with_confined(text, source.name(), &mut loader))
    }
}

/// The loader's own containment refusal, carried inside the `io::Error` so it
/// stays distinguishable from a `PermissionDenied` the filesystem raised. A
/// mode 000 file inside the case directory is an ordinary unreadable include,
/// not an escape attempt, and must not be reported as one.
#[derive(Debug)]
pub(crate) struct Containment(String);

impl Containment {
    pub(crate) fn refused(reason: impl Into<String>) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            Containment(reason.into()),
        )
    }

    /// Whether `e` is a refusal this module raised.
    pub(crate) fn refused_by_us(e: &std::io::Error) -> bool {
        e.get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Self>)
    }
}

impl std::fmt::Display for Containment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Containment {}

/// Like [`__parse_raw_with`], but confines `Redirect`/`Compile`/`Buscoords`
/// includes to the directory of `path`: an include that is absolute or climbs
/// out of that directory with `..` is refused with a warning and read nothing.
/// Used by the file entry point so an untrusted case file on disk cannot read
/// arbitrary paths.
pub(crate) fn parse_raw_with_confined(text: &str, path: &str, loader: &mut impl Loader) -> RawDss {
    let root = lexical_normalize(
        &Path::new(path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    );
    run_executor(
        text,
        path,
        Some(root),
        IncludeBoundary::CaseDirectory,
        loader,
    )
}

/// Like [`parse_raw_with_confined`], but confines includes to `include_root`
/// instead of the case directory. The caller vouches that the case file sits
/// under `include_root`; refusals name the include root as the boundary.
pub(crate) fn parse_raw_confined_under(
    text: &str,
    path: &str,
    include_root: &Path,
    loader: &mut impl Loader,
) -> RawDss {
    let root = lexical_normalize(include_root);
    run_executor(text, path, Some(root), IncludeBoundary::IncludeRoot, loader)
}

fn run_executor(
    text: &str,
    path: &str,
    root: Option<PathBuf>,
    boundary: IncludeBoundary,
    loader: &mut impl Loader,
) -> RawDss {
    let mut exec = Executor {
        raw: RawDss::default(),
        loader,
        dirs: vec![
            Path::new(path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        ],
        root,
        boundary,
        includes: 0,
        include_bytes: 0,
        budget_spent: false,
        var_count: 0,
        var_bytes: 0,
        var_budget_spent: false,
    };
    exec.run_script(text, path);
    exec.raw
}

/// Parses a `.dss` file from disk, following its includes through the
/// source's acquisition walk. `Redirect`/`Compile`/`Buscoords` includes are
/// confined to the case directory, lexically and with symbolic links refused
/// at read time, exactly like [`crate::convert::parse`] on a file source; an
/// include that escapes is refused with a warning. Use [`__parse_raw_with`]
/// with your own loader for unconfined resolution.
#[doc(hidden)]
pub fn __parse_raw_file(path: impl AsRef<Path>) -> Result<RawDss> {
    let path = path.as_ref();
    let source = powerio_core::Source::open(path).map_err(|error| Error::Io {
        path: path.display().to_string(),
        source: std::io::Error::other(error.to_string()),
    })?;
    parse_raw_from_source(&source)
}

#[cfg(test)]
mod tests {
    #[test]
    fn calculation_events_record_frequency_and_copy_horizons() {
        let raw = parse(
            "New Circuit.c\nNew Line.a\nSolve frequency=50\nNew Line.b like=a\nBuildY\nSolve frequency=70",
        );
        let a = raw
            .objects
            .iter()
            .position(|o| o.class == "line" && o.name == "a")
            .unwrap();
        let b = raw
            .objects
            .iter()
            .position(|o| o.class == "line" && o.name == "b")
            .unwrap();
        assert_eq!(raw.line_history[&a].events.len(), 3);
        assert_eq!(raw.line_history[&b].copies[0].source.events, 1);
        assert_eq!(
            raw.line_history[&a].events[0].frequency.to_bits(),
            50.0_f64.to_bits()
        );
        assert!(raw.line_history[&b].events[0].rebuild_only);
        assert_eq!(
            raw.line_history[&b].events[1].frequency.to_bits(),
            70.0_f64.to_bits()
        );
    }

    #[test]
    fn calculation_history_charges_the_whole_parse_budget_atomically() {
        let mut raw = parse("New Circuit.c\nNew Line.a\nNew Line.b");
        raw.total_props = MAX_TOTAL_PROPS - 1;
        let mut loader = no_files;
        let mut executor = Executor {
            raw,
            loader: &mut loader,
            dirs: Vec::new(),
            root: None,
            boundary: IncludeBoundary::CaseDirectory,
            includes: 0,
            include_bytes: 0,
            budget_spent: false,
            var_count: 0,
            var_bytes: 0,
            var_budget_spent: false,
        };
        executor.record_line_calculation(false, &|s| s);
        assert!(executor.raw.halted);
        assert!(
            executor
                .raw
                .line_history
                .values()
                .all(|h| h.events.is_empty())
        );
    }

    #[test]
    fn line_wildcards_preserve_selection_and_apply_only_to_existing_lines() {
        let raw = parse(
            "New Circuit.c\nNew Line.a\nNew Line.b\nDisable Line.*\n~ enabled=yes\nNew Line.c",
        );
        assert_eq!(
            raw.find("line", "a").unwrap().get("enabled").unwrap().text,
            "false"
        );
        assert_eq!(
            raw.find("line", "b").unwrap().get("enabled").unwrap().text,
            "yes"
        );
        assert!(raw.find("line", "c").unwrap().get("enabled").is_none());
    }

    #[test]
    fn empty_linecode_self_copies_bound_edit_history_as_well_as_properties() {
        let raw = parse(&format!(
            "New Linecode.empty\n{}",
            "Edit Linecode.empty like=empty\n".repeat(30)
        ));
        let object = raw.find("linecode", "empty").unwrap();
        let history = &raw.line_history[&0];
        assert_eq!(object.props, [] as [crate::dss::raw::RawProp; 0]);
        assert!(history.copies.len() < 30);
        assert!(object.edits.len() + history.copies.len() <= MAX_OBJECT_PROPS);
        assert!(raw.total_props <= MAX_TOTAL_PROPS);
        assert!(
            raw.diagnostics
                .iter()
                .any(|d| d.code() == C::READ_DSS_VALUE_CLAMPED.code)
        );
    }

    use super::*;

    fn no_files(_: &Path) -> std::io::Result<String> {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "test"))
    }

    fn parse(text: &str) -> RawDss {
        __parse_raw_with(text, "test.dss", &mut no_files)
    }

    #[test]
    fn batch_edit_matches_names_and_keeps_native_last_visited_object_active() {
        let raw =
            parse("new load.alpha kw=1\nnew load.beta kw=2\nbatchedit load.ALP kw=3\n~ kvar=4\n");
        assert_eq!(
            raw.find("load", "alpha").unwrap().get("kw").unwrap().text,
            "3"
        );
        assert_eq!(
            raw.find("load", "beta").unwrap().get("kw").unwrap().text,
            "2"
        );
        assert_eq!(
            raw.find("load", "beta").unwrap().get("kvar").unwrap().text,
            "4"
        );
        assert!(raw.commands.iter().all(|c| c.verb != "batchedit"));
    }

    #[test]
    fn unsupported_batch_selectors_are_errors() {
        for selector in ["load.(?=alpha)", "load.(a)\\1", "missing..*", "load.é"] {
            let raw = parse(&format!("new load.alpha kw=1\nbatchedit {selector} kw=3\n"));
            assert!(
                raw.diagnostics
                    .iter()
                    .any(|d| d.code() == C::READ_DSS_BATCH_UNRESOLVED.code),
                "{selector}"
            );
        }
    }

    #[test]
    fn source_copy_reenables_independently_of_the_templates_property_history() {
        let raw = parse(
            "New Circuit.c enabled=no\nNew Vsource.copy enabled=no like=source\nEdit Vsource.source like=source",
        );
        for object in raw.of_class("vsource") {
            assert!(raw.source_replay(object).enabled);
        }
        let raw = parse("New Circuit.c enabled=no\nNew Vsource.copy like=source enabled=1");
        assert!(
            raw.of_class("vsource")
                .all(|o| !raw.source_replay(o).enabled)
        );
    }

    #[test]
    fn source_wildcard_updates_current_objects_without_changing_active_selection() {
        let raw = parse(
            "New Circuit.c\nNew Vsource.aux\nDisable Vsource.*\n~ enabled=yes\nNew Vsource.later",
        );
        assert!(
            !raw.source_replay(raw.find("vsource", "source").unwrap())
                .enabled
        );
        assert!(
            raw.source_replay(raw.find("vsource", "aux").unwrap())
                .enabled
        );
        assert!(
            raw.source_replay(raw.find("vsource", "later").unwrap())
                .enabled
        );
        assert!(
            raw.commands
                .iter()
                .any(|c| c.verb == "disable" && c.args == "Vsource.*")
        );
    }

    #[test]
    fn repeated_source_wildcards_charge_the_existing_whole_parse_budget() {
        let mut raw = parse("New Circuit.c");
        raw.total_props = MAX_TOTAL_PROPS;
        let mut loader = no_files;
        let mut executor = Executor {
            raw,
            loader: &mut loader,
            dirs: Vec::new(),
            root: None,
            boundary: IncludeBoundary::CaseDirectory,
            includes: 0,
            include_bytes: 0,
            budget_spent: false,
            var_count: 0,
            var_bytes: 0,
            var_budget_spent: false,
        };
        executor.do_enabled("disable", &mut Scanner::new("Vsource.*", None), &|s| s);
        let raw = executor.raw;
        assert!(raw.halted);
        assert!(
            raw.source_replay(raw.find("vsource", "source").unwrap())
                .enabled
        );
    }

    #[test]
    fn empty_source_copy_histories_are_bounded_even_without_properties() {
        let mut text = String::from("New Circuit.c\nNew Vsource.other\n");
        for _ in 0..24 {
            text.push_str("Edit Vsource.source like=other\nEdit Vsource.other like=source\n");
        }
        let raw = parse(&text);
        assert!(raw.warnings.iter().any(|w| w.contains("maximum")));
        assert!(raw.total_props <= MAX_TOTAL_PROPS);
        for object in &raw.objects {
            assert!(
                object.props.len() + raw.source_replay(object).resets.len() <= MAX_OBJECT_PROPS
            );
        }
    }

    #[test]
    fn new_object_with_positional_and_named() {
        let raw = parse("New Line.l1 b1 b2 lc 0.3 phases=2 r1=0.1");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("bus1").unwrap().text, "b1");
        assert_eq!(l.get("bus2").unwrap().text, "b2");
        assert_eq!(l.get("linecode").unwrap().text, "lc");
        assert_eq!(l.get("length").unwrap().text, "0.3");
        assert_eq!(l.get("phases").unwrap().text, "2");
        assert_eq!(l.get("r1").unwrap().text, "0.1");
        assert_eq!(raw.warnings, [] as [std::string::String; 0]);
    }

    #[test]
    fn positional_continues_after_named() {
        // After r1=0.1 (index 5), the next positional is x1 (index 6).
        let raw = parse("New Line.l1 r1=0.1 0.2");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("x1").unwrap().text, "0.2");
    }

    #[test]
    fn unknown_property_resets_the_positional_pointer() {
        // `ParamPointer = Getcommand("bogus")` is 0 in the engine, so the
        // next positional gets property 1 (bus1), not the one after r1.
        let raw = parse("New Line.l1 r1=0.1 bogus=2 0.5");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("bus1").unwrap().text, "0.5");
        assert!(l.get("x1").is_none());
        assert_eq!(raw.warnings.len(), 1);
    }

    #[test]
    fn tilde_continues_the_active_object() {
        let raw = parse("New Load.ld bus1=b1\n~ kW=15 kvar=3\nMore pf=0.9");
        let ld = raw.find("load", "ld").unwrap();
        assert_eq!(ld.get("kw").unwrap().text, "15");
        assert_eq!(ld.get("kvar").unwrap().text, "3");
        assert_eq!(ld.get("pf").unwrap().text, "0.9");
    }

    #[test]
    fn abbreviated_property_names() {
        let raw = parse("New Line.l1 ph=3 len=2 rm=(1 | 0 1)");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("phases").unwrap().text, "3");
        assert_eq!(l.get("length").unwrap().text, "2");
        assert!(l.get("rmatrix").unwrap().quoted);
    }

    #[test]
    fn new_circuit_creates_the_source() {
        let raw = parse("New Circuit.test basekv=115 pu=1.05\n~ angle=30");
        assert_eq!(raw.circuit_name.as_deref(), Some("test"));
        let vs = raw.find("vsource", "source").unwrap();
        assert_eq!(vs.get("basekv").unwrap().text, "115");
        assert_eq!(vs.get("angle").unwrap().text, "30");
        // bus1 was not written; the default (sourcebus) is the reader's to
        // materialize, so the raw layer must not invent it.
        assert!(vs.get("bus1").is_none());
    }

    #[test]
    fn edit_and_property_reference() {
        let raw = parse("New Line.l1 length=1\nEdit Line.l1 length=2\nLine.l1.Length=3 phases=2");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("length").unwrap().text, "3");
        assert_eq!(l.get("phases").unwrap().text, "2");
    }

    #[test]
    fn property_reference_resolves_abbreviations() {
        let raw = parse("New Line.l1 bus1=a\nLine.l1.Len=2.5");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("length").unwrap().text, "2.5");
        assert_eq!(raw.warnings, [] as [std::string::String; 0]);
    }

    #[test]
    fn bare_property_edits_the_active_object() {
        let raw = parse("New Line.l1 bus1=a bus2=b\nlength=2.5");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("length").unwrap().text, "2.5");
        assert_eq!(raw.warnings, [] as [std::string::String; 0]);
    }

    #[test]
    fn classless_reference_uses_the_active_class() {
        // SetObject with no dot in the spec looks the name up in the last
        // referenced class, line here via the active object.
        let raw = parse("New Line.l1 bus1=a\nNew Line.l2 bus1=b\nl1.length=7 phases=2");
        let l1 = raw.find("line", "l1").unwrap();
        assert_eq!(l1.get("length").unwrap().text, "7");
        assert_eq!(l1.get("phases").unwrap().text, "2");
        assert!(raw.find("line", "l2").unwrap().get("length").is_none());
        assert_eq!(raw.warnings, [] as [std::string::String; 0]);
    }

    #[test]
    fn like_splices_source_props() {
        let raw = parse("New Load.a kW=10 pf=0.9\nNew Load.b like=a kW=20");
        let b = raw.find("load", "b").unwrap();
        assert_eq!(b.get("kw").unwrap().text, "20");
        assert_eq!(b.get("pf").unwrap().text, "0.9");
    }

    #[test]
    fn the_whole_parse_record_ceilings_bound_repeated_execution() {
        // A tiny loader-backed include re-executed many times repeats its
        // preserved commands and findings; the ceilings bound what one parse
        // may keep and stop execution with exactly one terminal refusal.
        struct Repeat;
        impl Loader for Repeat {
            fn load(&mut self, _: &Path) -> std::io::Result<String> {
                // Each load yields many preserved commands and one warning.
                let mut text = String::new();
                for _ in 0..8192 {
                    text.push_str("Solve\n");
                }
                text.push_str("Bogus命令 arg\n");
                Ok(text)
            }
        }
        use std::fmt::Write as _;
        let mut script = String::new();
        for index in 0..512 {
            let _ = writeln!(script, "Redirect inc{index}.dss");
        }
        let raw = __parse_raw_with(&script, "test.dss", &mut Repeat);
        assert!(raw.commands.len() <= MAX_TOTAL_PRESERVED);
        assert!(raw.diagnostics.len() <= MAX_TOTAL_DIAGNOSTICS + 1);
        let refusals = raw
            .warnings
            .iter()
            .filter(|w| w.contains("whole parse ceiling"))
            .count();
        assert_eq!(refusals, 1, "exactly one terminal refusal: {refusals}");
    }

    #[test]
    fn crossing_both_ceilings_in_either_order_leaves_one_refusal() {
        // Whichever ceiling fires first, the halt is one indivisible step:
        // exactly one READ.DSS.INCLUDE_BUDGET record lands in both channels
        // and the other ceiling's later crossing adds nothing.
        struct Warny;
        impl Loader for Warny {
            fn load(&mut self, _: &Path) -> std::io::Result<String> {
                let mut text = String::new();
                for _ in 0..8192 {
                    text.push_str("Bogus命令 arg\n");
                }
                for _ in 0..8192 {
                    text.push_str("Solve\n");
                }
                Ok(text)
            }
        }
        struct Quiet;
        impl Loader for Quiet {
            fn load(&mut self, _: &Path) -> std::io::Result<String> {
                let mut text = String::new();
                for _ in 0..16384 {
                    text.push_str("Solve\n");
                }
                text.push_str("Bogus命令 arg\n");
                Ok(text)
            }
        }
        let count = |raw: &RawDss| {
            (
                raw.warnings
                    .iter()
                    .filter(|w| w.contains("READ.DSS.INCLUDE_BUDGET"))
                    .count(),
                raw.diagnostics
                    .iter()
                    .filter(|d| d.code() == "READ.DSS.INCLUDE_BUDGET")
                    .count(),
            )
        };
        // Findings first: unknown commands warn AND preserve, so a warning
        // heavy prefix crosses the diagnostic ceiling, then preserved lines
        // keep arriving.
        let mut script = String::new();
        {
            use std::fmt::Write as _;
            for index in 0..64 {
                let _ = writeln!(script, "Redirect winc{index}.dss");
            }
        }
        let raw = __parse_raw_with(&script, "test.dss", &mut Warny);
        assert!(raw.commands.len() <= MAX_TOTAL_PRESERVED);
        assert!(raw.diagnostics.len() <= MAX_TOTAL_DIAGNOSTICS + 1);
        assert_eq!(count(&raw), (1, 1), "findings-first order");

        // Preserved first: quiet preserved lines cross that ceiling, then
        // warning lines keep arriving.
        let mut script = String::new();
        {
            use std::fmt::Write as _;
            for index in 0..80 {
                let _ = writeln!(script, "Redirect qinc{index}.dss");
            }
        }
        let raw = __parse_raw_with(&script, "test.dss", &mut Quiet);
        assert!(raw.commands.len() <= MAX_TOTAL_PRESERVED);
        assert!(raw.diagnostics.len() <= MAX_TOTAL_DIAGNOSTICS + 1);
        assert_eq!(count(&raw), (1, 1), "preserved-first order");
    }

    #[test]
    fn a_ceiling_crossed_inside_buscoords_halts_without_false_malformed_findings() {
        // Once the preserved ceiling fires partway through a coordinate
        // file, the halt stops the scan: lines past the refusal are neither
        // retained nor reported, and a well formed line is never called
        // malformed just because its charge was refused. The unparseable
        // tail after the crossing pins the latch itself: scanning past the
        // halt would report those lines malformed.
        use std::fmt::Write as _;
        let root = std::env::temp_dir().join(format!("powerio-dist-fs008-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let extra = 64usize;
        let mut coords = String::with_capacity((MAX_TOTAL_PRESERVED + 2 * extra) * 12);
        for index in 0..MAX_TOTAL_PRESERVED + extra {
            let _ = writeln!(coords, "b{index} 1 2");
        }
        for index in 0..extra {
            let _ = writeln!(coords, "tail{index} xx yy");
        }
        std::fs::write(root.join("coords.csv"), coords).unwrap();
        std::fs::write(root.join("master.dss"), "Buscoords coords.csv\n").unwrap();

        let raw = __parse_raw_file(root.join("master.dss")).unwrap();
        std::fs::remove_dir_all(&root).unwrap();

        let warned = raw
            .warnings
            .iter()
            .filter(|w| w.contains("READ.DSS.INCLUDE_BUDGET"))
            .count();
        let recorded = raw
            .diagnostics
            .iter()
            .filter(|d| d.code() == "READ.DSS.INCLUDE_BUDGET")
            .count();
        assert_eq!((warned, recorded), (1, 1), "exactly one terminal refusal");
        let malformed_warned = raw
            .warnings
            .iter()
            .filter(|w| w.contains("PARSE.DSS.SOURCE_MALFORMED"))
            .count();
        let malformed_recorded = raw
            .diagnostics
            .iter()
            .filter(|d| d.code() == "PARSE.DSS.SOURCE_MALFORMED")
            .count();
        assert_eq!(
            (malformed_warned, malformed_recorded),
            (0, 0),
            "lines past the halt were scanned and reported"
        );
        assert_eq!(
            raw.buscoords.len(),
            MAX_TOTAL_PRESERVED,
            "retention must fill the budget exactly and stop"
        );
    }

    #[test]
    fn the_whole_parse_property_retention_is_bounded() {
        // One object filled near its own cap, then spliced into many new
        // objects: per object each splice is legal, but the script's total
        // retained properties cross the whole-parse budget and further
        // splices refuse through the clamp diagnostic.
        use std::fmt::Write as _;
        let mut script = String::from("New Load.seed kW=1\n");
        // Grow the seed to ~32k props by doubling (self splice under the
        // per-object cap).
        for _ in 0..15 {
            script.push_str("Edit Load.seed like=seed\n");
        }
        for index in 0..200 {
            let _ = writeln!(script, "New Load.c{index} like=seed");
        }
        let raw = parse(&script);
        let total: usize = raw.objects.iter().map(|object| object.props.len()).sum();
        assert!(
            total <= MAX_TOTAL_PROPS,
            "retained {total} properties past the whole-parse budget"
        );
        assert!(
            raw.warnings
                .iter()
                .any(|w| w.contains("retained property count")),
            "expected the whole-parse refusal, got {} warnings",
            raw.warnings.len()
        );
    }

    #[test]
    fn self_referencing_like_cannot_explode_the_prop_count() {
        // `Edit X like=X` splices the object into itself; unbounded, each
        // repeat doubles the prop count. A few dozen lines would exhaust
        // memory. The cap turns the runaway splices into warnings.
        let mut script = String::from("New Load.a kW=1\n");
        for _ in 0..40 {
            script.push_str("Edit Load.a like=a\n");
        }
        let raw = parse(&script);
        let a = raw.find("load", "a").unwrap();
        assert!(
            a.props.len() <= MAX_OBJECT_PROPS,
            "prop count {} exceeded the cap",
            a.props.len()
        );
        assert!(
            raw.warnings.iter().any(|w| w.contains("splice refused")),
            "expected a refusal warning, got {:?}",
            raw.warnings
        );
    }

    #[test]
    fn unknown_class_is_preserved_raw() {
        let raw = parse("New Reactor.r1 bus1=b1 x=3");
        let r = raw.find("reactor", "r1").unwrap();
        assert_eq!(r.get("bus1").unwrap().text, "b1");
        assert_eq!(r.get("x").unwrap().text, "3");
    }

    #[test]
    fn set_options_accumulate() {
        let raw = parse("Set VoltageBases=[115, 12.47]\nset mode=snapshot");
        assert_eq!(raw.options[0].0, "voltagebases");
        assert_eq!(
            raw.options[0].1.to_vector(None).unwrap(),
            vec![115.0, 12.47]
        );
        assert_eq!(raw.options[1].0, "mode");
    }

    #[test]
    fn unexecuted_commands_are_preserved() {
        let raw = parse("Solve\ncalcv\nShow Voltages LN");
        let verbs: Vec<&str> = raw.commands.iter().map(|c| c.verb.as_str()).collect();
        assert_eq!(verbs, vec!["solve", "calcvoltagebases", "show"]);
        assert_eq!(raw.commands[2].args, "Voltages LN");
    }

    /// `Clear` resets the circuit and nothing about containment. The root and
    /// the include budget live on the executor precisely so this holds
    /// (GHSA-wg3j-3v62-fv3f); if they ever moved onto `RawDss`, a case could
    /// disarm the check by clearing first.
    #[test]
    fn clear_does_not_disarm_the_include_containment() {
        let requested: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Err::<String, _>(std::io::Error::new(std::io::ErrorKind::NotFound, "x"))
        };
        let raw = parse_raw_with_confined(
            "Redirect ../../secret.dss\nClear\nRedirect ../../secret.dss\nRedirect /etc/passwd",
            "/case/dir/master.dss",
            &mut loader,
        );
        assert!(
            requested.borrow().is_empty(),
            "an escaping include reached the loader after Clear: {:?}",
            requested.borrow()
        );
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the case directory"))
                .count(),
            3,
            "every refusal is stated, and Clear does not erase the ones before it"
        );
    }

    #[test]
    fn clear_resets() {
        let raw = parse("New Line.l1 length=1\nClear\nNew Line.l2 length=2");
        assert!(raw.find("line", "l1").is_none());
        assert!(raw.find("line", "l2").is_some());
    }

    #[test]
    fn block_comments_skip_lines() {
        let raw = parse("/* comment\nNew Line.l1 length=1\n*/\nNew Line.l2 length=2");
        assert!(raw.find("line", "l1").is_none());
        assert!(raw.find("line", "l2").is_some());
    }

    #[test]
    fn indented_block_comments_skip_lines() {
        let raw = parse("  /* comment\nNew Line.l1 length=1\n*/\nNew Line.l2 length=2");
        assert!(raw.find("line", "l1").is_none());
        assert!(raw.find("line", "l2").is_some());
    }

    #[test]
    fn one_line_block_comment() {
        let raw = parse("\t/* x */\nNew Line.l2 length=2");
        assert!(raw.find("line", "l2").is_some());
    }

    #[test]
    fn redirect_includes_a_file() {
        let mut files = BTreeMap::from([(
            PathBuf::from("sub/codes.dss"),
            "New Linecode.lc1 nphases=3".to_string(),
        )]);
        let mut loader = move |p: &Path| {
            files
                .remove(p)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
        };
        let raw = __parse_raw_with(
            "Redirect sub/codes.dss\nNew Line.l1 linecode=lc1",
            "test.dss",
            &mut loader,
        );
        assert!(raw.find("linecode", "lc1").is_some());
        assert_eq!(raw.warnings, [] as [std::string::String; 0]);
    }

    /// A loader that serves `text` for every path, gives up at `give_up`
    /// loads, and reports how many it served. Without the include budget the
    /// count runs away, so the test fails instead of hanging.
    fn counting_loader(text: &str, give_up: usize, script: &str) -> (RawDss, usize) {
        let mut loads = 0usize;
        let raw = {
            let mut loader = |_: &Path| {
                loads += 1;
                if loads > give_up {
                    Err(std::io::Error::other("loader gave up"))
                } else {
                    Ok(text.to_string())
                }
            };
            __parse_raw_with(script, "test.dss", &mut loader)
        };
        (raw, loads)
    }

    fn budget_refusals(raw: &RawDss) -> usize {
        raw.diagnostics
            .iter()
            .filter(|d| d.code() == crate::diagnostics::codes::READ_DSS_INCLUDE_BUDGET.code)
            .count()
    }

    #[test]
    fn a_self_redirecting_include_stops_at_the_file_budget() {
        // Two self redirects per file expand into a binary tree of depth
        // MAX_REDIRECT_DEPTH: ~2^65 script runs, which is why the depth limit
        // alone is not a bound.
        let (raw, loads) = counting_loader(
            "Redirect a.dss\nRedirect a.dss",
            MAX_TOTAL_INCLUDES * 4,
            "Redirect a.dss",
        );
        assert_eq!(loads, MAX_TOTAL_INCLUDES, "{loads} includes followed");
        assert_eq!(budget_refusals(&raw), 1, "one refusal, however many follow");
    }

    #[test]
    fn a_large_self_redirecting_include_stops_at_the_byte_budget() {
        // The same tree with a big file: the file budget would let it re-scan
        // MAX_TOTAL_INCLUDES copies of it, so the bytes are charged too. Each
        // load carries a quarter megabyte on one long line.
        let chunk = 256 << 10;
        let text = format!("Redirect a.dss\nRedirect a.dss\n// {}", "a".repeat(chunk));
        let (raw, loads) = counting_loader(&text, MAX_TOTAL_INCLUDES, "Redirect a.dss");
        assert!(
            loads <= MAX_TOTAL_INCLUDE_BYTES / chunk + 1,
            "{loads} includes followed, {} bytes",
            loads * chunk
        );
        assert!(
            loads < MAX_TOTAL_INCLUDES,
            "the byte budget is what stopped it"
        );
        assert_eq!(budget_refusals(&raw), 1);
    }

    #[test]
    fn buscoords_is_charged_against_the_include_budget() {
        // Buscoords does not recurse, but a redirect tree can load one per
        // node and every line of it lands in `raw.buscoords`. An uncharged
        // Buscoords would push the load count past the budget.
        let (raw, loads) = counting_loader(
            "Redirect a.dss\nRedirect a.dss\nBuscoords a.csv",
            MAX_TOTAL_INCLUDES * 4,
            "Redirect a.dss",
        );
        assert_eq!(loads, MAX_TOTAL_INCLUDES, "{loads} includes followed");
        assert_eq!(budget_refusals(&raw), 1);
    }

    #[test]
    fn clear_does_not_refund_the_include_budget() {
        // `Clear` resets the parsed script, so a budget counted in RawDss
        // would reset with it and the tree would run unbounded again.
        let (_, loads) = counting_loader(
            "Clear\nRedirect a.dss\nRedirect a.dss",
            MAX_TOTAL_INCLUDES * 4,
            "Redirect a.dss",
        );
        assert_eq!(loads, MAX_TOTAL_INCLUDES, "{loads} includes followed");
    }

    #[test]
    fn missing_redirect_warns() {
        let raw = parse("Redirect nope.dss");
        assert_eq!(raw.warnings.len(), 1);
        assert!(raw.warnings[0].contains("nope.dss"));
    }

    #[test]
    fn compile_moves_the_directory_redirect_restores_it() {
        // After `Compile sub/feeder.dss`, the caller's relative paths
        // resolve against sub/; after a Redirect they resolve against the
        // caller's own directory again. Both directories carry a lines.dss
        // so the wrong resolution shows up as the wrong object.
        let root = std::env::temp_dir().join(format!("powerio-dist-raw-{}", std::process::id()));
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("feeder.dss"), "New Linecode.lc1 nphases=3").unwrap();
        std::fs::write(sub.join("lines.dss"), "New Line.fromsub bus1=a").unwrap();
        std::fs::write(root.join("lines.dss"), "New Line.fromroot bus1=a").unwrap();
        std::fs::write(
            root.join("compile.dss"),
            "Compile sub/feeder.dss\nRedirect lines.dss",
        )
        .unwrap();
        std::fs::write(
            root.join("redirect.dss"),
            "Redirect sub/feeder.dss\nRedirect lines.dss",
        )
        .unwrap();

        let compiled = __parse_raw_file(root.join("compile.dss")).unwrap();
        assert_eq!(compiled.warnings, Vec::<String>::new());
        assert!(compiled.find("line", "fromsub").is_some());

        let redirected = __parse_raw_file(root.join("redirect.dss")).unwrap();
        assert_eq!(redirected.warnings, Vec::<String>::new());
        assert!(redirected.find("line", "fromroot").is_some());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn compile_inside_compile_pins_the_compiled_files_directory() {
        // ExecHelper DoRedirect sets CurrDir from the file path once at
        // entry and compile exit reapplies it (SetDataPath → ChDir), so a
        // Compile that itself compiles deeper still leaves the caller in
        // the directly compiled file's directory, not the innermost one.
        // probe.dss exists in both sub/ and sub/inner/; the engine resolves
        // sub/probe.dss.
        let root =
            std::env::temp_dir().join(format!("powerio-dist-rawnest-{}", std::process::id()));
        let sub = root.join("sub");
        let inner = sub.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(
            root.join("main.dss"),
            "Compile sub/a.dss\nRedirect probe.dss",
        )
        .unwrap();
        std::fs::write(sub.join("a.dss"), "Compile inner/b.dss").unwrap();
        std::fs::write(inner.join("b.dss"), "New Linecode.lc1 nphases=1").unwrap();
        std::fs::write(sub.join("probe.dss"), "New Line.fromsub bus1=a").unwrap();
        std::fs::write(inner.join("probe.dss"), "New Line.frominner bus1=a").unwrap();

        let raw = __parse_raw_file(root.join("main.dss")).unwrap();
        assert_eq!(raw.warnings, Vec::<String>::new());
        assert!(raw.find("linecode", "lc1").is_some());
        assert!(raw.find("line", "fromsub").is_some());
        assert!(raw.find("line", "frominner").is_none());

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn edit_boundaries_are_recorded() {
        // One checkpoint per command line; like= splices the source's
        // boundaries (offset) before the splicing edit's own.
        let raw = parse("New Load.a kW=10 pf=0.9\n~ kvar=5\nNew Load.b like=a kw=20");
        let a = raw.find("load", "a").unwrap();
        assert_eq!(a.edits, vec![2, 3]);
        let b = raw.find("load", "b").unwrap();
        assert_eq!(b.props.len(), 4);
        assert_eq!(b.edits, vec![2, 3, 4]);
        assert_eq!(b.edit_bounds().collect::<Vec<_>>(), vec![2, 3, 4]);
    }

    #[test]
    fn var_definition_and_use() {
        let raw = parse("var @kv=12.47\nNew Load.ld kv=@kv");
        let ld = raw.find("load", "ld").unwrap();
        assert_eq!(ld.get("kv").unwrap().text, "12.47");
    }

    #[test]
    fn quoted_var_value_stays_rpn() {
        // The braces TParserVar::Add wraps around the stored value come
        // back off as a quoted token, so the substituted expression still
        // evaluates as RPN.
        let raw = parse("var @z=(8 1000 /)\nNew Load.ld kW=@z");
        let v = raw.find("load", "ld").unwrap().get("kw").unwrap();
        assert!(v.quoted);
        assert_eq!(v.to_f64(None), Ok(0.008));
    }

    #[test]
    fn vars_cross_redirect_boundaries() {
        // A var defined in the parent substitutes inside the include, and a
        // var defined in the include survives back in the parent.
        let mut loader = |p: &Path| {
            if p == Path::new("inc.dss") {
                Ok("New Load.inner kv=@kv\nvar @kw=42".to_string())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            }
        };
        let raw = __parse_raw_with(
            "var @kv=12.47\nRedirect inc.dss\nNew Load.outer kW=@kw",
            "test.dss",
            &mut loader,
        );
        assert_eq!(raw.warnings, Vec::<String>::new());
        assert_eq!(
            raw.find("load", "inner").unwrap().get("kv").unwrap().text,
            "12.47"
        );
        assert_eq!(
            raw.find("load", "outer").unwrap().get("kw").unwrap().text,
            "42"
        );
    }

    #[test]
    fn var_defined_after_clear_does_not_see_the_pre_clear_table() {
        // Without the generation guard, `run_command`'s restore after
        // dispatching `Clear` would resurrect the pre-`Clear` var snapshot
        // over the empty table `clear` just installed.
        let raw = parse("var @a=1\nClear\nvar @b=2\nNew Load.ld kv=@a kw=@b");
        let ld = raw.find("load", "ld").unwrap();
        assert_eq!(ld.get("kw").unwrap().text, "2", "post-clear var resolves");
        assert_eq!(
            ld.get("kv").unwrap().text,
            "@a",
            "pre-clear var does not survive Clear"
        );
    }

    #[test]
    fn the_maximum_permitted_var_count_parses() {
        use std::fmt::Write as _;
        let mut script = String::new();
        for i in 0..MAX_TOTAL_VARS {
            let _ = writeln!(script, "var @v{i}={i}");
        }
        script.push_str("New Load.ld kv=@v0\n");
        let raw = parse(&script);
        assert_eq!(raw.warnings, Vec::<String>::new());
        assert_eq!(raw.find("load", "ld").unwrap().get("kv").unwrap().text, "0");
    }

    #[test]
    fn one_var_past_the_count_cap_is_refused() {
        use std::fmt::Write as _;
        let mut script = String::new();
        for i in 0..=MAX_TOTAL_VARS {
            let _ = writeln!(script, "var @v{i}={i}");
        }
        let _ = writeln!(script, "New Load.ld kv=@v{MAX_TOTAL_VARS}");
        let raw = parse(&script);
        assert_eq!(
            raw.diagnostics
                .iter()
                .filter(|d| d.code() == crate::diagnostics::codes::READ_DSS_VALUE_CLAMPED.code)
                .count(),
            1,
            "one refusal, however many follow"
        );
        // The var one past the cap was refused, so it never resolves.
        assert_eq!(
            raw.find("load", "ld").unwrap().get("kv").unwrap().text,
            format!("@v{MAX_TOTAL_VARS}")
        );
    }

    #[test]
    fn one_byte_past_the_var_byte_cap_is_refused() {
        // One var sized to fill the byte budget, then one more small var
        // that must be refused for lack of budget rather than count.
        let filler = "x".repeat(MAX_TOTAL_VAR_BYTES);
        let raw = parse(&format!("var @big={filler}\nvar @small=1\n"));
        assert_eq!(
            raw.diagnostics
                .iter()
                .filter(|d| d.code() == crate::diagnostics::codes::READ_DSS_VALUE_CLAMPED.code)
                .count(),
            1,
            "one refusal, however many follow"
        );
    }

    #[test]
    fn duplicate_new_warns_and_edits() {
        let raw = parse("New Line.l1 length=1\nNew Line.l1 length=2");
        assert_eq!(raw.warnings.len(), 1);
        assert_eq!(
            raw.find("line", "l1").unwrap().get("length").unwrap().text,
            "2"
        );
    }

    #[test]
    fn rpn_value_via_props() {
        let raw = parse("New Load.ld kW=(8 1000 /)");
        let v = raw.find("load", "ld").unwrap().get("kw").unwrap().clone();
        assert_eq!(v.to_f64(None), Ok(0.008));
    }

    #[test]
    fn confined_parsing_refuses_includes_outside_the_case_directory() {
        use std::cell::RefCell;
        // Records every path the loader is actually asked to read, so we can
        // assert a refused include never reaches the filesystem.
        let requested: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Err::<String, _>(std::io::Error::new(std::io::ErrorKind::NotFound, "x"))
        };
        let raw = parse_raw_with_confined(
            "Redirect ../../secret.dss\nRedirect /etc/passwd\nBuscoords ../up.csv",
            "/case/dir/master.dss",
            &mut loader,
        );
        assert!(
            requested.borrow().is_empty(),
            "escaping include reached the loader: {:?}",
            requested.borrow()
        );
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the case directory"))
                .count(),
            3
        );
        // Each refusal is also an Error-severity finding (#275): the parse
        // continued, but the network is incomplete.
        let refused: Vec<_> = raw
            .diagnostics
            .iter()
            .filter(|d| d.code() == crate::diagnostics::codes::READ_DSS_INCLUDE_REFUSED.code)
            .collect();
        assert_eq!(refused.len(), 3);
        assert!(
            refused
                .iter()
                .all(|d| d.severity() == crate::diagnostics::DiagnosticSeverity::Error)
        );
    }

    #[test]
    fn confined_parsing_with_an_empty_root_refuses_absolute_and_climbing_includes() {
        use std::cell::RefCell;
        // A bare filename ("master.dss") has an empty parent, so the
        // confinement root is empty. `starts_with("")` holds for every path,
        // so containment must come from the component rule instead: only
        // plain relative includes stay inside the working directory.
        let requested: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Err::<String, _>(std::io::Error::new(std::io::ErrorKind::NotFound, "x"))
        };
        let raw = parse_raw_with_confined(
            "Redirect /etc/passwd\nRedirect ../secret.dss\nRedirect sub/ok.dss",
            "master.dss",
            &mut loader,
        );
        assert_eq!(
            *requested.borrow(),
            vec!["sub/ok.dss".to_string()],
            "an absolute or climbing include reached the loader"
        );
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the case directory"))
                .count(),
            2
        );
    }

    #[test]
    fn confined_parsing_allows_includes_under_a_root_that_starts_with_parent_dirs() {
        use std::cell::RefCell;
        // The case path itself may climb ("../case/master.dss"); includes
        // under that same directory are inside the case and must load.
        let requested: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Ok(String::new())
        };
        let raw = parse_raw_with_confined(
            "Redirect codes.dss\nRedirect ../../outside.dss",
            "../case/master.dss",
            &mut loader,
        );
        assert_eq!(*requested.borrow(), vec!["../case/codes.dss".to_string()]);
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the case directory"))
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_parsing_refuses_includes_that_escape_through_a_symlink() {
        // A lexically contained include that is really a symlink out of the
        // case directory must not be read.
        let root =
            std::env::temp_dir().join(format!("powerio-dist-symlink-{}", std::process::id()));
        let case = root.join("case");
        std::fs::create_dir_all(&case).unwrap();
        std::fs::write(root.join("secret.dss"), "New Line.leaked bus1=a").unwrap();
        std::fs::write(case.join("master.dss"), "Redirect linked.dss").unwrap();
        std::os::unix::fs::symlink(root.join("secret.dss"), case.join("linked.dss")).unwrap();

        let raw = __parse_raw_file(case.join("master.dss")).unwrap();
        assert!(raw.find("line", "leaked").is_none());
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("symbolic link"))
                .count(),
            1,
            "warnings: {:?}",
            raw.warnings
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn raw_file_parsing_refuses_includes_outside_the_case_directory() {
        let root =
            std::env::temp_dir().join(format!("powerio-dist-rawconf-{}", std::process::id()));
        std::fs::create_dir_all(root.join("case")).unwrap();
        std::fs::write(root.join("secret.dss"), "New Line.leaked bus1=a").unwrap();
        std::fs::write(
            root.join("case").join("master.dss"),
            "Redirect ../secret.dss",
        )
        .unwrap();

        let raw = __parse_raw_file(root.join("case").join("master.dss")).unwrap();
        assert!(raw.find("line", "leaked").is_none());
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the case directory"))
                .count(),
            1
        );

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn confined_parsing_allows_includes_within_the_case_directory() {
        use std::cell::RefCell;
        let requested: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Ok(String::new())
        };
        // A subdirectory include and an absolute path inside the root both load.
        parse_raw_with_confined(
            "Redirect sub/codes.dss\nRedirect /case/dir/abs.dss",
            "/case/dir/master.dss",
            &mut loader,
        );
        assert_eq!(
            *requested.borrow(),
            vec![
                "/case/dir/sub/codes.dss".to_string(),
                "/case/dir/abs.dss".to_string(),
            ]
        );
    }

    #[test]
    fn a_widened_include_root_admits_siblings_and_refuses_escapes_past_it() {
        use std::cell::RefCell;
        let requested: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let mut loader = |p: &Path| {
            requested.borrow_mut().push(p.display().to_string());
            Ok(String::new())
        };
        // The shared sibling directory is inside the include root; a climb
        // past the root is refused with the root named as the boundary.
        let raw = parse_raw_confined_under(
            "Redirect ../shared/codes.dss\nRedirect ../../outside.dss",
            "/root/feeder/master.dss",
            Path::new("/root"),
            &mut loader,
        );
        assert_eq!(
            *requested.borrow(),
            vec!["/root/shared/codes.dss".to_string()]
        );
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("escapes the include root"))
                .count(),
            1,
            "warnings: {:?}",
            raw.warnings
        );
    }
}
