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
    pub profiles: Vec<String>,
    pub description: Option<String>,
    pub scenario_time: Option<String>,
}

#[derive(Debug)]
pub(crate) struct CimDocument {
    /// The resolved `cim:` namespace URI (None if no cim-prefixed content).
    pub cim_namespace: Option<String>,
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
fn prefixed_name(resolve: &ResolveResult, local: &[u8], cim_ns: &mut Option<String>) -> String {
    let local = String::from_utf8_lossy(local).into_owned();
    match resolve {
        ResolveResult::Bound(ns) => {
            let uri = String::from_utf8_lossy(ns.as_ref()).into_owned();
            if uri.contains("CIM-schema-cim") || uri.contains("CIM100#") {
                if cim_ns.is_none() {
                    *cim_ns = Some(uri);
                }
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

    let mut cim_ns: Option<String> = None;
    let mut header: Option<ModelHeader> = None;
    let mut objects: Vec<CimObject> = Vec::new();

    // Depth-1 = object elements, depth-2 = property elements. Deeper nesting
    // (rare vendor extensions) is skipped wholesale.
    let mut current: Option<CimObject> = None;
    let mut current_prop: Option<String> = None;
    let mut prop_text = String::new();
    let mut in_header = false;
    let mut depth = 0usize;

    loop {
        match reader.read_resolved_event().map_err(xml_err)? {
            (resolve, Event::Start(start)) => {
                depth += 1;
                let name = prefixed_name(&resolve, start.local_name().as_ref(), &mut cim_ns);
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
                            let mut id = String::new();
                            let mut definition = false;
                            for attr in start.attributes() {
                                let attr = attr.map_err(xml_err)?;
                                let key = attr.key.as_ref();
                                if key.ends_with(b"ID") {
                                    definition = true;
                                    id = normalize_id(&String::from_utf8_lossy(&attr.value));
                                } else if key.ends_with(b"about") {
                                    id = normalize_id(&String::from_utf8_lossy(&attr.value));
                                }
                            }
                            current = Some(CimObject {
                                class: name,
                                id,
                                definition,
                                props: Vec::new(),
                            });
                        }
                    }
                    3 => {
                        current_prop = Some(name);
                        prop_text.clear();
                        // An rdf:resource reference is usually self-closing,
                        // but a Start+End pair carries it on the attributes
                        // too.
                        if let Some(value) = resource_attr(&start)? {
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
                    _ => {}
                }
            }
            (resolve, Event::Empty(start)) => {
                let name = prefixed_name(&resolve, start.local_name().as_ref(), &mut cim_ns);
                if depth == 2 {
                    if let Some(value) = resource_attr(&start)? {
                        push_prop(
                            in_header,
                            &mut header,
                            &mut current,
                            name,
                            PropValue::Ref(value),
                        );
                    }
                }
            }
            (_, Event::Text(t)) => {
                if current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                    prop_text.push_str(&t.xml10_content().map_err(xml_err)?);
                }
            }
            // `&amp;`-style references arrive as their own events.
            (_, Event::GeneralRef(r)) => {
                if current_prop.as_deref().is_some_and(|p| !p.is_empty()) {
                    let resolved = resolve_general_ref(&r).ok_or_else(|| {
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
                        if let Some(prop) = current_prop.take() {
                            if !prop.is_empty() {
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

    Ok(CimDocument {
        cim_namespace: cim_ns,
        header,
        objects,
    })
}

/// Numeric character references resolve directly; the five XML predefined
/// entities by name. Anything else (a DTD-defined entity, absent from CGMES
/// practice) contributes nothing.
fn resolve_general_ref(r: &quick_xml::events::BytesRef<'_>) -> Option<String> {
    if let Ok(Some(ch)) = r.resolve_char_ref() {
        return Some(ch.to_string());
    }
    match r.xml10_content().ok()?.as_ref() {
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" => Some("'".to_string()),
        _ => None,
    }
}

fn resource_attr(start: &quick_xml::events::BytesStart<'_>) -> Result<Option<String>> {
    for attr in start.attributes() {
        let attr = attr.map_err(xml_err)?;
        if attr.key.as_ref().ends_with(b"resource") {
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
                "md:Model.description" => header.description = Some(value.as_str().to_string()),
                "md:Model.scenarioTime" => {
                    header.scenario_time = Some(value.as_str().to_string());
                }
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
         xmlns:c="http://iec.ch/TC57/2013/CIM-schema-cim16#"
         xmlns:e="http://entsoe.eu/CIM/SchemaExtension/3/1#"
         xmlns:md="http://iec.ch/TC57/61970-552/ModelDescription/1#">
  <md:FullModel rdf:about="urn:uuid:aaaa-bbbb">
    <md:Model.description>demo &amp; test</md:Model.description>
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
        assert_eq!(
            doc.cim_namespace.as_deref(),
            Some("http://iec.ch/TC57/2013/CIM-schema-cim16#")
        );
        let header = doc.header.unwrap();
        assert_eq!(header.profiles.len(), 2);
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
    fn rejects_non_rdf_documents() {
        assert!(parse_cimxml("<html><body/></html>").is_err());
    }
}
