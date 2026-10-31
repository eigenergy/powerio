//! The CIMXML (IEC 61970-552) subset distribution CIM instance files use, on
//! quick-xml's namespace-aware pull parser.
//!
//! Distribution CIM serializes with the same RDF/XML syntax as CGMES, so this
//! module follows the CGMES reader in `powerio-tx` (`format/cgmes/xml.rs`).
//! `powerio-dist` does not depend on the transmission crate, so it keeps its
//! own copy of these reading rules.
//!
//! One document is an `rdf:RDF` element holding an optional `md:FullModel`
//! header plus flat object elements. An object element's name is its CIM
//! class; `rdf:ID="_<id>"` defines an object, and `rdf:about="#_<id>"` (or
//! `urn:uuid:<id>`) extends one defined elsewhere. Children are either text
//! properties or `rdf:resource` references; enum values are references whose
//! fragment is `EnumClass.value`. Forward references are legal, so the parser
//! resolves nothing; [`super::read`] merges the objects and checks the
//! references the mapping follows.

use std::borrow::Cow;
use std::collections::BTreeSet;

use quick_xml::XmlVersion;
use quick_xml::encoding::Decoder;
use quick_xml::events::Event;
use quick_xml::events::attributes::Attribute;
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use crate::{Error, Result};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const MD_NS: &str = "http://iec.ch/TC57/61970-552/ModelDescription/1#";

/// Distribution CIM documents are XML 1.0: a literal tab, carriage return,
/// or line feed in an attribute value becomes one space, and a character
/// reference keeps its character.
const XML_VERSION: XmlVersion = XmlVersion::Implicit1_0;

/// A property value: literal text, or the normalized target of an
/// `rdf:resource` (an object id, or an `EnumClass.value` fragment).
#[derive(Debug, Clone, PartialEq)]
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
/// `cim` namespace), normalized id, whether it was an `rdf:ID` definition
/// rather than an `rdf:about` extension, its properties in document order,
/// and the names of properties that hold nested elements instead of a value.
#[derive(Debug)]
pub(crate) struct CimObject {
    pub class: String,
    pub id: String,
    pub definition: bool,
    pub props: Vec<(String, PropValue)>,
    pub nested: Vec<String>,
}

/// The `md:FullModel` header fields the distribution mapping reads: the
/// profile URIs and the description a network name can come from.
#[derive(Debug, Default)]
pub(crate) struct ModelHeader {
    pub profiles: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CimDocument {
    /// Every CIM namespace used by a class or property in this document.
    pub cim_namespaces: BTreeSet<String>,
    pub header: Option<ModelHeader>,
    pub objects: Vec<CimObject>,
}

/// Strip `urn:uuid:`, any `…#` prefix, and one leading `_` (the three id
/// spellings 61970-552 allows) to the bare identifier both `rdf:ID` and
/// `rdf:resource` forms share.
fn normalize_id(raw: &str) -> String {
    let s = raw.strip_prefix("urn:uuid:").unwrap_or(raw);
    let s = s.rsplit_once('#').map_or(s, |(_, frag)| frag);
    s.strip_prefix('_').unwrap_or(s).to_string()
}

/// Whether a namespace URI is a CIM schema namespace: the versioned CIM16
/// and older spellings, or the CIM100 namespace CIM17 and later use.
pub(crate) fn is_cim_namespace(uri: &str) -> bool {
    uri.contains("CIM-schema-cim") || uri.contains("CIM100#")
}

/// A property/class name with its namespace collapsed to the conventional
/// prefix: bare for `cim`, `rdf:`/`md:` for those, `entsoe:`/`eu:` for the
/// extension namespaces and `sedms:` for the Schneider Electric ADMS one
/// (matched on their URI, not the file's chosen prefix letters),
/// `<last-uri-segment>:` for anything else (vendor extensions).
fn prefixed_name(
    resolve: &ResolveResult,
    local: &[u8],
    cim_namespaces: &mut BTreeSet<String>,
) -> String {
    let local = String::from_utf8_lossy(local).into_owned();
    match resolve {
        ResolveResult::Bound(ns) => {
            let uri = String::from_utf8_lossy(ns.as_ref());
            if is_cim_namespace(&uri) {
                if !cim_namespaces.contains(uri.as_ref()) {
                    cim_namespaces.insert(uri.into_owned());
                }
                local
            } else if uri == MD_NS {
                format!("md:{local}")
            } else if uri.contains("entsoe.eu") {
                format!("entsoe:{local}")
            } else if uri.contains("CIM100-European") {
                format!("eu:{local}")
            } else if uri.contains("schneider-electric-dms.com") {
                format!("sedms:{local}")
            } else if uri == RDF_NS {
                format!("rdf:{local}")
            } else {
                let tag = uri
                    .trim_end_matches(['#', '/'])
                    .rsplit(['/', '#'])
                    .next()
                    .unwrap_or("x");
                format!("{tag}:{local}")
            }
        }
        _ => local,
    }
}

fn xml_err(e: impl std::fmt::Display) -> Error {
    Error::FormatRead {
        format: "distribution CIM",
        message: e.to_string(),
    }
}

/// Parse one CIMXML instance file into its header and object soup.
#[allow(clippy::too_many_lines)] // one event loop, one arm per event kind
pub(crate) fn parse_cimxml(text: &str) -> Result<CimDocument> {
    let mut reader = NsReader::from_str(text);

    let mut cim_namespaces = BTreeSet::new();
    let mut header: Option<ModelHeader> = None;
    let mut objects: Vec<CimObject> = Vec::new();

    // Depth 1 is `rdf:RDF`, depth 2 the object elements, depth 3 their
    // properties. A property holding nested elements (rare vendor
    // extensions) has no literal value: its text is discarded and the
    // property is recorded as nested on its object.
    let mut current: Option<CimObject> = None;
    let mut current_prop: Option<String> = None;
    let mut prop_text = String::new();
    let mut in_header = false;
    let mut current_prop_nested = false;
    let mut depth = 0usize;

    loop {
        match reader.read_resolved_event().map_err(xml_err)? {
            (resolve, Event::Start(start)) => {
                depth += 1;
                let name =
                    prefixed_name(&resolve, start.local_name().as_ref(), &mut cim_namespaces);
                if depth == 1 && name != "rdf:RDF" {
                    return Err(xml_err(format!(
                        "top level element is `{name}`, not rdf:RDF"
                    )));
                }
                match depth {
                    2 => {
                        if name == "md:FullModel" {
                            in_header = true;
                            header.get_or_insert_with(ModelHeader::default);
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
                            current_prop = Some(String::new());
                        }
                    }
                    _ => {
                        if current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                            current_prop_nested = true;
                        }
                    }
                }
            }
            (resolve, Event::Empty(start)) => {
                let name =
                    prefixed_name(&resolve, start.local_name().as_ref(), &mut cim_namespaces);
                if depth == 1 {
                    if name == "md:FullModel" {
                        header.get_or_insert_with(ModelHeader::default);
                    } else {
                        objects.push(read_object(&reader, &start, name)?);
                    }
                } else if depth == 2 {
                    let value = match resource_attr(&reader, &start)? {
                        Some(value) => PropValue::Ref(value),
                        None => PropValue::Text(String::new()),
                    };
                    push_prop(in_header, &mut header, &mut current, name, value);
                } else if depth >= 3 && current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
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
                    let resolved = resolve_general_ref(&r).ok_or_else(|| {
                        xml_err(
                            "distribution CIM XML contains an undeclared or external entity \
                             reference",
                        )
                    })?;
                    prop_text.push_str(&resolved);
                }
            }
            (_, Event::DocType(_)) => {
                return Err(xml_err("distribution CIM XML must not contain a DTD"));
            }
            (_, Event::End(_)) => {
                match depth {
                    3 => {
                        if let Some(prop) = current_prop.take()
                            && !prop.is_empty()
                        {
                            let value = prop_text.trim().to_string();
                            prop_text.clear();
                            // A property holding nested elements has no
                            // literal value. The header fields cannot take
                            // one; an object records it for reporting.
                            if !current_prop_nested {
                                push_prop(
                                    in_header,
                                    &mut header,
                                    &mut current,
                                    prop,
                                    PropValue::Text(value),
                                );
                            } else if !in_header && let Some(object) = current.as_mut() {
                                object.nested.push(prop);
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
            "one distribution CIM document mixes CIM16 and CIM100 namespaces: {}",
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

/// The character a general entity reference in element text stands for: a
/// character reference, or one of the five predefined XML entities. Any
/// other name is `None`, since the reader accepts no DTD that could define
/// one.
fn resolve_general_ref(reference: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if let Ok(Some(character)) = reference.resolve_char_ref() {
        return Some(character.to_string());
    }
    match reference.xml10_content().ok()?.as_ref() {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        _ => None,
    }
}

/// The text of one attribute: transcoded to UTF-8, the predefined entities
/// expanded, and normalized under [`XML_VERSION`].
fn attribute_value<'a>(attribute: &Attribute<'a>, decoder: Decoder) -> Result<Cow<'a, str>> {
    attribute
        .decoded_and_normalized_value(XML_VERSION, decoder)
        .map_err(xml_err)
}

fn read_object<R>(
    reader: &NsReader<R>,
    start: &quick_xml::events::BytesStart<'_>,
    class: String,
) -> Result<CimObject> {
    let mut id = String::new();
    let mut definition = false;
    for attr in start.attributes() {
        let attr = attr.map_err(xml_err)?;
        let (namespace, local) = reader.resolver().resolve_attribute(attr.key);
        if is_rdf_attribute(&namespace, local.as_ref(), b"ID") {
            definition = true;
            id = normalize_id(&attribute_value(&attr, reader.decoder())?);
        } else if is_rdf_attribute(&namespace, local.as_ref(), b"about") {
            id = normalize_id(&attribute_value(&attr, reader.decoder())?);
        }
    }
    Ok(CimObject {
        class,
        id,
        definition,
        props: Vec::new(),
        nested: Vec::new(),
    })
}

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
            return Ok(Some(normalize_id(&attribute_value(
                &attr,
                reader.decoder(),
            )?)));
        }
    }
    Ok(None)
}

fn push_prop(
    in_header: bool,
    header: &mut Option<ModelHeader>,
    current: &mut Option<CimObject>,
    prop: String,
    value: PropValue,
) {
    if in_header {
        if let Some(header) = header.as_mut() {
            match prop.as_str() {
                "md:Model.profile" => header.profiles.push(value.as_str().to_string()),
                "md:Model.description" => header.description = Some(value.as_str().to_string()),
                _ => {}
            }
        }
    } else if let Some(obj) = current.as_mut() {
        obj.props.push((prop, value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:c="http://iec.ch/TC57/CIM100#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">
  <md:FullModel rdf:about="urn:uuid:aaaa-bbbb">
    <md:Model.description>demo &amp; test</md:Model.description>
    <md:Model.profile>http://iec.ch/TC57/ns/CIM/Distribution/1</md:Model.profile>
  </md:FullModel>
  <c:ConnectivityNode rdf:ID="_cn1">
    <c:IdentifiedObject.name>BUS 1</c:IdentifiedObject.name>
  </c:ConnectivityNode>
  <c:EnergyConsumer rdf:about="#_load1">
    <c:EnergyConsumer.p>23.5</c:EnergyConsumer.p>
    <c:EnergyConsumer.phaseConnection rdf:resource="http://iec.ch/TC57/CIM100#PhaseShuntConnectionKind.D"/>
  </c:EnergyConsumer>
  <c:Terminal rdf:about="urn:uuid:t-99">
    <c:Terminal.ConnectivityNode rdf:resource="#_cn1"/>
  </c:Terminal>
  <c:BaseFrequency rdf:ID="_f"/>
</rdf:RDF>"##;

    #[test]
    fn a_property_holding_nested_elements_has_no_value() {
        let doc = parse_cimxml(
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:c="http://iec.ch/TC57/CIM100#" xmlns:v="http://vendor.example/ext#">
  <c:EnergyConsumer rdf:ID="_load1">
    <c:EnergyConsumer.p><v:A>12</v:A><v:B>3</v:B></c:EnergyConsumer.p>
    <c:EnergyConsumer.q><v:C/></c:EnergyConsumer.q>
    <c:IdentifiedObject.name>load1</c:IdentifiedObject.name>
  </c:EnergyConsumer>
</rdf:RDF>"#,
        )
        .unwrap();
        let load = &doc.objects[0];
        assert_eq!(load.nested, ["EnergyConsumer.p", "EnergyConsumer.q"]);
        assert_eq!(load.props.len(), 1);
        assert_eq!(load.props[0].0, "IdentifiedObject.name");
    }

    #[test]
    fn parses_header_objects_props_and_references() {
        let doc = parse_cimxml(SAMPLE).unwrap();
        assert_eq!(
            doc.cim_namespaces.iter().collect::<Vec<_>>(),
            ["http://iec.ch/TC57/CIM100#"]
        );
        let header = doc.header.unwrap();
        assert_eq!(header.profiles.len(), 1);
        assert_eq!(header.description.as_deref(), Some("demo & test"));

        assert_eq!(doc.objects.len(), 4);
        let cn = &doc.objects[0];
        assert_eq!(cn.class, "ConnectivityNode");
        assert_eq!(cn.id, "cn1");
        assert!(cn.definition);
        assert_eq!(
            cn.props[0],
            (
                "IdentifiedObject.name".into(),
                PropValue::Text("BUS 1".into())
            )
        );

        let load = &doc.objects[1];
        assert_eq!(load.id, "load1");
        assert!(!load.definition);
        // An enum reference normalizes to its EnumClass.value fragment.
        assert_eq!(
            load.props[1],
            (
                "EnergyConsumer.phaseConnection".into(),
                PropValue::Ref("PhaseShuntConnectionKind.D".into())
            )
        );

        // urn:uuid ids normalize like #_ ids, and a self-closing object
        // element is an object with no properties.
        assert_eq!(doc.objects[2].id, "t-99");
        assert_eq!(doc.objects[3].class, "BaseFrequency");
        assert_eq!(doc.objects[3].props, []);
    }

    #[test]
    fn decodes_entities_in_identifiers_and_text() {
        let doc = parse_cimxml(
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                        xmlns:cim="http://iec.ch/TC57/CIM100#">
              <cim:ConnectivityNode rdf:ID="_a&amp;b">
                <cim:IdentifiedObject.name><![CDATA[x<y]]>&#38;z</cim:IdentifiedObject.name>
              </cim:ConnectivityNode>
            </rdf:RDF>"#,
        )
        .unwrap();
        assert_eq!(doc.objects[0].id, "a&b");
        assert_eq!(doc.objects[0].props[0].1.as_str(), "x<y&z");
    }

    #[test]
    fn recognizes_rdf_attributes_by_namespace_not_suffix() {
        let doc = parse_cimxml(
            r##"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                        xmlns:x="urn:vendor#"
                        xmlns:cim="http://iec.ch/TC57/CIM100#">
              <cim:Terminal x:ID="_wrong" rdf:ID="_right">
                <cim:Terminal.ConnectivityNode x:resource="#_wrong"/>
              </cim:Terminal>
            </rdf:RDF>"##,
        )
        .unwrap();
        assert_eq!(doc.objects[0].id, "right");
        assert_eq!(doc.objects[0].props[0].1, PropValue::Text(String::new()));
    }

    #[test]
    fn rejects_non_rdf_documents_dtds_and_unknown_entities() {
        assert!(parse_cimxml("<html><body/></html>").is_err());
        assert!(
            parse_cimxml(
                r#"<!DOCTYPE rdf:RDF [<!ENTITY e "x">]>
                <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>"#
            )
            .is_err()
        );
        assert!(
            parse_cimxml(
                r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                            xmlns:cim="http://iec.ch/TC57/CIM100#">
                  <cim:ConnectivityNode rdf:ID="_n">
                    <cim:IdentifiedObject.name>&unknown;</cim:IdentifiedObject.name>
                  </cim:ConnectivityNode>
                </rdf:RDF>"#
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_one_document_that_mixes_cim16_and_cim100() {
        let error = parse_cimxml(
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                        xmlns:a="http://iec.ch/TC57/2013/CIM-schema-cim16#"
                        xmlns:b="http://iec.ch/TC57/CIM100#">
              <a:ConnectivityNode rdf:ID="_n1"/>
              <b:ConnectivityNode rdf:ID="_n2"/>
            </rdf:RDF>"#,
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("mixes CIM16 and CIM100"),
            "{error}"
        );
    }
}
