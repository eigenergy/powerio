//! CIM vocabulary shared by the adapters that speak it: CGMES in
//! `powerio-tx`, distribution CIM in `powerio-dist`, and MG-RAVENS, whose
//! balanced profile lives in `powerio-tx`, whose multiconductor profile lives
//! in `powerio-dist`, and whose two profiles meet in `powerio-tnd`.
//!
//! Those crates depend on this one and not on each other, so whatever two of
//! them must agree on is stated once here: how a stable identifier derives
//! from an object's class and name, how CIM phase kinds map onto wire
//! coordinate terminal names, and which classes mark each MG-RAVENS profile.

/// A UUID version 5 identifier for `kind:name` under `namespace`, a URL
/// naming the adapter that writes it. Same inputs give the same id, so a
/// canonical write is byte stable and two writes of one case diff only where
/// the case changed. The CGMES and distribution CIM writers derive their ids
/// the same way under their own namespaces.
#[must_use]
pub fn deterministic_mrid(namespace: &str, kind: &str, name: &str) -> String {
    let namespace = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, namespace.as_bytes());
    uuid::Uuid::new_v5(&namespace, format!("{kind}:{name}").as_bytes()).to_string()
}

/// The wire coordinate terminal name (an OpenDSS node number) of a CIM
/// `SinglePhaseKind` letter: `A`, `B`, `C`, and `N` are terminals 1 to 4,
/// and the split phase secondaries `s1` and `s2` are terminals 1 and 2.
#[must_use]
pub fn phase_terminal(letter: &str) -> Option<&'static str> {
    match letter {
        "A" | "s1" => Some("1"),
        "B" | "s2" => Some("2"),
        "C" => Some("3"),
        "N" => Some("4"),
        _ => None,
    }
}

/// The CIM `SinglePhaseKind` letter of a terminal name: the node numbers 1
/// to 4, and the letter names `a`, `b`, `c`, `n` in either case that BMOPF
/// terminal conventions use. Split phase collapses onto `A` and `B`. Any
/// other name has no phase kind, and a writer reports it.
#[must_use]
pub fn terminal_phase(terminal: &str) -> Option<&'static str> {
    match terminal {
        "1" | "a" | "A" => Some("A"),
        "2" | "b" | "B" => Some("B"),
        "3" | "c" | "C" => Some("C"),
        "4" | "n" | "N" => Some("N"),
        _ => None,
    }
}

/// Expand a `PhaseCode.<code>` or `OrderedPhaseCodeKind.<code>` value into
/// its letters: `ABCN` gives `A`, `B`, `C`, `N`, and `s12N` (or the
/// `s1s2N` spelling) gives `s1`, `s2`, `N`. Letters outside the kind are skipped.
#[must_use]
pub fn expand_phase_code(code: &str) -> Vec<String> {
    let code = code.rsplit('.').next().unwrap_or(code);
    let mut out = Vec::new();
    let mut chars = code.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            'A' | 'B' | 'C' | 'N' => out.push(c.to_string()),
            's' => {
                if let Some(d) = chars.next() {
                    out.push(format!("s{d}"));
                    if d == '1' && chars.peek() == Some(&'2') {
                        chars.next();
                        out.push("s2".to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// MG-RAVENS, the LANL CIM-derived microgrid interchange schema: one JSON
/// document of name-keyed hash tables nested under the CIM class hierarchy.
/// Its two profiles share the token and the top level shape and differ in
/// the classes they use, so the profile follows the content.
pub mod ravens {
    /// The format token both profiles share.
    pub const TOKEN: &str = "ravens-json";

    /// The namespace both profiles derive deterministic mRIDs under.
    pub const NAMESPACE: &str = "https://powerio.dev/ravens";

    /// Classes only the balanced profile uses: the MATPOWER converter's
    /// settings record, polynomial costs, and two-end transformers.
    pub const BALANCED_MARKERS: [&str; 3] = [
        "AlgorithmSettings",
        "ProducerCostFunction",
        "PowerTransformerEnd",
    ];

    /// Classes only the multiconductor profile uses: per-phase equipment,
    /// transformer tanks, per-length impedance matrices, and wire catalogs.
    pub const MULTICONDUCTOR_MARKERS: [&str; 9] = [
        "ACLineSegmentPhase",
        "EnergyConsumerPhase",
        "PowerElectronicsConnectionPhase",
        "SwitchPhase",
        "TransformerTank",
        "TransformerTankEnd",
        "PerLengthPhaseImpedance",
        "PerLengthLineParameter",
        "WireSpacingInfo",
    ];

    /// The profile a document's classes select.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Profile {
        /// The MATPOWER derived register, read into a balanced network. A
        /// document with neither profile's markers reads as this one.
        Balanced,
        /// The OpenDSS derived register, read into a multiconductor network.
        Multiconductor,
        /// Both registers at once, which no reader takes.
        Mixed,
    }

    impl Profile {
        /// The profile's name in messages.
        #[must_use]
        pub fn name(self) -> &'static str {
            match self {
                Self::Balanced => "balanced",
                Self::Multiconductor => "multiconductor",
                Self::Mixed => "mixed",
            }
        }
    }

    /// The profile of MG-RAVENS JSON text, from decoded
    /// `Ravens.cimObjectType` values and the schema's class containers.
    /// Object names and ordinary string properties do not select a profile.
    /// Malformed or excessively nested JSON defaults to the balanced route,
    /// whose reader reports the JSON error; the decoder keeps its depth limit.
    #[must_use]
    pub fn detect_profile(text: &str) -> Profile {
        use serde_json::Value;

        let Ok(root) = serde_json::from_str::<Value>(text) else {
            return Profile::Balanced;
        };
        // These are structural containers in the MG-RAVENS schema, not
        // arbitrary hash keys: ConnectivityNode["TransformerTank"], for
        // example, names a bus and is not a transformer class statement.
        let mut balanced = root
            .get("ProducerCostFunction")
            .is_some_and(Value::is_object);
        let mut multiconductor = root
            .get("PerLengthLineParameter")
            .is_some_and(Value::is_object)
            || root
                .get("AssetInfo")
                .and_then(|info| info.get("WireSpacingInfo"))
                .is_some_and(Value::is_object);
        visit_classes(&root, &mut balanced, &mut multiconductor);
        match (balanced, multiconductor) {
            (true, true) => Profile::Mixed,
            (false, true) => Profile::Multiconductor,
            _ => Profile::Balanced,
        }
    }

    fn visit_classes(value: &serde_json::Value, balanced: &mut bool, multiconductor: &mut bool) {
        use serde_json::Value;

        match value {
            Value::Object(object) => {
                if let Some(kind) = object.get("Ravens.cimObjectType").and_then(Value::as_str) {
                    *balanced |= BALANCED_MARKERS.contains(&kind);
                    *multiconductor |= MULTICONDUCTOR_MARKERS.contains(&kind);
                }
                for child in object.values() {
                    visit_classes(child, balanced, multiconductor);
                }
            }
            Value::Array(items) => {
                for child in items {
                    visit_classes(child, balanced, multiconductor);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_mrids_are_stable_version_5_uuids() {
        let a = deterministic_mrid(ravens::NAMESPACE, "ConnectivityNode", "1");
        assert_eq!(
            a,
            deterministic_mrid(ravens::NAMESPACE, "ConnectivityNode", "1")
        );
        assert_ne!(
            a,
            deterministic_mrid(ravens::NAMESPACE, "ConnectivityNode", "2")
        );
        assert_ne!(
            a,
            deterministic_mrid("https://powerio.dev/cim-dist", "ConnectivityNode", "1")
        );
        assert_eq!(a.len(), 36);
        assert_eq!(a.as_bytes()[14], b'5', "version nibble");
    }

    #[test]
    fn phase_kinds_map_both_ways() {
        for (letter, terminal) in [("A", "1"), ("B", "2"), ("C", "3"), ("N", "4")] {
            assert_eq!(phase_terminal(letter), Some(terminal));
            assert_eq!(terminal_phase(terminal), Some(letter));
            assert_eq!(terminal_phase(&letter.to_ascii_lowercase()), Some(letter));
        }
        assert_eq!(phase_terminal("s1"), Some("1"));
        assert_eq!(phase_terminal("s2"), Some("2"));
        assert_eq!(terminal_phase("5"), None);
        assert_eq!(terminal_phase("x"), None);
        assert_eq!(expand_phase_code("PhaseCode.ABCN"), ["A", "B", "C", "N"]);
        assert_eq!(expand_phase_code("OrderedPhaseCodeKind.CA"), ["C", "A"]);
        assert_eq!(expand_phase_code("PhaseCode.s1s2N"), ["s1", "s2", "N"]);
        assert_eq!(expand_phase_code("PhaseCode.s12"), ["s1", "s2"]);
        assert_eq!(expand_phase_code("PhaseCode.s12N"), ["s1", "s2", "N"]);
        assert_eq!(expand_phase_code("A"), ["A"]);
    }

    #[test]
    fn ravens_profiles_follow_the_classes_present() {
        use ravens::{Profile, detect_profile};
        assert_eq!(
            detect_profile(r#"{"ConnectivityNode":{}}"#),
            Profile::Balanced
        );
        assert_eq!(
            detect_profile(r#"{"x":{"Ravens.cimObjectType":"ProducerCostFunction"}}"#),
            Profile::Balanced
        );
        assert_eq!(
            detect_profile(r#"{"PerLengthLineParameter":{}}"#),
            Profile::Multiconductor
        );
        assert_eq!(
            detect_profile(r#"[{"Ravens.cimObjectType":"ACLineSegmentPhase"}]"#),
            Profile::Multiconductor
        );
        assert_eq!(
            detect_profile(
                r#"{"PerLengthLineParameter":{},"y":{"Ravens.cimObjectType":"AlgorithmSettings"}}"#
            ),
            Profile::Mixed
        );
        // A pointer or a property key is not a class statement.
        assert_eq!(
            detect_profile(r#"{"a":"PerLengthPhaseImpedance::'lc'","PowerTransformerEnd.r":1}"#),
            Profile::Balanced
        );
    }

    #[test]
    fn ravens_profiles_ignore_names_and_descriptions() {
        use ravens::{Profile, detect_profile};
        for marker in ravens::MULTICONDUCTOR_MARKERS {
            let text = serde_json::json!({
                "ConnectivityNode": {marker: {
                    "Ravens.cimObjectType": "ConnectivityNode",
                    "IdentifiedObject.name": marker,
                    "IdentifiedObject.description": marker
                }},
                "MySettings": {"Ravens.cimObjectType": "AlgorithmSettings"}
            });
            assert_eq!(
                detect_profile(&text.to_string()),
                Profile::Balanced,
                "{marker}"
            );
        }
        for marker in ravens::BALANCED_MARKERS {
            let text = serde_json::json!({
                "ConnectivityNode": {marker: {"IdentifiedObject.description": marker}},
                "PerLengthLineParameter": {}
            });
            assert_eq!(
                detect_profile(&text.to_string()),
                Profile::Multiconductor,
                "{marker}"
            );
        }
    }

    #[test]
    fn ravens_profiles_decode_escapes_and_structural_containers() {
        use ravens::{Profile, detect_profile};
        for text in [
            r#"{"PerLengthLine\u0050arameter":{}}"#,
            r#"{"AssetInfo":{"WireSpacing\u0049nfo":{}}}"#,
            r#"{"x":{"Ravens.cimObject\u0054ype":"ACLineSegment\u0050hase"}}"#,
        ] {
            assert_eq!(detect_profile(text), Profile::Multiconductor, "{text}");
        }
        assert_eq!(detect_profile(r#"{"AssetInfo":{}}"#), Profile::Balanced);
        assert_eq!(
            detect_profile(r#"{"ProducerCost\u0046unction":{},"PerLengthLineParameter":{}}"#),
            Profile::Mixed
        );
    }

    #[test]
    fn malformed_ravens_profile_input_is_bounded() {
        use ravens::{Profile, detect_profile};
        for text in [
            "",
            "{",
            r#"{"Ravens.cimObjectType":"TransformerTank"} trailing"#,
        ] {
            assert_eq!(detect_profile(text), Profile::Balanced);
        }
        let text = format!("{}null{}", "[".repeat(2048), "]".repeat(2048));
        assert_eq!(detect_profile(&text), Profile::Balanced);
    }
}
