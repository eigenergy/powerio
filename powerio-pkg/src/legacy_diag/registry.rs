//! Registry entries: how a crate declares the codes it emits.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use crate::legacy_diag::{DiagnosticSeverity, DiagnosticStage, ErrorCategory, code_is_well_formed};

/// Whether a code is still emitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeStatus {
    Active,
    /// No longer emitted. The entry stays so the identity is never reassigned
    /// and a document carrying the code still reads.
    Retired {
        since: &'static str,
    },
}

/// One registered code.
///
/// An emitting crate declares its codes as `DiagnosticInfo` constants and emits
/// through [`crate::StructuredDiagnostic::of`], so a code literal is written in
/// exactly one place and "every emitted code is registered" holds by
/// construction. There is no stage field: the code carries it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DiagnosticInfo {
    pub code: &'static str,
    /// The default severity. A site may raise or lower it.
    pub severity: DiagnosticSeverity,
    /// The coarse projection, for codes that can be fatal.
    pub category: Option<ErrorCategory>,
    /// One line: what the finding means.
    pub summary: &'static str,
    pub status: CodeStatus,
}

impl DiagnosticInfo {
    #[must_use]
    pub const fn new(
        code: &'static str,
        severity: DiagnosticSeverity,
        summary: &'static str,
    ) -> Self {
        Self {
            code,
            severity,
            category: None,
            summary,
            status: CodeStatus::Active,
        }
    }

    #[must_use]
    pub const fn with_category(mut self, category: ErrorCategory) -> Self {
        self.category = Some(category);
        self
    }

    #[must_use]
    pub const fn retired(mut self, since: &'static str) -> Self {
        self.status = CodeStatus::Retired { since };
        self
    }

    /// The namespace segment of the code.
    #[must_use]
    pub fn namespace(&self) -> &'static str {
        self.code.split('.').next().unwrap_or("")
    }

    /// The stage the code names, or `None` when the namespace is outside the
    /// ten. A registered code always decodes; the check below enforces it.
    #[must_use]
    pub fn stage(&self) -> Option<DiagnosticStage> {
        DiagnosticStage::from_namespace(self.namespace())
    }
}

/// Check a registry: every code matches the grammar, every namespace is one of
/// the ten, and no code appears twice. Returns one message per problem, empty
/// when the registry is sound.
///
/// A crate gates its own registry with this; the workspace gate runs it over
/// every registry concatenated, which is where a code shared by two crates
/// shows up.
pub fn check_registry<'a, I>(entries: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a DiagnosticInfo>,
{
    let mut problems = Vec::new();
    let mut seen: BTreeSet<&'static str> = BTreeSet::new();
    for entry in entries {
        // A retired code is a historical identity, kept so it is never
        // reassigned; some predate the grammar and cannot be made to satisfy
        // it without reassigning them.
        let retired = matches!(entry.status, CodeStatus::Retired { .. });
        if !retired && !code_is_well_formed(entry.code) {
            problems.push(format!("{}: does not match the code grammar", entry.code));
        } else if !retired && entry.stage().is_none() {
            problems.push(format!(
                "{}: namespace {} is not one of the ten",
                entry.code,
                entry.namespace()
            ));
        }
        if entry.summary.is_empty() {
            problems.push(format!("{}: has no summary", entry.code));
        }
        if !seen.insert(entry.code) {
            problems.push(format!("{}: registered twice", entry.code));
        }
    }
    problems
}

/// Check that no two crates declare codes under the same scope, i.e. the same
/// `NAMESPACE.SCOPE` prefix. Returns one message per shared scope.
///
/// A scope names one reader, one writer, or one pass, so two crates claiming it
/// means one of them is emitting from the other's territory. The workspace gate
/// runs this over every registry at once; a single crate cannot see it.
///
/// Retired entries are skipped: they name no live emitter, and a retired code
/// whose scope moved to another crate is exactly what retirement records.
pub fn check_scope_ownership(registries: &[(&str, &[&DiagnosticInfo])]) -> Vec<String> {
    let mut owner: BTreeMap<(&str, &str), &str> = BTreeMap::new();
    let mut problems = Vec::new();
    for (crate_name, entries) in registries {
        for entry in *entries {
            if matches!(entry.status, CodeStatus::Retired { .. }) {
                continue;
            }
            let mut segments = entry.code.split('.');
            let (Some(namespace), Some(scope)) = (segments.next(), segments.next()) else {
                continue;
            };
            match owner.entry((namespace, scope)) {
                Entry::Vacant(slot) => {
                    slot.insert(crate_name);
                }
                Entry::Occupied(slot) if *slot.get() != *crate_name => problems.push(format!(
                    "{namespace}.{scope}: claimed by both {} and {crate_name}",
                    slot.get()
                )),
                Entry::Occupied(_) => {}
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: DiagnosticInfo = DiagnosticInfo::new(
        "EMIT.PSSE.FIELD_DROPPED",
        DiagnosticSeverity::Warning,
        "a field with no PSS/E record was dropped",
    );

    #[test]
    fn a_sound_registry_reports_nothing() {
        let other = DiagnosticInfo::new(
            "READ.DSS.INCLUDE_REFUSED",
            DiagnosticSeverity::Error,
            "an include escaping the case directory was refused",
        )
        .with_category(ErrorCategory::Io);
        assert_eq!(check_registry([&GOOD, &other]), Vec::<String>::new());
        assert_eq!(GOOD.stage(), Some(DiagnosticStage::Emit));
        assert_eq!(GOOD.status, CodeStatus::Active);
    }

    #[test]
    fn the_check_names_each_way_a_registry_goes_wrong() {
        let malformed = DiagnosticInfo::new("emit.psse.dropped", DiagnosticSeverity::Info, "s");
        let unknown_namespace =
            DiagnosticInfo::new("FIDELITY.PSSE.DROPPED", DiagnosticSeverity::Info, "s");
        let no_summary = DiagnosticInfo::new("EMIT.PSSE.DEFAULTED", DiagnosticSeverity::Info, "");
        let problems = check_registry([&GOOD, &GOOD, &malformed, &unknown_namespace, &no_summary]);
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(problems.iter().any(|p| p.contains("registered twice")));
        assert!(problems.iter().any(|p| p.contains("code grammar")));
        assert!(problems.iter().any(|p| p.contains("not one of the ten")));
        assert!(problems.iter().any(|p| p.contains("no summary")));
    }

    #[test]
    fn two_crates_cannot_claim_one_scope() {
        const OTHER: DiagnosticInfo = DiagnosticInfo::new(
            "EMIT.PSSE.DOWNGRADED",
            DiagnosticSeverity::Warning,
            "a newer revision was written into an older layout",
        );
        const ELSEWHERE: DiagnosticInfo = DiagnosticInfo::new(
            "EMIT.BMOPF.TRANSFORMER_UNSUPPORTED",
            DiagnosticSeverity::Warning,
            "a transformer the BMOPF schema cannot state",
        );
        assert!(
            check_scope_ownership(&[
                ("powerio", &[&GOOD, &OTHER]),
                ("powerio-dist", &[&ELSEWHERE])
            ])
            .is_empty()
        );
        let problems = check_scope_ownership(&[("powerio", &[&GOOD]), ("powerio-dist", &[&OTHER])]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("EMIT.PSSE"), "{problems:?}");
    }

    #[test]
    fn a_retired_entry_records_when_it_stopped_being_emitted() {
        const RETIRED: DiagnosticInfo = DiagnosticInfo::new(
            "READ.DIST.PARSE_WARNING",
            DiagnosticSeverity::Warning,
            "a distribution parse warning with no identity of its own",
        )
        .retired("0.9.0");
        assert_eq!(RETIRED.status, CodeStatus::Retired { since: "0.9.0" });
        assert!(check_registry([&RETIRED]).is_empty());
    }
}
