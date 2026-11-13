//! The CIMXML (IEC 61970-552) subset CGMES instance files use, on quick-xml's
//! namespace-aware pull parser.
//!
//! One document is an `rdf:RDF` element holding an optional `md:FullModel`
//! header plus flat object elements. An object element's name is its CIM
//! class; `rdf:ID="_<uuid>"` defines an object, `rdf:about="#_<uuid>"` (or
//! `urn:uuid:<uuid>`) extends one defined elsewhere — that is how SSH and SV
//! files update EQ objects. Children are either text properties or
//! `rdf:resource` references; enum values are references whose fragment is
//! `EnumClass.value`. Forward references are legal, so the parser resolves
//! nothing; it returns the raw object soup for [`super::read`] to merge.

use quick_xml::events::Event;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use std::collections::{BTreeSet, HashSet};
use std::rc::Rc;

use crate::{Error, Result};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const MD_NS: &str = "http://iec.ch/TC57/61970-552/ModelDescription/1#";

/// An owned vocabulary name shared within one parsed document. It does not
/// borrow the XML input, so merged objects do not keep whole documents alive.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct CimName(Rc<str>);

impl CimName {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for CimName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CimName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, f)
    }
}

impl From<&str> for CimName {
    fn from(value: &str) -> Self {
        Self(Rc::from(value))
    }
}

impl From<String> for CimName {
    fn from(value: String) -> Self {
        Self(Rc::from(value))
    }
}

impl PartialEq<str> for CimName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for CimName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Bounded cache only: names beyond either bound remain accepted and owned.
/// This prevents many distinct extension names from growing an extra cache.
#[derive(Default)]
struct NamePool {
    names: HashSet<Rc<str>>,
    bytes: usize,
    // A document usually repeats one CIM URI on every class and property.
    // Cache its raw bytes only after classification and namespace recording.
    // At most 512 bytes; longer URIs keep the ordinary acceptance path.
    cim_namespace: Option<Box<[u8]>>,
}

impl NamePool {
    fn intern(&mut self, text: &str) -> CimName {
        if let Some(name) = self.names.get(text) {
            return CimName(Rc::clone(name));
        }
        let name: Rc<str> = Rc::from(text);
        if self.names.len() < 4096 && text.len() <= 512 && self.bytes + text.len() <= 1 << 20 {
            self.bytes += text.len();
            self.names.insert(Rc::clone(&name));
        }
        CimName(name)
    }
}

/// A property value: literal text, or the normalized target of an
/// `rdf:resource` (an object id, or an `EnumClass.value` fragment).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum PropValue {
    Text(String),
    Ref(String),
}

impl PropValue {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            PropValue::Text(s) | PropValue::Ref(s) => s,
        }
    }
}

/// One object element: class name (`eu:`/`entsoe:`-prefixed when not in the
/// `cim` namespace), normalized id, whether it was an `rdf:about` extension,
/// and its properties in document order.
#[derive(Debug)]
pub(crate) struct CimObject {
    pub class: CimName,
    pub id: String,
    pub definition: bool,
    pub props: Vec<(CimName, PropValue)>,
}

/// The `md:FullModel` header: the model uuid and its profile URIs (the file's
/// role in the set), plus the description the network name can come from.
#[derive(Debug, Default)]
pub(crate) struct ModelHeader {
    pub identity: Option<String>,
    pub profiles: Vec<String>,
    pub modeling_authority_set: Option<String>,
    pub description: Option<String>,
    pub scenario_time: Option<String>,
    pub created: Option<String>,
    pub version: Option<String>,
    pub dependent_on: Vec<String>,
    pub unmapped_properties: Vec<(String, PropValue)>,
    pub nested_properties: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct CimDocument {
    /// Every CIM namespace used by a class or property in this document.
    pub cim_namespaces: BTreeSet<String>,
    pub header: Option<ModelHeader>,
    pub objects: Vec<CimObject>,
}

/// Strip `urn:uuid:`, any `…#` prefix, and one leading `_` — the three id
/// spellings 61970-552 allows — to the bare identifier both `rdf:ID` and
/// `rdf:resource` forms share.
fn normalize_id(raw: &str) -> String {
    let s = raw.strip_prefix("urn:uuid:").unwrap_or(raw);
    let s = s.rsplit_once('#').map_or(s, |(_, frag)| frag);
    s.strip_prefix('_').unwrap_or(s).to_string()
}

/// A property/class name with its namespace collapsed to the conventional
/// prefix: bare for `cim` and `rdf`/`md`, `entsoe:`/`eu:` for the extension
/// namespaces (matched on their URI, not the file's chosen prefix letters),
/// `<last-uri-segment>:` for anything else (vendor extensions).
fn prefixed_name(
    resolve: &ResolveResult,
    local: &[u8],
    cim_namespaces: &mut BTreeSet<String>,
    names: &mut NamePool,
) -> CimName {
    let local = String::from_utf8_lossy(local);
    match resolve {
        ResolveResult::Bound(ns) => {
            if names.cim_namespace.as_deref() == Some(ns.as_ref()) {
                return names.intern(&local);
            }
            let uri = String::from_utf8_lossy(ns.as_ref());
            if uri.contains("CIM-schema-cim") || uri.contains("CIM100#") {
                if !cim_namespaces.contains(uri.as_ref()) {
                    cim_namespaces.insert(uri.into_owned());
                }
                if ns.as_ref().len() <= 512 {
                    names.cim_namespace = Some(ns.as_ref().into());
                }
                names.intern(&local)
            } else if uri == MD_NS {
                names.intern(&format!("md:{local}"))
            } else if uri.contains("entsoe.eu") {
                names.intern(&format!("entsoe:{local}"))
            } else if uri.contains("CIM100-European") {
                names.intern(&format!("eu:{local}"))
            } else if uri == RDF_NS {
                names.intern(&format!("rdf:{local}"))
            } else {
                let tag = uri
                    .trim_end_matches(['#', '/'])
                    .rsplit(['/', '#'])
                    .next()
                    .unwrap_or("x");
                names.intern(&format!("{tag}:{local}"))
            }
        }
        _ => names.intern(&local),
    }
}

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::FormatRead {
        format: "CGMES",
        message: e.to_string(),
    }
}

/// Parse one CIMXML instance file into its header and object soup.
#[allow(clippy::too_many_lines)] // one event loop, one arm per event kind
pub(crate) fn parse_cimxml(text: &str) -> Result<CimDocument> {
    let mut reader = NsReader::from_str(text);

    let mut cim_namespaces = BTreeSet::new();
    let mut names = NamePool::default();
    let mut header: Option<ModelHeader> = None;
    let mut objects: Vec<CimObject> = Vec::new();

    // Depth-1 = object elements, depth-2 = property elements. Deeper nesting
    // (rare vendor extensions) is skipped wholesale.
    let mut current: Option<CimObject> = None;
    let mut current_prop: Option<CimName> = None;
    let mut prop_text = String::new();
    let mut in_header = false;
    let mut current_prop_nested = false;
    let mut depth = 0usize;

    loop {
        match reader.read_resolved_event().map_err(xml_err)? {
            (resolve, Event::Start(start)) => {
                depth += 1;
                let name = prefixed_name(
                    &resolve,
                    start.local_name().as_ref(),
                    &mut cim_namespaces,
                    &mut names,
                );
                if depth == 1 && name != "rdf:RDF" {
                    return Err(xml_err(format!(
                        "top level element is `{name}`, not rdf:RDF"
                    )));
                }
                match depth {
                    2 => {
                        if name == "md:FullModel" {
                            in_header = true;
                            let header = header.get_or_insert_with(ModelHeader::default);
                            header.identity = rdf_identity_attr(&reader, &start)?;
                        } else {
                            current = Some(read_object(&reader, &start, name)?);
                        }
                    }
                    3 => {
                        current_prop = Some(name);
                        prop_text.clear();
                        current_prop_nested = false;
                        // An rdf:resource reference is usually self-closing,
                        // but a Start+End pair carries it on the attributes
                        // too.
                        if let Some(value) = resource_attr(&reader, &start)? {
                            push_prop(
                                in_header,
                                &mut header,
                                &mut current,
                                current_prop.take().unwrap_or_default(),
                                PropValue::Ref(value),
                            );
                        }
                    }
                    _ => {
                        if in_header && current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                            current_prop_nested = true;
                        }
                    }
                }
            }
            (resolve, Event::Empty(start)) => {
                let name = prefixed_name(
                    &resolve,
                    start.local_name().as_ref(),
                    &mut cim_namespaces,
                    &mut names,
                );
                if depth == 1 {
                    if name == "md:FullModel" {
                        let header = header.get_or_insert_with(ModelHeader::default);
                        header.identity = rdf_identity_attr(&reader, &start)?;
                    } else {
                        objects.push(read_object(&reader, &start, name)?);
                    }
                } else if depth == 2 {
                    if let Some(value) = resource_attr(&reader, &start)? {
                        push_prop(
                            in_header,
                            &mut header,
                            &mut current,
                            name,
                            PropValue::Ref(value),
                        );
                    } else {
                        push_prop(
                            in_header,
                            &mut header,
                            &mut current,
                            name,
                            PropValue::Text(String::new()),
                        );
                    }
                } else if depth >= 3 && in_header {
                    current_prop_nested = true;
                }
            }
            (_, Event::Text(t)) => {
                if !current_prop_nested && current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                    prop_text.push_str(&t.xml10_content().map_err(xml_err)?);
                }
            }
            (_, Event::CData(t)) => {
                if !current_prop_nested && current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                    prop_text.push_str(&t.decode().map_err(xml_err)?);
                }
            }
            // `&amp;`-style references arrive as their own events.
            (_, Event::GeneralRef(r)) => {
                if !current_prop_nested && current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                    let resolved =
                        crate::format::xml::resolve_general_ref(&r).ok_or_else(|| {
                            xml_err("CGMES XML contains an undeclared or external entity reference")
                        })?;
                    prop_text.push_str(&resolved);
                }
            }
            (_, Event::DocType(_)) => {
                return Err(xml_err("CGMES XML must not contain a DTD"));
            }
            (_, Event::End(_)) => {
                match depth {
                    3 => {
                        if let Some(prop) = current_prop.take()
                            && !prop.is_empty()
                        {
                            if current_prop_nested && in_header {
                                if let Some(header) = header.as_mut() {
                                    header.nested_properties.push(prop.to_string());
                                }
                                prop_text.clear();
                            } else {
                                let value = prop_text.trim().to_string();
                                prop_text.clear();
                                push_prop(
                                    in_header,
                                    &mut header,
                                    &mut current,
                                    prop,
                                    PropValue::Text(value),
                                );
                            }
                        }
                        current_prop_nested = false;
                    }
                    2 => {
                        in_header = false;
                        if let Some(obj) = current.take() {
                            objects.push(obj);
                        }
                    }
                    _ => {}
                }
                depth = depth.saturating_sub(1);
            }
            (_, Event::Eof) => break,
            (_, _) => {}
        }
    }

    let has_cim16 = cim_namespaces
        .iter()
        .any(|namespace| namespace.contains("CIM-schema-cim16"));
    let has_cim100 = cim_namespaces
        .iter()
        .any(|namespace| namespace.contains("CIM100"));
    if has_cim16 && has_cim100 {
        return Err(xml_err(format!(
            "one CGMES XML document mixes CIM16 and CIM100 namespaces: {}",
            cim_namespaces
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }

    Ok(CimDocument {
        cim_namespaces,
        header,
        objects,
    })
}

fn rdf_identity_attr<R>(
    reader: &NsReader<R>,
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<String>> {
    for attr in start.attributes() {
        let attr = attr.map_err(xml_err)?;
        let (namespace, local) = reader.resolver().resolve_attribute(attr.key);
        if is_rdf_attribute(&namespace, local.as_ref(), b"about")
            || is_rdf_attribute(&namespace, local.as_ref(), b"ID")
        {
            return Ok(Some(normalize_id(&String::from_utf8_lossy(&attr.value))));
        }
    }
    Ok(None)
}

fn read_object<R>(
    reader: &NsReader<R>,
    start: &quick_xml::events::BytesStart<'_>,
    class: CimName,
) -> Result<CimObject> {
    let mut id = String::new();
    let mut definition = false;
    for attr in start.attributes() {
        let attr = attr.map_err(xml_err)?;
        let (namespace, local) = reader.resolver().resolve_attribute(attr.key);
        if is_rdf_attribute(&namespace, local.as_ref(), b"ID") {
            definition = true;
            id = normalize_id(&String::from_utf8_lossy(&attr.value));
        } else if is_rdf_attribute(&namespace, local.as_ref(), b"about") {
            id = normalize_id(&String::from_utf8_lossy(&attr.value));
        }
    }
    Ok(CimObject {
        class,
        id,
        definition,
        props: Vec::new(),
    })
}

/// Numeric character references resolve directly; the five XML predefined
/// entities by name. Anything else (a DTD-defined entity, absent from CGMES
/// practice) contributes nothing.
fn is_rdf_attribute(resolve: &ResolveResult, local: &[u8], expected_local: &[u8]) -> bool {
    matches!(resolve, ResolveResult::Bound(namespace) if namespace.as_ref() == RDF_NS.as_bytes())
        && local == expected_local
}

fn resource_attr<R>(
    reader: &NsReader<R>,
    start: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<String>> {
    for attr in start.attributes() {
        let attr = attr.map_err(xml_err)?;
        let (namespace, local) = reader.resolver().resolve_attribute(attr.key);
        if is_rdf_attribute(&namespace, local.as_ref(), b"resource") {
            return Ok(Some(normalize_id(&String::from_utf8_lossy(&attr.value))));
        }
    }
    Ok(None)
}

fn push_prop(
    in_header: bool,
    header: &mut Option<ModelHeader>,
    current: &mut Option<CimObject>,
    prop: CimName,
    value: PropValue,
) {
    if in_header {
        if let Some(header) = header.as_mut() {
            match prop.as_str() {
                "md:Model.profile" => header.profiles.push(value.as_str().to_string()),
                "md:Model.modelingAuthoritySet" => {
                    header.modeling_authority_set = Some(value.as_str().to_string());
                }
                "md:Model.description" => header.description = Some(value.as_str().to_string()),
                "md:Model.scenarioTime" => {
                    header.scenario_time = Some(value.as_str().to_string());
                }
                "md:Model.created" => header.created = Some(value.as_str().to_string()),
                "md:Model.version" => header.version = Some(value.as_str().to_string()),
                "md:Model.DependentOn" => {
                    header.dependent_on.push(value.as_str().to_string());
                }
                _ => header.unmapped_properties.push((prop.to_string(), value)),
            }
        }
    } else if let Some(obj) = current.as_mut() {
        obj.props.push((prop, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_names_share_ownership_after_namespace_resolution() {
        let document = parse_cimxml(r#"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:a="http://iec.ch/TC57/CIM100#"
            xmlns:b="http://iec.ch/TC57/CIM100#"
            xmlns:e="http://entsoe.eu/CIM/SchemaExtension/3/1#">
            <a:BaseVoltage rdf:ID="_one"><a:IdentifiedObject.name>one</a:IdentifiedObject.name></a:BaseVoltage>
            <b:BaseVoltage rdf:ID="_two"><b:IdentifiedObject.name>two</b:IdentifiedObject.name><e:IdentifiedObject.name>extension</e:IdentifiedObject.name></b:BaseVoltage>
        </rdf:RDF>"#).unwrap();
        let first = &document.objects[0];
        let second = &document.objects[1];
        assert!(Rc::ptr_eq(&first.class.0, &second.class.0));
        assert!(Rc::ptr_eq(&first.props[0].0.0, &second.props[0].0.0));
        assert_eq!(first.props[0].1.as_str(), "one");
        assert_eq!(second.props[0].1.as_str(), "two");
        assert_eq!(second.props[1].0, "entsoe:IdentifiedObject.name");
        assert!(!Rc::ptr_eq(&second.props[0].0.0, &second.props[1].0.0));
    }

    #[test]
    fn repeated_cim_uris_preserve_rebinding_and_unicode_names() {
        let doc = parse_cimxml(r#"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:c="http://iec.ch/TC57/CIM100#">
            <c:BaseVoltage rdf:ID="_one">
                <c:IdentifiedObject.name>one</c:IdentifiedObject.name>
                <c:étiquette>énergie</c:étiquette>
                <c:IdentifiedObject.name xmlns:c="https://example.test/vendor#">vendor</c:IdentifiedObject.name>
                <c:BaseVoltage.nominalVoltage>110</c:BaseVoltage.nominalVoltage>
            </c:BaseVoltage>
            <c:BaseVoltage xmlns:c="https://example.test/CIM100#" rdf:ID="_two"/>
            <c:BaseVoltage rdf:ID="_three"/>
        </rdf:RDF>"#).unwrap();
        assert_eq!(doc.objects.len(), 3);
        assert!(doc.objects.iter().all(|o| o.class == "BaseVoltage"));
        assert_eq!(doc.objects[0].props[1].0, "étiquette");
        assert_eq!(doc.objects[0].props[1].1.as_str(), "énergie");
        assert_eq!(doc.objects[0].props[2].0, "vendor:IdentifiedObject.name");
        assert_eq!(doc.objects[0].props[3].0, "BaseVoltage.nominalVoltage");
        assert_eq!(
            doc.cim_namespaces,
            BTreeSet::from([
                "http://iec.ch/TC57/CIM100#".to_owned(),
                "https://example.test/CIM100#".to_owned(),
            ])
        );
    }

    #[test]
    fn long_cim_namespace_is_recorded_without_growing_the_cache() {
        let mut names = NamePool::default();
        let mut namespaces = BTreeSet::new();
        let short = b"http://iec.ch/TC57/CIM100#";
        let long = format!("https://example.test/{}/CIM100#", "x".repeat(513));
        for uri in [short.as_slice(), long.as_bytes(), short.as_slice()] {
            assert_eq!(
                prefixed_name(
                    &ResolveResult::Bound(quick_xml::name::Namespace(uri)),
                    b"BaseVoltage",
                    &mut namespaces,
                    &mut names,
                ),
                "BaseVoltage"
            );
            assert_eq!(names.cim_namespace.as_deref(), Some(short.as_slice()));
        }
        assert!(namespaces.contains(long.as_str()));
        assert_eq!(namespaces.len(), 2);
    }

    #[test]
    fn cache_limits_do_not_reject_or_change_uncached_names() {
        let mut pool = NamePool::default();
        let original = pool.intern("original");
        for index in 0..5000 {
            pool.intern(&format!("field{index}"));
        }
        assert_eq!(pool.names.len(), 4096);
        assert!(Rc::ptr_eq(&original.0, &pool.intern("original").0));
        let first = pool.intern("beyond-the-cache");
        let second = pool.intern("beyond-the-cache");
        assert_eq!(first, second);
        assert!(!Rc::ptr_eq(&first.0, &second.0));
    }

    #[test]
    fn oversized_names_are_accepted_without_entering_a_fresh_cache() {
        let mut pool = NamePool::default();
        let long_name = "x".repeat(513);
        let first = pool.intern(&long_name);
        let second = pool.intern(&long_name);
        assert_eq!(first.as_str(), long_name);
        assert_eq!(first, second);
        assert!(!Rc::ptr_eq(&first.0, &second.0));
        assert!(pool.names.is_empty());
        assert_eq!(pool.bytes, 0);
    }

    #[test]
    fn name_cache_has_an_independent_byte_budget() {
        let mut pool = NamePool::default();
        for index in 0..3000 {
            pool.intern(&format!("{index:0512}"));
        }
        assert_eq!(pool.bytes, 1 << 20);
        assert!(pool.names.len() < 4096);
        assert_eq!(pool.intern("new-name").as_str(), "new-name");
        assert_eq!(pool.bytes, 1 << 20);
    }

    const SAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:c="http://iec.ch/TC57/2013/CIM-schema-cim16#"
         xmlns:e="http://entsoe.eu/CIM/SchemaExtension/3/1#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">
  <md:FullModel rdf:about="urn:uuid:aaaa-bbbb">
    <md:Model.description>demo &amp; test</md:Model.description>
    <md:Model.modelingAuthoritySet>https://example.test/operator</md:Model.modelingAuthoritySet>
    <md:Model.profile>http://entsoe.eu/CIM/EquipmentCore/3/1</md:Model.profile>
    <md:Model.profile>http://entsoe.eu/CIM/EquipmentOperation/3/1</md:Model.profile>
  </md:FullModel>
  <c:TopologicalNode rdf:ID="_tn1">
    <c:IdentifiedObject.name>BUS 1</c:IdentifiedObject.name>
    <c:TopologicalNode.BaseVoltage rdf:resource="#_bv1"/>
  </c:TopologicalNode>
  <c:EnergyConsumer rdf:about="#_load1">
    <c:EnergyConsumer.p>23.5</c:EnergyConsumer.p>
    <c:RegulatingControl.mode rdf:resource="http://iec.ch/TC57/2013/CIM-schema-cim16#RegulatingControlModeKind.voltage"/>
    <e:IdentifiedObject.shortName>L1</e:IdentifiedObject.shortName>
  </c:EnergyConsumer>
  <c:Terminal rdf:about="urn:uuid:t-99">
    <c:ACDCTerminal.connected>true</c:ACDCTerminal.connected>
  </c:Terminal>
</rdf:RDF>"##;

    #[test]
    fn parses_header_objects_props_and_references() {
        let doc = parse_cimxml(SAMPLE).unwrap();
        assert!(
            doc.cim_namespaces
                .contains("http://iec.ch/TC57/2013/CIM-schema-cim16#")
        );
        let header = doc.header.unwrap();
        assert_eq!(header.profiles.len(), 2);
        assert_eq!(
            header.modeling_authority_set.as_deref(),
            Some("https://example.test/operator")
        );
        assert_eq!(header.description.as_deref(), Some("demo & test"));

        assert_eq!(doc.objects.len(), 3);
        let tn = &doc.objects[0];
        assert_eq!(tn.class, "TopologicalNode");
        assert_eq!(tn.id, "tn1");
        assert_eq!(
            tn.props[0],
            (
                "IdentifiedObject.name".into(),
                PropValue::Text("BUS 1".into())
            )
        );
        assert_eq!(
            tn.props[1],
            (
                "TopologicalNode.BaseVoltage".into(),
                PropValue::Ref("bv1".into())
            )
        );

        let load = &doc.objects[1];
        assert_eq!(load.id, "load1");
        // Enum reference normalizes to its EnumClass.value fragment; the
        // extension property keeps its entsoe: prefix regardless of the
        // file's chosen letters.
        assert_eq!(
            load.props[1],
            (
                "RegulatingControl.mode".into(),
                PropValue::Ref("RegulatingControlModeKind.voltage".into())
            )
        );
        assert_eq!(load.props[2].0, "entsoe:IdentifiedObject.shortName");

        // urn:uuid ids normalize like #_ ids.
        assert_eq!(doc.objects[2].id, "t-99");
    }

    #[test]
    fn parses_self_closing_objects_and_cdata_literals() {
        let source = r#"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:c="http://iec.ch/TC57/CIM100#">
          <c:BaseFrequency rdf:ID="_frequency"/>
          <c:TopologicalNode rdf:ID="_node">
            <c:IdentifiedObject.name><![CDATA[North <bus> & yard]]></c:IdentifiedObject.name>
          </c:TopologicalNode>
        </rdf:RDF>"#;
        let document = parse_cimxml(source).unwrap();
        assert_eq!(document.objects.len(), 2);
        assert_eq!(document.objects[0].class, "BaseFrequency");
        assert_eq!(document.objects[0].id, "frequency");
        assert!(document.objects[0].definition);
        assert_eq!(
            document.objects[1].props,
            vec![(
                "IdentifiedObject.name".into(),
                PropValue::Text("North <bus> & yard".into())
            )]
        );
    }

    #[test]
    fn rejects_non_rdf_documents() {
        assert!(parse_cimxml("<html><body/></html>").is_err());
    }

    #[test]
    fn retains_full_model_fields_and_identifies_nested_rdf() {
        let source = r#"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#"
            xmlns:cim="http://iec.ch/TC57/CIM100#">
          <md:FullModel rdf:about="urn:uuid:model-1">
            <md:Model.created>2026-01-02T03:04:05Z</md:Model.created>
            <md:Model.version>7</md:Model.version>
            <md:Model.DependentOn rdf:resource="urn:uuid:model-0"/>
            <md:Model.Supersedes rdf:resource="urn:uuid:older"/>
            <md:Model.empty/>
            <md:Model.custom><rdf:Description rdf:about="urn:uuid:nested"/></md:Model.custom>
          </md:FullModel>
          <cim:BaseVoltage rdf:ID="_base"/>
        </rdf:RDF>"#;
        let document = parse_cimxml(source).unwrap();
        let header = document.header.unwrap();
        assert_eq!(header.identity.as_deref(), Some("model-1"));
        assert_eq!(header.created.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(header.version.as_deref(), Some("7"));
        assert_eq!(header.dependent_on, ["model-0"]);
        assert_eq!(
            header.unmapped_properties,
            [
                ("md:Model.Supersedes".into(), PropValue::Ref("older".into())),
                ("md:Model.empty".into(), PropValue::Text(String::new()))
            ]
        );
        assert_eq!(header.nested_properties, ["md:Model.custom"]);
    }

    #[test]
    fn rejects_one_document_that_mixes_cim16_and_cim100() {
        let source = r#"<rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:cim16="http://iec.ch/TC57/2013/CIM-schema-cim16#"
            xmlns:cim100="http://iec.ch/TC57/CIM100#">
          <cim16:BaseVoltage rdf:ID="_base"/>
          <cim100:TopologicalNode rdf:ID="_node"/>
        </rdf:RDF>"#;
        let message = parse_cimxml(source).unwrap_err().to_string();
        assert!(message.contains("mixes CIM16 and CIM100 namespaces"));
        assert!(message.contains("CIM-schema-cim16"));
        assert!(message.contains("CIM100"));
    }

    #[test]
    fn recognizes_rdf_attributes_by_namespace_not_suffix() {
        let text = r##"<root:RDF
            xmlns:root="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:alt="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
            xmlns:cim="http://iec.ch/TC57/CIM100#"
            xmlns:vendor="https://vendor.example/schema#">
          <cim:TopologicalNode alt:ID="_real">
            <cim:TopologicalNode.BaseVoltage alt:resource="#_base"/>
          </cim:TopologicalNode>
          <cim:BaseVoltage alt:about="#_base">
            <cim:BaseVoltage.nominalVoltage>110</cim:BaseVoltage.nominalVoltage>
          </cim:BaseVoltage>
          <cim:EnergyConsumer vendor:ID="_false-id">
            <cim:EnergyConsumer.p>1</cim:EnergyConsumer.p>
          </cim:EnergyConsumer>
          <cim:Terminal alt:about="#_terminal">
            <cim:Terminal.ConductingEquipment vendor:resource="#_false-reference"/>
          </cim:Terminal>
        </root:RDF>"##;

        let doc = parse_cimxml(text).unwrap();
        assert_eq!(doc.objects[0].id, "real");
        assert!(doc.objects[0].definition);
        assert_eq!(
            doc.objects[0].props,
            vec![(
                "TopologicalNode.BaseVoltage".into(),
                PropValue::Ref("base".into())
            )]
        );
        assert_eq!(doc.objects[1].id, "base");
        assert!(!doc.objects[1].definition);
        assert_eq!(doc.objects[2].id, "");
        assert_eq!(
            doc.objects[3].props,
            vec![(
                "Terminal.ConductingEquipment".into(),
                PropValue::Text(String::new())
            )]
        );
    }
}
