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
    pub warnings: Vec<String>,
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

    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    fn clear(&mut self) {
        *self = RawDss::default();
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

struct Executor<'l, L: Loader> {
    raw: RawDss,
    loader: &'l mut L,
    /// Directory stack for relative include resolution; starts with the
    /// root file's directory, so its depth is the redirect nesting level.
    dirs: Vec<PathBuf>,
}

/// Splits script text into command lines, dropping block comments. A block
/// comment starts on a line whose first character is `/` followed by `*`
/// and ends on the first line containing `*/`; both boundary lines are
/// consumed whole, matching the OpenDSS executive.
fn command_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut in_block = false;
    text.lines().enumerate().filter_map(move |(i, line)| {
        if in_block {
            if line.contains("*/") {
                in_block = false;
            }
            return None;
        }
        if line.starts_with("/*") {
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
                self.raw.warn(ctx(format!(
                    "unknown command `{verb}`; line preserved verbatim"
                )));
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
                raw.warn(ctx(format!("`{spec}=` with no active object")));
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
                    self.raw.warn(ctx(format!(
                        "property reference to unknown object `{class}.{name}`"
                    )));
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
                    self.raw.warn(ctx(format!(
                        "unknown property `{prop}` on {}; kept as written",
                        c.name
                    )));
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
            &mut self.raw.warnings,
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
                self.raw.warn(ctx(format!(
                    "duplicate `New {class}.{name}`; editing the existing object"
                )));
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
            self.raw
                .warn(ctx(format!("`Edit {class}.{name}` on an unknown object")));
            return;
        };
        self.consume_and_apply(idx, scan, ctx);
    }

    fn do_more(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some(idx) = self.raw.active else {
            self.raw.warn(ctx("`~` with no active object".into()));
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
            None => self
                .raw
                .warn(ctx(format!("`Select {class}.{name}` on an unknown object"))),
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
    /// Backslash separators (the format's DOS heritage) become `/`.
    fn resolve(&self, file_arg: &str) -> PathBuf {
        let rel = file_arg.replace('\\', "/");
        self.dirs
            .last()
            .map_or_else(|| PathBuf::from(&rel), |d| d.join(&rel))
    }

    fn do_redirect(&mut self, scan: &mut Scanner, compile: bool, ctx: &dyn Fn(String) -> String) {
        let Some(p) = scan.next_param() else {
            self.raw.warn(ctx("redirect with no file".into()));
            return;
        };
        let path = self.resolve(&p.value.text);
        if self.dirs.len() > MAX_REDIRECT_DEPTH {
            self.raw
                .warn(ctx(format!("redirect depth limit at {}", path.display())));
            return;
        }
        match self.loader.load(&path) {
            Ok(text) => {
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                self.dirs.push(dir);
                self.run_script(&text, &path.display().to_string());
                // The engine keeps one current directory: Redirect restores
                // the caller's on return, Compile leaves it wherever the
                // compiled script ended (ExecHelper DoRedirect restores
                // SaveDir only when not compiling), so the caller's later
                // relative paths follow the compiled file.
                let ended = self.dirs.pop().unwrap_or_default();
                if compile && let Some(top) = self.dirs.last_mut() {
                    *top = ended;
                }
            }
            Err(e) => {
                let verb = if compile { "compile" } else { "redirect" };
                self.raw
                    .warn(ctx(format!("{verb} {}: {e}", path.display())));
            }
        }
    }

    fn do_buscoords(&mut self, scan: &mut Scanner, ctx: &dyn Fn(String) -> String) {
        let Some(p) = scan.next_param() else {
            self.raw.warn(ctx("buscoords with no file".into()));
            return;
        };
        let path = self.resolve(&p.value.text);
        match self.loader.load(&path) {
            Ok(text) => {
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
                        _ => self.raw.warn(ctx(format!(
                            "buscoords {}:{}: unparseable coordinates",
                            path.display(),
                            line_no + 1
                        ))),
                    }
                }
            }
            Err(e) => self
                .raw
                .warn(ctx(format!("buscoords {}: {e}", path.display()))),
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
                self.raw
                    .warn(ctx(format!("expected Class.Name, got `{name}=`")));
                return None;
            }
        }
        let spec = p.value.text;
        match spec.split_once('.') {
            Some((class, name)) if !class.is_empty() && !name.is_empty() => {
                Some((class.to_string(), name.to_string()))
            }
            _ => {
                self.raw
                    .warn(ctx(format!("malformed object spec `{spec}`")));
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
            &mut self.raw.warnings,
            ctx,
        );
        self.apply_props(idx, props, ctx);
    }

    fn apply_props(&mut self, idx: usize, props: Vec<RawProp>, ctx: &dyn Fn(String) -> String) {
        self.raw.active = Some(idx);
        for p in props {
            // `like=<name>` splices the source object's accumulated props.
            if p.name.as_deref() == Some("like") {
                let class = self.raw.objects[idx].class.clone();
                let key = (class.clone(), p.value.text.to_ascii_lowercase());
                match self.raw.index.get(&key).copied() {
                    Some(src) => {
                        let cloned = self.raw.objects[src].props.clone();
                        self.raw.objects[idx].props.extend(cloned);
                    }
                    None => self.raw.warn(ctx(format!(
                        "like={} names an unknown {class}",
                        p.value.text
                    ))),
                }
                continue;
            }
            self.raw.objects[idx].props.push(p);
        }
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
    warnings: &mut Vec<String>,
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
                    warnings.push(ctx(format!(
                        "unknown property `{written}` on {}; kept as written",
                        c.name
                    )));
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
                    warnings.push(ctx(format!(
                        "positional value `{}` beyond the last {} property",
                        p.value.text, c.name
                    )));
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
pub fn parse_raw_with(text: &str, path: &str, loader: &mut impl Loader) -> RawDss {
    let mut exec = Executor {
        raw: RawDss::default(),
        loader,
        dirs: vec![
            Path::new(path)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
        ],
    };
    exec.run_script(text, path);
    exec.raw
}

/// Parses a `.dss` file from disk, following its includes.
pub fn parse_raw_file(path: impl AsRef<Path>) -> Result<RawDss> {
    let path = path.as_ref();
    let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(parse_raw_with(
        &text,
        &path.display().to_string(),
        &mut |p: &Path| std::fs::read_to_string(p),
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
    fn one_line_block_comment() {
        let raw = parse("/* x */\nNew Line.l2 length=2");
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
}
