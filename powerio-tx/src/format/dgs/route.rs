//! Which network family a DGS export justifies.
//!
//! PowerFactory stores every element with sequence parameters and a phase
//! technology. An export whose elements are all three phase and described by
//! sequence data alone is a balanced positive sequence case. An export that
//! states conductor identities, phase domain matrices, neutral conductors, or
//! phase specific equipment carries data the balanced model cannot hold, so
//! it routes to the multiconductor model. An export with no terminals carries
//! no topology and routes nowhere.

use super::tokens::{DgsDocument, DgsObject, DgsValue};

/// The family a document routes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DgsRoute {
    /// Sequence data only: the balanced transmission model.
    Balanced,
    /// Conductor level data; each entry names one attribute that justified it.
    Multiconductor(Vec<String>),
    /// Neither family is justified; the reason says why.
    Undecided(String),
}

/// Matrix attributes the sequence data classes carry that are not phase
/// domain matrices: zero and positive sequence circuit matrices, conductor
/// geometry, and line coordinates.
const SEQUENCE_MATRICES: [&str; 13] = [
    "R_c0", "X_c0", "B_c0", "G_c0", "C_c0", "R_c1", "X_c1", "B_c1", "G_c1", "C_c1", "xy_c", "xy_e",
    "GPScoords",
];

/// Classes whose presence states conductor identities.
const CONDUCTOR_CLASSES: [&str; 5] = ["TypCon", "TypGeo", "TypCabsys", "TypCab", "TypCabmult"];

/// Classes whose matrix attributes describe conductors.
const CONDUCTOR_MATRIX_CLASSES: [&str; 5] = ["TypTow", "TypCabsys", "TypGeo", "TypLne", "ElmLne"];

/// Decide the family for `document`.
#[must_use]
pub fn route(document: &DgsDocument) -> DgsRoute {
    if document.of_class("ElmTerm").next().is_none() {
        return DgsRoute::Undecided(
            "the export declares no ElmTerm terminal, so it carries no network topology; \
             export the study case network from PowerFactory, or read the file with a \
             reader for the classes it does carry"
                .to_owned(),
        );
    }
    let mut markers = Vec::new();
    let mut push = |object: &DgsObject, why: String| {
        markers.push(format!("{} `{}` (id {}): {why}", object.class(), object.name(), object.id));
    };
    for class in CONDUCTOR_CLASSES {
        for object in document.of_class(class) {
            push(object, "conductor identity".to_owned());
        }
    }
    for class in CONDUCTOR_MATRIX_CLASSES {
        for object in document.of_class(class) {
            for (name, value) in object.attributes() {
                if matches!(value, DgsValue::RealMatrix { .. })
                    && !SEQUENCE_MATRICES.contains(&name)
                {
                    push(object, format!("phase domain matrix `{name}`"));
                }
            }
        }
    }
    for terminal in document.of_class("ElmTerm") {
        if let Some(phtech) = terminal.int("phtech").filter(|code| *code != 0) {
            push(terminal, format!("phase technology `phtech={phtech}`"));
        }
    }
    for typ in document.of_class("TypLne") {
        if let Some(phases) = typ.int("nlnph").filter(|phases| *phases != 3) {
            push(typ, format!("`nlnph={phases}` phase conductors"));
        }
        if let Some(neutrals) = typ.int("nneutral").filter(|neutrals| *neutrals > 0) {
            push(typ, format!("`nneutral={neutrals}` neutral conductors"));
        }
    }
    for class in ["ElmLod", "ElmLodlv", "ElmLodlvp", "ElmLodmv"] {
        for load in document.of_class(class) {
            if load.int("i_sym") == Some(1) {
                push(load, "`i_sym=1` per phase demand".to_owned());
            }
            if let Some(phtech) = load.int("phtech").filter(|code| *code >= 2) {
                push(load, format!("phase technology `phtech={phtech}`"));
            }
        }
    }
    for class in ["ElmGenstat", "ElmPvsys", "ElmAsm"] {
        for generator in document.of_class(class) {
            if let Some(phtech) = generator.int("phtech").filter(|code| *code >= 2) {
                push(generator, format!("phase technology `phtech={phtech}`"));
            }
        }
    }
    for typ in document.of_class("TypSym") {
        if let Some(phases) = typ.int("nphase").filter(|phases| *phases != 3) {
            push(typ, format!("`nphase={phases}` machine"));
        }
    }
    for typ in document.of_class("TypTr2") {
        if let Some(phases) = typ.int("nt2ph").filter(|phases| *phases != 3) {
            push(typ, format!("`nt2ph={phases}` transformer"));
        }
    }
    for shunt in document.of_class("ElmShnt") {
        if let Some(ctech) = shunt.int("ctech").filter(|code| *code >= 2) {
            push(shunt, format!("phase technology `ctech={ctech}`"));
        }
    }
    for coupler in document.of_class("ElmCoup") {
        if let Some(phases) = coupler.int("nphase").filter(|phases| *phases != 3) {
            push(coupler, format!("`nphase={phases}` switch"));
        }
    }
    for cubicle in document.of_class("StaCubic") {
        if let Some(phases) = cubicle.int("nphase").filter(|phases| *phases != 3) {
            push(cubicle, format!("`nphase={phases}` connection"));
        }
        let mapping = (
            cubicle.int("it2p1"),
            cubicle.int("it2p2"),
            cubicle.int("it2p3"),
        );
        if let (Some(a), Some(b), Some(c)) = mapping
            && (a, b, c) != (0, 1, 2)
        {
            push(cubicle, format!("phase connection `it2p1..3={a},{b},{c}`"));
        }
    }
    if markers.is_empty() {
        DgsRoute::Balanced
    } else {
        DgsRoute::Multiconductor(markers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "$$General;ID(a:40);Descr(a:40);Val(a:40)\n1;Version;5.0\n";

    #[test]
    fn sequence_only_exports_are_balanced() {
        let text = format!(
            "{HEADER}$$ElmTerm;ID(a:40);loc_name(a:40);uknom(r);phtech(i)\n3;Bus;20;0\n\
             $$TypLne;ID(a:40);loc_name(a:40);nlnph(i);nneutral(i)\n4;Typ;3;0\n"
        );
        let document = DgsDocument::parse(&text).unwrap();
        assert_eq!(route(&document), DgsRoute::Balanced);
    }

    #[test]
    fn neutral_conductors_and_phase_technologies_route_multiconductor() {
        let text = format!(
            "{HEADER}$$ElmTerm;ID(a:40);loc_name(a:40);uknom(r);phtech(i)\n3;Bus;0.4;1\n\
             $$TypLne;ID(a:40);loc_name(a:40);nlnph(i);nneutral(i)\n4;Typ;3;1\n\
             $$ElmLod;ID(a:40);loc_name(a:40);i_sym(i)\n5;Load;1\n"
        );
        let document = DgsDocument::parse(&text).unwrap();
        let DgsRoute::Multiconductor(markers) = route(&document) else {
            panic!("expected the multiconductor route");
        };
        assert_eq!(markers.len(), 3, "{markers:?}");
        assert!(markers[0].contains("phtech=1"), "{markers:?}");
        assert!(markers[1].contains("nneutral=1"), "{markers:?}");
        assert!(markers[2].contains("i_sym=1"), "{markers:?}");
    }

    #[test]
    fn a_tower_with_sequence_circuit_matrices_stays_balanced() {
        let text = format!(
            "{HEADER}$$ElmTerm;ID(a:40);loc_name(a:40);uknom(r)\n3;Bus;400\n\
             $$TypTow;ID(a:40);loc_name(a:40);R_c1:SIZEROW(i);R_c1:SIZECOL(i);R_c1:0:0(r)\n\
             4;Tow;1;1;0.1\n"
        );
        let document = DgsDocument::parse(&text).unwrap();
        assert_eq!(route(&document), DgsRoute::Balanced);
        let text = format!(
            "{HEADER}$$ElmTerm;ID(a:40);loc_name(a:40);uknom(r)\n3;Bus;400\n\
             $$TypTow;ID(a:40);loc_name(a:40);R_c:SIZEROW(i);R_c:SIZECOL(i);R_c:0:0(r)\n\
             4;Tow;1;1;0.1\n"
        );
        let document = DgsDocument::parse(&text).unwrap();
        assert!(matches!(route(&document), DgsRoute::Multiconductor(_)));
    }

    #[test]
    fn an_export_without_terminals_is_undecided() {
        let text = format!("{HEADER}$$TypLne;ID(a:40);loc_name(a:40)\n4;Typ\n");
        let document = DgsDocument::parse(&text).unwrap();
        assert!(matches!(route(&document), DgsRoute::Undecided(_)));
    }
}
