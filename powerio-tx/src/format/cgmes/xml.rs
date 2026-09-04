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

use std::collections::BTreeSet;

use crate::{Error, Result};

const RDF_NS: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const MD_NS: &str = "http://iec.ch/TC57/61970-552/ModelDescription/1#";

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
/// `cim` namespace), normalized id, whether it was an `rdf:about` extension,
/// and its properties in document order.
#[derive(Debug)]
pub(crate) struct CimObject {
    pub class: String,
    pub id: String,
    pub definition: bool,
    pub props: Vec<(String, PropValue)>,
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
) -> String {
    let local = String::from_utf8_lossy(local).into_owned();
    match resolve {
        ResolveResult::Bound(ns) => {
            let uri = String::from_utf8_lossy(ns.as_ref()).into_owned();
            if uri.contains("CIM-schema-cim") || uri.contains("CIM100#") {
                cim_namespaces.insert(uri.clone());
                local
            } else if uri == MD_NS {
                format!("md:{local}")
            } else if uri.contains("entsoe.eu") {
                format!("entsoe:{local}")
            } else if uri.contains("CIM100-European") {
                format!("eu:{local}")
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
        format: "CGMES",
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

    // Depth-1 = object elements, depth-2 = property elements. Deeper nesting
    // (rare vendor extensions) is skipped wholesale.
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
                            current_prop = Some(String::new());
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
                let name =
                    prefixed_name(&resolve, start.local_name().as_ref(), &mut cim_namespaces);
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
                                    header.nested_properties.push(prop);
                                }
                                prop_text.clear();
                            } else {
                                let value = std::mem::take(&mut prop_text);
                                push_prop(
                                    in_header,
                                    &mut header,
                                    &mut current,
                                    prop,
                                    PropValue::Text(value.trim().to_string()),
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
    class: String,
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
    prop: String,
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
                _ => header.unmapped_properties.push((prop, value)),
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
        assert!(doc.objects[2].id.is_empty());
        assert_eq!(
            doc.objects[3].props,
            vec![(
                "Terminal.ConductingEquipment".into(),
                PropValue::Text(String::new())
            )]
        );
    }
}
