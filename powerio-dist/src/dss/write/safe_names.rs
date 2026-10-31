//! Names the dss tokenizer cannot carry, replaced before writing.
//!
//! Other formats allow free text names (a CIM `IdentifiedObject.name` such
//! as `Cable type A`), but dss splits a token on whitespace and its
//! delimiters, reads `!` and `//` as comment openers, and reads a `.` in a
//! bus reference as the node separator. A deck that spells such a name
//! verbatim does not parse back. The writer instead works on a copy of the
//! network in which each such name, and every typed reference to it, is
//! replaced by a name dss can carry: each offending character becomes `_`,
//! with a numeric suffix when that name is already taken in the same dss
//! class. Each replacement is reported.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};

use crate::diagnostics::codes as C;
use crate::model::MulticonductorNetwork;

use super::name_breaks_dss;

/// `name` with each character [`name_breaks_dss`] rejects replaced by `_`.
fn substitute(name: &str, is_bus_id: bool) -> String {
    name.replace("//", "__")
        .chars()
        .map(|c| {
            let breaks = c.is_whitespace()
                || matches!(
                    c,
                    ',' | '=' | '!' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}'
                )
                || (is_bus_id && c == '.');
            if breaks { '_' } else { c }
        })
        .collect()
}

/// The replacements of one dss name scope (a class, or the bus ids), by the
/// lower case original name: dss names are case insensitive.
struct Scope {
    label: &'static str,
    is_bus_id: bool,
    renamed: BTreeMap<String, String>,
}

impl Scope {
    /// Plan the replacements for `names`, the names one dss class holds.
    fn plan<'a>(
        label: &'static str,
        is_bus_id: bool,
        names: impl Iterator<Item = &'a str> + Clone,
    ) -> Self {
        let mut taken: HashSet<String> = names
            .clone()
            .filter(|name| !name_breaks_dss(name, is_bus_id))
            .map(str::to_ascii_lowercase)
            .collect();
        let mut renamed = BTreeMap::new();
        for name in names {
            let key = name.to_ascii_lowercase();
            if !name_breaks_dss(name, is_bus_id) || renamed.contains_key(&key) {
                continue;
            }
            let base = substitute(name, is_bus_id);
            let mut candidate = base.clone();
            let mut n = 1;
            while !taken.insert(candidate.to_ascii_lowercase()) {
                n += 1;
                candidate = format!("{base}_{n}");
            }
            renamed.insert(key, candidate);
        }
        Self {
            label,
            is_bus_id,
            renamed,
        }
    }

    /// Replace `name` in place when the scope renames it.
    fn apply(&self, name: &mut String) {
        if let Some(new) = self.renamed.get(&name.to_ascii_lowercase()) {
            new.clone_into(name);
        }
    }
}

/// The replacements for each dss name scope of one network.
struct Renames {
    buses: Scope,
    line_codes: Scope,
    lines: Scope,
    transformers: Scope,
    loads: Scope,
    generators: Scope,
    ibrs: Scope,
    profiles: Scope,
    shunts: Scope,
    sources: Scope,
    circuit: Scope,
}

impl Renames {
    fn plan(net: &MulticonductorNetwork) -> Self {
        let buses = Scope::plan("bus", true, net.buses().iter().map(|b| b.id.as_str()));
        let line_codes = Scope::plan(
            "linecode",
            false,
            net.line_codes().iter().map(|c| c.name.as_str()),
        );
        // Lines and switches both write as dss `Line` objects.
        let lines = Scope::plan(
            "line",
            false,
            net.lines()
                .iter()
                .map(|l| l.name.as_str())
                .chain(net.switches().iter().map(|s| s.name.as_str())),
        );
        let transformers = Scope::plan(
            "transformer",
            false,
            net.transformers().iter().map(|t| t.name.as_str()),
        );
        let loads = Scope::plan("load", false, net.loads().iter().map(|l| l.name.as_str()));
        let generators = Scope::plan(
            "generator",
            false,
            net.generators().iter().map(|g| g.name.as_str()),
        );
        let ibrs = Scope::plan("ibr", false, net.ibrs().iter().map(|i| i.name.as_str()));
        let profiles = Scope::plan(
            "control profile",
            false,
            net.control_profiles().iter().map(|p| p.name.as_str()),
        );
        // Shunts and capacitors can both write as dss `Capacitor` objects.
        let shunts = Scope::plan(
            "shunt",
            false,
            net.shunts()
                .iter()
                .map(|s| s.name.as_str())
                .chain(net.capacitors().iter().map(|c| c.name.as_str())),
        );
        // The network name names the dss circuit.
        let circuit = Scope::plan("circuit", false, net.name().as_deref().into_iter());
        let sources = Scope::plan(
            "voltage source",
            false,
            net.sources().iter().map(|s| s.name.as_str()),
        );
        Self {
            buses,
            line_codes,
            lines,
            transformers,
            loads,
            generators,
            ibrs,
            profiles,
            shunts,
            sources,
            circuit,
        }
    }

    fn scopes(&self) -> [&Scope; 11] {
        [
            &self.buses,
            &self.line_codes,
            &self.lines,
            &self.transformers,
            &self.loads,
            &self.generators,
            &self.ibrs,
            &self.profiles,
            &self.shunts,
            &self.sources,
            &self.circuit,
        ]
    }

    /// Rewrite every name and typed reference the plan replaces.
    fn rename(&self, net: &MulticonductorNetwork) -> MulticonductorNetwork {
        let mut out = net.clone();
        let t = out.tables_mut();
        if let Some(name) = &mut t.name {
            self.circuit.apply(name);
        }
        for bus in &mut t.buses {
            self.buses.apply(&mut bus.id);
        }
        for code in &mut t.line_codes {
            self.line_codes.apply(&mut code.name);
        }
        for line in &mut t.lines {
            self.lines.apply(&mut line.name);
            self.line_codes.apply(&mut line.linecode);
            self.buses.apply(&mut line.bus_from);
            self.buses.apply(&mut line.bus_to);
        }
        for switch in &mut t.switches {
            self.lines.apply(&mut switch.name);
            self.buses.apply(&mut switch.bus_from);
            self.buses.apply(&mut switch.bus_to);
        }
        for transformer in &mut t.transformers {
            self.transformers.apply(&mut transformer.name);
            for winding in &mut transformer.windings {
                self.buses.apply(&mut winding.bus);
            }
        }
        for load in &mut t.loads {
            self.loads.apply(&mut load.name);
            self.buses.apply(&mut load.bus);
        }
        for generator in &mut t.generators {
            self.generators.apply(&mut generator.name);
            self.buses.apply(&mut generator.bus);
        }
        for ibr in &mut t.ibrs {
            self.ibrs.apply(&mut ibr.name);
            self.buses.apply(&mut ibr.bus);
            if let Some(profile) = &mut ibr.control_profile {
                self.profiles.apply(profile);
            }
        }
        for profile in &mut t.control_profiles {
            self.profiles.apply(&mut profile.name);
        }
        for shunt in &mut t.shunts {
            self.shunts.apply(&mut shunt.name);
            self.buses.apply(&mut shunt.bus);
        }
        for capacitor in &mut t.capacitors {
            self.shunts.apply(&mut capacitor.name);
            self.buses.apply(&mut capacitor.bus);
        }
        for source in &mut t.sources {
            self.sources.apply(&mut source.name);
            self.buses.apply(&mut source.bus);
        }
        out
    }
}

/// The network with every name dss cannot carry replaced, and a finding per
/// replacement. Borrowed unchanged when every name is already safe.
pub(super) fn apply<'a>(
    net: &'a MulticonductorNetwork,
    warnings: &mut crate::diagnostics::Diagnostics,
) -> Cow<'a, MulticonductorNetwork> {
    let renames = Renames::plan(net);
    if renames
        .scopes()
        .iter()
        .all(|scope| scope.renamed.is_empty())
    {
        return Cow::Borrowed(net);
    }
    for scope in renames.scopes() {
        for (old, new) in &scope.renamed {
            let what = if scope.is_bus_id { "id" } else { "name" };
            warnings.push(
                &C::EMIT_DSS_VALUE_SUBSTITUTED,
                format!(
                    "{} `{old}`: {what} holds characters dss cannot carry in a token; written \
                     as `{new}`",
                    scope.label
                ),
            );
        }
    }
    Cow::Owned(renames.rename(net))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution_covers_every_character_dss_rejects() {
        for (name, bus) in [
            ("Cable type A", false),
            ("a,b=c!d\"e'f(g)h[i]j{k}l", false),
            ("x//y", false),
            ("tab\tnew\nline", false),
            ("node.1", true),
        ] {
            let safe = substitute(name, bus);
            assert!(!name_breaks_dss(&safe, bus), "{name} -> {safe}");
        }
        assert_eq!(substitute("node.1", false), "node.1");
        assert_eq!(substitute("Cable type A", false), "Cable_type_A");
    }

    #[test]
    fn a_replacement_avoids_names_the_class_already_holds() {
        let scope = Scope::plan("line", false, ["a_b", "a b", "A B", "a,b"].into_iter());
        assert_eq!(scope.renamed.get("a b").map(String::as_str), Some("a_b_2"));
        assert_eq!(scope.renamed.get("a,b").map(String::as_str), Some("a_b_3"));
        assert_eq!(scope.renamed.len(), 2);
    }
}
