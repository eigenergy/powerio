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
    /// The reader's findings as `CODE: message` lines, rendered from
    /// `diagnostics` so the text and the structure cannot disagree.
    pub warnings: Vec<String>,
    /// Structured findings beside `warnings`; an `Error` entry marks an
    /// incomplete parse the CLI must not exit 0 on.
    pub diagnostics: Vec<crate::diagnostics::Diagnostic>,
    index: BTreeMap<(String, String), usize>,
    active: Option<usize>,
}

impl RawDss {
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
    /// from the record it is added with.
    fn record(&mut self, diagnostic: crate::diagnostics::Diagnostic) {
        self.warnings
            .push(crate::diagnostics::render_diagnostic(&diagnostic));
        self.diagnostics.push(diagnostic);
    }

    fn clear(&mut self) {
        // `Clear` resets the circuit, not the record of how the script was
        // read. A warning or an `Error` finding already emitted describes input
        // the caller wrote, and no later command un-writes it — dropping them
        // here returned a network that had refused an include, or skipped one
        // that escaped the case directory, with an empty `warnings` and nothing
        // for `powerio package` to lift into a finding.
        let (warnings, diagnostics) = (
            std::mem::take(&mut self.warnings),
            std::mem::take(&mut self.diagnostics),
        );
        *self = RawDss {
            warnings,
            diagnostics,
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

/// Script text a single parse may pull in through includes. The include count
/// alone still admits amplification: one large file that redirects to itself is
/// re-executed once per load, so a few megabytes of input buy thousands of
/// times that in scanning. Together the two budgets keep an include tree
/// costing about what a single case file of this size costs. The root file is
/// not charged against it, so a case without includes is never truncated.
const MAX_TOTAL_INCLUDE_BYTES: usize = 64 << 20;

/// Cap on the accumulated property assignments a single object may hold.
/// `like=` splices the source object's whole prop list, so a self-referencing
/// or mutually-referencing chain (`Edit Load.a like=a` repeated) doubles the
/// count each edit — a few hundred bytes could otherwise reach memory
/// exhaustion. No real object comes near this bound: the largest DSS class has
/// on the order of 100 properties and legitimate scripts edit an object a
/// handful of times.
const MAX_OBJECT_PROPS: usize = 1 << 16;

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
            self.run_command(line, file, line_no);
        }
    }

    fn run_command(&mut self, line: &str, file: &str, line_no: usize) {
        // The scanner substitutes against a snapshot of the var table so the
        // live table stays free for mutation: `var` inserts into it directly
        // and redirected files both see and extend it. The snapshot only
        // diverges for a self referencing `var` line, which OpenDSS scripts
        // do not write.
        let vars = self.raw.vars.clone();
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
    }

    fn dispatch(&mut self, verb: String, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        match command_index(&verb).map(|i| COMMANDS[i]) {
            Some("new") => self.do_new(scan, ctx),
            Some("edit") => self.do_edit(scan, ctx),
            Some("more" | "m" | "~") => self.do_more(scan, ctx),
            Some("select") => self.do_select(scan, ctx),
            Some("set") => self.do_set(scan),
            Some("redirect") => self.do_redirect(scan, false, ctx),
            Some("compile") => self.do_redirect(scan, true, ctx),
            Some("buscoords") => self.do_buscoords(scan, ctx),
            Some("setbusxy") => self.do_setbusxy(scan, ctx),
            Some("var") => self.do_var(scan),
            Some("clear" | "clearall") => self.raw.clear(),
            Some("//") => {}
            Some(canonical) => {
                self.raw.commands.push(RawCommand {
                    verb: canonical.to_string(),
                    args: scan.remainder().to_string(),
                });
            }
            None => {
                self.raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!("unknown command `{verb}`; line preserved verbatim")),
                );
                self.raw.commands.push(RawCommand {
                    verb,
                    args: scan.remainder().to_string(),
                });
            }
        }
    }

    /// `var @name=value ...` defines parser variables. TParserVar::Add
    /// stores every value brace wrapped unless it begins with `@`;
    /// CheckforVar unwraps the braces into a quoted token, so a definition
    /// like `var @z=(8 1000 /)` still evaluates as RPN where it is used.
    fn do_var(&mut self, scan: &mut Scanner) {
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
                self.raw.vars.insert(name.to_ascii_lowercase(), stored);
            }
        }
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
        while let Some(p) = scan.next_param() {
            if p.value.text.is_empty() && p.name.is_none() {
                break;
            }
            let name = p.name.unwrap_or_default().to_ascii_lowercase();
            self.raw.options.push((name, p.value));
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
                self.run_script(&text, &path.display().to_string());
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
                    let mut s = Scanner::new(line, None);
                    let Some(bus) = s.next_param() else { continue };
                    if bus.value.text.is_empty() {
                        continue;
                    }
                    let x = s.next_param().map(|p| p.value).unwrap_or_default();
                    let y = s.next_param().map(|p| p.value).unwrap_or_default();
                    match (x.to_f64(None), y.to_f64(None)) {
                        (Ok(x), Ok(y)) => self.raw.buscoords.push(BusCoord {
                            bus: bus.value.text,
                            x,
                            y,
                        }),
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
                self.raw.buscoords.push(BusCoord { bus, x, y });
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
        if let Some(name) = &p.name {
            if !name.eq_ignore_ascii_case("object") {
                self.raw.warn(
                    &C::PARSE_DSS_SOURCE_MALFORMED,
                    ctx(format!("expected Class.Name, got `{name}=`")),
                );
                return None;
            }
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

    fn apply_props(&mut self, idx: usize, props: Vec<RawProp>, ctx: &dyn Fn(String) -> String) {
        self.raw.active = Some(idx);
        for p in props {
            // `like=<name>` splices the source object's accumulated props,
            // checkpoints included: MakeLike copies the source's recalced
            // state (Load.cpp ~810-815 takes kWBase, kvarBase, LoadSpecType,
            // AND PFNominal), which equals replaying the source's writes
            // with its own edit boundaries.
            if p.name.as_deref() == Some("like") {
                let class = self.raw.objects[idx].class.clone();
                let key = (class.clone(), p.value.text.to_ascii_lowercase());
                match self.raw.index.get(&key).copied() {
                    Some(src) => {
                        let base = self.raw.objects[idx].props.len();
                        let src_len = self.raw.objects[src].props.len();
                        // Refuse a splice that would push the object past the
                        // cap. A self reference (`Edit X like=X`) or a mutual
                        // chain otherwise doubles the prop count per edit; the
                        // guard turns that into a warning instead of unbounded
                        // growth.
                        if base.saturating_add(src_len) > MAX_OBJECT_PROPS {
                            self.raw.warn(
                                &C::READ_DSS_VALUE_CLAMPED,
                                ctx(format!(
                                    "like={}: {class} property count would exceed the supported \
                                 maximum of {MAX_OBJECT_PROPS}; splice refused",
                                    p.value.text
                                )),
                            );
                            continue;
                        }
                        let cloned = self.raw.objects[src].props.clone();
                        let bounds: Vec<usize> = self.raw.objects[src]
                            .edit_bounds()
                            .map(|e| base + e)
                            .collect();
                        self.raw.objects[idx].props.extend(cloned);
                        self.raw.objects[idx].edits.extend(bounds);
                    }
                    None => self.raw.warn(
                        &C::READ_DSS_REFERENCE_DROPPED,
                        ctx(format!("like={} names an unknown {class}", p.value.text)),
                    ),
                }
                continue;
            }
            if self.raw.objects[idx].props.len() >= MAX_OBJECT_PROPS {
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
            self.raw.objects[idx].props.push(p);
        }
        // This command line was one engine Edit; it ends in
        // RecalcElementData, so record the boundary.
        let end = self.raw.objects[idx].props.len();
        self.raw.objects[idx].edits.push(end);
    }
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
/// [`parse_dss_str`](crate::dss::parse_dss_str) (no filesystem access) or
/// [`parse_dss_file`](crate::dss::parse_dss_file) / [`parse_raw_file`]
/// (includes confined to the case directory), or enforce your own containment
/// inside the loader.
pub fn parse_raw_with(text: &str, path: &str, loader: &mut impl Loader) -> RawDss {
    run_executor(text, path, None, IncludeBoundary::CaseDirectory, loader)
}

/// The case directory of `path` in canonical (symlink resolved) form, for
/// checking filesystem reads against the lexical confinement root. `None`
/// when canonicalization fails (directory missing or unreadable), which the
/// confined filesystem reader treats as "refuse every include".
pub(crate) fn canonical_case_root(path: &Path) -> Option<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new(""));
    let dir = if dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        dir
    };
    dir.canonicalize().ok()
}

/// Reads an include for confined file parsing. The executor's lexical check
/// already ran; this closes the symlink hole it cannot see: the path is
/// canonicalized (resolving symlinks) and refused unless the real file still
/// sits under the confinement boundary's canonical root. The refusal surfaces
/// through the executor's ordinary load-error warning.
pub(crate) fn confined_fs_read(
    canonical_root: Option<&Path>,
    boundary: IncludeBoundary,
    path: &Path,
) -> std::io::Result<String> {
    let Some(root) = canonical_root else {
        return Err(Containment::refused(match boundary {
            IncludeBoundary::CaseDirectory => {
                "case directory cannot be resolved; includes are disabled"
            }
            IncludeBoundary::IncludeRoot => {
                "include root cannot be resolved; includes are disabled"
            }
        }));
    };
    let real = path.canonicalize()?;
    if !real.starts_with(root) {
        return Err(Containment::refused(match boundary {
            IncludeBoundary::CaseDirectory => {
                "resolves outside the case directory through a symbolic link"
            }
            IncludeBoundary::IncludeRoot => {
                "resolves outside the include root through a symbolic link"
            }
        }));
    }
    std::fs::read_to_string(&real)
}

/// The loader's own containment refusal, carried inside the `io::Error` so it
/// stays distinguishable from a `PermissionDenied` the filesystem raised. A
/// mode 000 file inside the case directory is an ordinary unreadable include,
/// not an escape attempt, and must not be reported as one.
#[derive(Debug)]
pub(crate) struct Containment(&'static str);

impl Containment {
    fn refused(reason: &'static str) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, Containment(reason))
    }

    /// Whether `e` is a refusal this module raised.
    pub(crate) fn refused_by_us(e: &std::io::Error) -> bool {
        e.get_ref()
            .is_some_and(<dyn std::error::Error + Send + Sync>::is::<Self>)
    }
}

impl std::fmt::Display for Containment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Containment {}

/// Like [`parse_raw_with`], but confines `Redirect`/`Compile`/`Buscoords`
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
    };
    exec.run_script(text, path);
    exec.raw
}

/// Parses a `.dss` file from disk, following its includes.
/// `Redirect`/`Compile`/`Buscoords` includes are confined to the case
/// directory, lexically and after symlink resolution, exactly like
/// [`parse_dss_file`](crate::dss::parse_dss_file); an include that escapes is
/// refused with a warning. Use [`parse_raw_with`] with your own loader for
/// unconfined resolution.
pub fn parse_raw_file(path: impl AsRef<Path>) -> Result<RawDss> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    let root = canonical_case_root(path);
    Ok(parse_raw_with_confined(
        &text,
        &path.display().to_string(),
        &mut |p: &Path| confined_fs_read(root.as_deref(), IncludeBoundary::CaseDirectory, p),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_files(_: &Path) -> std::io::Result<String> {
        Err(std::io::Error::new(std::io::ErrorKind::NotFound, "test"))
    }

    fn parse(text: &str) -> RawDss {
        parse_raw_with(text, "test.dss", &mut no_files)
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
        assert!(raw.warnings.is_empty());
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
        assert!(raw.warnings.is_empty());
    }

    #[test]
    fn bare_property_edits_the_active_object() {
        let raw = parse("New Line.l1 bus1=a bus2=b\nlength=2.5");
        let l = raw.find("line", "l1").unwrap();
        assert_eq!(l.get("length").unwrap().text, "2.5");
        assert!(raw.warnings.is_empty());
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
        assert!(raw.warnings.is_empty());
    }

    #[test]
    fn like_splices_source_props() {
        let raw = parse("New Load.a kW=10 pf=0.9\nNew Load.b like=a kW=20");
        let b = raw.find("load", "b").unwrap();
        assert_eq!(b.get("kw").unwrap().text, "20");
        assert_eq!(b.get("pf").unwrap().text, "0.9");
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
        let raw = parse_raw_with(
            "Redirect sub/codes.dss\nNew Line.l1 linecode=lc1",
            "test.dss",
            &mut loader,
        );
        assert!(raw.find("linecode", "lc1").is_some());
        assert!(raw.warnings.is_empty());
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
            parse_raw_with(script, "test.dss", &mut loader)
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

        let compiled = parse_raw_file(root.join("compile.dss")).unwrap();
        assert_eq!(compiled.warnings, Vec::<String>::new());
        assert!(compiled.find("line", "fromsub").is_some());

        let redirected = parse_raw_file(root.join("redirect.dss")).unwrap();
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

        let raw = parse_raw_file(root.join("main.dss")).unwrap();
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
        let raw = parse_raw_with(
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

        let raw = parse_raw_file(case.join("master.dss")).unwrap();
        assert!(raw.find("line", "leaked").is_none());
        assert_eq!(
            raw.warnings
                .iter()
                .filter(|w| w.contains("outside the case directory"))
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

        let raw = parse_raw_file(root.join("case").join("master.dss")).unwrap();
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
