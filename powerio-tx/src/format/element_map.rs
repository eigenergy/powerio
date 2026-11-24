//! Element map plumbing shared by the balanced writers.
//!
//! A writer that records an element map starts a builder with [`builder`],
//! records each bus, branch, and generator row as it writes it, and attaches
//! the builder to its [`TextEmission`](super::TextEmission) with
//! `with_element_map`. The emission path finishes the builder through
//! [`finish`], which adds the map's findings to the emission and returns the
//! map that `EmitResult::element_map` carries.

use std::borrow::Cow;

use powerio_core::__implementation::ElementMapBuilder;
use powerio_core::{Diagnostic, ElementMap};

use crate::network::BalancedNetwork;

use super::TargetFormat;

/// A builder for `format` over every bus, branch, and generator of `net`.
///
/// Keys are the element `uid`s. When a record has none, the builder keys on
/// the identity `assign_missing_component_ids` (and so `serialize`) would
/// give it, computed on a copy: the caller's network and the written bytes do
/// not change.
pub(crate) fn builder(net: &BalancedNetwork, format: TargetFormat) -> ElementMapBuilder {
    builder_for_token(net, format.token())
}

/// [`builder`] for a target with no [`TargetFormat`], such as the PyPSA CSV
/// directory, named by its format token.
pub(crate) fn builder_for_token(net: &BalancedNetwork, token: &str) -> ElementMapBuilder {
    let missing = net.buses().iter().any(|bus| bus.uid.is_none())
        || net.branches().iter().any(|branch| branch.uid.is_none())
        || net
            .generators()
            .iter()
            .any(|generator| generator.uid.is_none());
    let net = if missing {
        let mut copy = net.clone();
        copy.assign_missing_component_ids();
        Cow::Owned(copy)
    } else {
        Cow::Borrowed(net)
    };
    let uid = |uid: &Option<String>| {
        uid.clone()
            .expect("assign_missing_component_ids gives every record a uid")
    };
    ElementMapBuilder::new(
        token,
        net.buses().iter().map(|bus| uid(&bus.uid)),
        net.branches().iter().map(|branch| uid(&branch.uid)),
        net.generators().iter().map(|generator| uid(&generator.uid)),
    )
}

/// Finish a writer's builder, appending its findings to `diagnostics`.
///
/// # Errors
/// The writer recorded two elements for one row or key (a writer bug the map
/// refuses to hide).
pub(crate) fn finish(
    builder: Option<ElementMapBuilder>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Option<ElementMap>, powerio_core::Error> {
    let Some(builder) = builder else {
        return Ok(None);
    };
    let (map, findings) = builder.finish()?;
    diagnostics.extend(findings);
    Ok(map)
}

/// Attach `map` to a committed emission result.
pub(crate) fn attach(
    result: powerio_core::EmitResult,
    map: Option<ElementMap>,
) -> powerio_core::EmitResult {
    match map {
        Some(map) => result.__with_element_map(map),
        None => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::test_parse::parse_file;

    /// A network built without uids keys its map on the uids
    /// `assign_missing_component_ids` would give, and the network itself is
    /// left as it was.
    #[test]
    fn missing_uids_key_on_the_assigned_uids() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case4par.m");
        let mut bare = parse_file(path, None).unwrap().network;
        for bus in bare.buses_mut() {
            bus.uid = None;
        }
        for branch in bare.branches_mut() {
            branch.uid = None;
        }
        for generator in bare.generators_mut() {
            generator.uid = None;
        }
        let mut assigned = bare.clone();
        assigned.assign_missing_component_ids();

        let written = super::super::matpower::write_matpower_conversion(&bare);
        let mut diagnostics = Vec::new();
        let map = finish(written.element_map, &mut diagnostics)
            .unwrap()
            .unwrap();
        assert_eq!(diagnostics, Vec::<Diagnostic>::new());
        let uids: Vec<&str> = map.branch().map(|(uid, _)| uid).collect();
        let expected: Vec<&str> = assigned
            .branches()
            .iter()
            .map(|branch| branch.uid.as_deref().unwrap())
            .collect();
        assert_eq!(uids, expected);
        assert_eq!(uids[..2], ["10-20", "10-20-2"]);
        // A load at bus 20 keeps `bus-20`, so the generator there takes a
        // suffix: the uids are unique across the whole network.
        let generators: Vec<&str> = map.generator().map(|(uid, _)| uid).collect();
        let expected: Vec<&str> = assigned
            .generators()
            .iter()
            .map(|generator| generator.uid.as_deref().unwrap())
            .collect();
        assert_eq!(generators, expected);
        assert!(bare.branches().iter().all(|branch| branch.uid.is_none()));
    }
}
